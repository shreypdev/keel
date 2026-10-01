import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { SseError, type SseEvent, WsError, type WsMessage } from "../src/adapters/types.js";
import type { PortImpl } from "../src/port.js";
import {
  DID_NOT_KEEP_UP,
  HEADERS_REFUSED,
  type PlatformWebSocket,
  type WebSocketAdapter,
  type WebSocketConstructorLike,
  browserWebSocket,
  fetchSse,
  nodeWebSocket,
  ssePort,
  webSocketPort,
} from "../src/realtime.js";
import { err, ok, sseCalls, text, wsCalls } from "./support/port-calls.js";
import { type RealtimeServer, lastOn, startRealtimeServer, stopRealtimeServer, until } from "./support/realtime-server.js";

/*
 * The shared failure-injection suite of the default real-time adapters (the brief, section 5),
 * through their bindings, against contract-tests/servers/realtime-server.mjs: echo, subprotocol and
 * headers, a refused upgrade, the peer's close, a drop, a text frame that is not UTF-8, a flood under
 * a stalled reader; the SSE feed, a resume, refusals, the wrong type, a hang closed by the client.
 *
 * Two WebSocket adapters run it: `nodeWebSocket` (Node's sockets and the runtime's framing: every
 * case as the brief writes it, the flood pushed back by TCP) and `browserWebSocket` over Node's
 * global `WebSocket` (undici), which reads the cases the way a browser can: no refusal status, a
 * bad UTF-8 frame reported like a drop, no 1001 from script, and a flood that ends `Closed(1008)`.
 */

let server: RealtimeServer;

beforeAll(async () => {
  server = await startRealtimeServer();
});

afterAll(async () => {
  await stopRealtimeServer(server);
});

beforeEach(() => {
  server.reset();
});

/** Reads `count` messages (or until the connection's end), pulling up to 16 at a time like the core. */
async function read(api: ReturnType<typeof wsCalls>, conn: number, count: number): Promise<{ messages: WsMessage[]; end: WsError | null }> {
  const messages: WsMessage[] = [];
  while (messages.length < count) {
    const got = await api.receive(conn, Math.min(16, count - messages.length));
    if ("err" in got) return { messages, end: got.err };
    if (got.ok.length === 0) break;
    messages.push(...got.ok);
  }
  return { messages, end: null };
}

/** Reads until the connection ends, and returns everything with the end. */
async function readToEnd(api: ReturnType<typeof wsCalls>, conn: number): Promise<{ messages: WsMessage[]; end: WsError | null }> {
  return read(api, conn, Number.MAX_SAFE_INTEGER);
}

interface Case {
  readonly name: string;
  readonly adapter: () => WebSocketAdapter;
  /** What a refused upgrade reports as its status. */
  readonly refusedStatus: number | null;
  /** How a text frame that is not UTF-8 ends the connection. */
  readonly badUtf8: "protocol" | "network";
  /** The close code the server sees when the core goes away (1001), or `1005` (none) where a script cannot send 1001. */
  readonly goingAway: number;
}

const CASES: readonly Case[] = [
  { name: "nodeWebSocket", adapter: () => nodeWebSocket(), refusedStatus: 401, badUtf8: "protocol", goingAway: 1001 },
  { name: "browserWebSocket (Node's global WebSocket)", adapter: () => browserWebSocket(), refusedStatus: null, badUtf8: "network", goingAway: 1005 },
];

describe.each(CASES)("$name against the realtime server", ({ adapter, refusedStatus, badUtf8, goingAway }) => {
  let port: PortImpl;
  let api: ReturnType<typeof wsCalls>;

  beforeEach(() => {
    port = webSocketPort(adapter());
    api = wsCalls(port);
  });

  it("echoes text and binary in order, and the server sees the core's close", async () => {
    const { conn, protocol } = ok(await api.connect(`${server.wsUrl}/ws/echo`));
    expect(protocol).toBe("");
    const sent: WsMessage[] = [text("a"), { kind: "binary", value: Uint8Array.of(1, 2, 3) }, text("é")];
    for (const m of sent) ok(await api.send(conn, m));
    expect((await read(api, conn, 3)).messages).toEqual(sent);
    ok(await api.close(conn, 1000, "done"));
    const seen = await until("the server to see the close", () => {
      const c = lastOn(server, "/ws/echo");
      return c?.closeCode !== null ? c : undefined;
    });
    expect([seen.closeCode, seen.closeReason]).toEqual([1000, "done"]);
  });

  it("offers subprotocols, sends headers, and closes with the core's code and reason", async () => {
    const { conn, protocol } = ok(await api.connect(`${server.wsUrl}/ws/headers`, ["v2", "v1"], [{ name: "X-Token", value: "t" }]));
    expect(protocol).toBe("v2");
    const [first] = (await read(api, conn, 1)).messages;
    expect(first?.kind).toBe("text");
    expect(JSON.parse((first as { value: string }).value)).toMatchObject({ "x-token": "t", "sec-websocket-protocol": "v2, v1" });
    ok(await api.send(conn, text("ping")));
    expect((await read(api, conn, 1)).messages).toEqual([text("ping")]);
    ok(await api.close(conn, 4000, "bye"));
    const seen = await until("the server to see the close", () => {
      const c = lastOn(server, "/ws/headers");
      return c?.closeCode !== null ? c : undefined;
    });
    expect([seen.closeCode, seen.closeReason]).toEqual([4000, "bye"]);
  });

  it(`a refused upgrade is Refused (status ${String(refusedStatus)})`, async () => {
    const refused = err(await api.connect(`${server.wsUrl}/ws/deny?status=401`));
    expect(refused).toBeInstanceOf(WsError.Refused);
    expect((refused as WsError.Refused).status).toBe(refusedStatus);
  });

  it("the peer's close frame ends the stream with its code and reason, after what came before", async () => {
    const { conn } = ok(await api.connect(`${server.wsUrl}/ws/close?code=4001&reason=kicked`));
    expect(await readToEnd(api, conn)).toEqual({ messages: [text("hello")], end: new WsError.Closed(4001, "kicked") });
    expect(err(await api.send(conn, text("late")))).toEqual(new WsError.Closed(4001, "kicked"));
  });

  it("a drop without a close frame ends the stream with Network", async () => {
    const { conn } = ok(await api.connect(`${server.wsUrl}/ws/drop`));
    const { messages, end } = await readToEnd(api, conn);
    expect(messages).toEqual([text("hello")]);
    expect(end).toBeInstanceOf(WsError.Network);
  });

  it(`a text frame that is not UTF-8 ends the stream with ${badUtf8 === "protocol" ? "Protocol" : "Network (the platform reports it as a drop)"}`, async () => {
    const { conn } = ok(await api.connect(`${server.wsUrl}/ws/bad-utf8`));
    const { messages, end } = await readToEnd(api, conn);
    expect(messages).toEqual([]);
    expect(end).toBeInstanceOf(badUtf8 === "protocol" ? WsError.Protocol : WsError.Network);
  });

  it(`a connection the core abandons is closed going away when the core shuts down (the server sees ${goingAway})`, async () => {
    ok(await api.connect(`${server.wsUrl}/ws/stall`));
    port.dispose?.();
    const seen = await until("the server to see the client leave", () => {
      const c = lastOn(server, "/ws/stall");
      return c?.closeCode !== null ? c : undefined;
    }, 1000);
    expect(seen.closeCode).toBe(goingAway);
  });
});

describe("a flood under a stalled reader", () => {
  it("nodeWebSocket stops reading: the server's writes stall far below 2000, then everything arrives in order", async () => {
    const api = wsCalls(webSocketPort(nodeWebSocket()));
    const { conn } = ok(await api.connect(`${server.wsUrl}/ws/flood?n=2000&size=65536`));
    await until("the server to start writing", () => (lastOn(server, "/ws/flood")?.written ?? 0) > 0);
    await new Promise((resolve) => setTimeout(resolve, 500));
    const stalled = lastOn(server, "/ws/flood")?.written ?? 0;
    expect(stalled, "TCP pushed back on the server").toBeLessThan(500);
    const { messages, end } = await readToEnd(api, conn);
    expect(messages).toHaveLength(2000);
    messages.forEach((m, i) => {
      expect(m.kind).toBe("text");
      expect((m as { value: string }).value.startsWith(`${i}.`)).toBe(true);
    });
    expect(end).toEqual(new WsError.Closed(1000, "end"));
  }, 30_000);

  it("browserWebSocket cannot pause: it gives up past 16 MiB and the stream ends Closed(1008) after what it held", async () => {
    const api = wsCalls(webSocketPort(browserWebSocket()));
    const { conn } = ok(await api.connect(`${server.wsUrl}/ws/flood?n=2000&size=65536`));
    await until("the client to give up", () => lastOn(server, "/ws/flood")?.clientClosed, 10_000);
    const { messages, end } = await readToEnd(api, conn);
    expect(end).toEqual(new WsError.Closed(1008, DID_NOT_KEEP_UP));
    expect(messages.length).toBeGreaterThan(200);
    expect(messages.length).toBeLessThan(400);
    messages.forEach((m, i) => {
      expect((m as { value: string }).value.startsWith(`${i}.`)).toBe(true);
    });
  }, 30_000);
});

/** A scripted WHATWG WebSocket: the test fires its events. */
class FakeSocket implements PlatformWebSocket {
  static last: FakeSocket | null = null;
  readonly listeners = new Map<string, Array<(event: Event) => void>>();
  readonly closes: Array<[number | undefined, string | undefined]> = [];
  readonly sent: unknown[] = [];
  protocol = "";
  bufferedAmount = 0;
  readyState = 0;
  binaryType = "blob";
  constructor(
    readonly url: string,
    readonly init?: unknown,
    readonly options?: unknown,
  ) {
    FakeSocket.last = this;
  }
  send(data: string | ArrayBufferView): void {
    this.sent.push(data);
  }
  close(code?: number, reason?: string): void {
    if (code !== undefined && code !== 1000 && (code < 3000 || code > 4999)) throw new DOMException("invalid code", "InvalidAccessError");
    this.closes.push([code, reason]);
  }
  addEventListener(type: string, listener: (event: Event) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  removeEventListener(): void {}
  fire(type: string, event: object = {}): void {
    if (type === "open") this.readyState = 1;
    for (const listener of this.listeners.get(type) ?? []) listener(event as Event);
  }
}

describe("browserWebSocket, scripted", () => {
  const Socket = FakeSocket as unknown as WebSocketConstructorLike;

  it("refuses headers where the platform cannot send them, and passes them where it can", async () => {
    const refusing = browserWebSocket({ WebSocket: Socket, headers: "refuse" });
    await expect(refusing.connect("ws://x.test/", [], [{ name: "Authorization", value: "t" }])).rejects.toEqual(new WsError.Refused(null, HEADERS_REFUSED));
    const passing = browserWebSocket({ WebSocket: Socket, headers: "pass" });
    const opening = passing.connect("ws://x.test/", ["v1"], [{ name: "A", value: "1" }, { name: "A", value: "2" }]);
    const socket = FakeSocket.last as FakeSocket;
    expect(socket.init, "Node's form: protocols and headers in the second argument").toEqual({ protocols: ["v1"], headers: { A: "1, 2" } });
    socket.fire("open");
    await opening;
  });

  it("an error before open is Refused with no status", async () => {
    const opening = browserWebSocket({ WebSocket: Socket }).connect("ws://x.test/", [], []);
    const socket = FakeSocket.last as FakeSocket;
    socket.fire("error", { message: "" });
    socket.fire("close", { code: 1006, reason: "" });
    await expect(opening).rejects.toEqual(new WsError.Refused(null, "the WebSocket could not connect"));
  });

  it("past maxBufferedMessages it closes (without a code where 1008 is refused) and ends Closed(1008) after what it held", async () => {
    const opening = browserWebSocket({ WebSocket: Socket, maxBufferedMessages: 3 }).connect("ws://x.test/", [], []);
    const socket = FakeSocket.last as FakeSocket;
    socket.fire("open");
    const connection = await opening;
    expect(socket.binaryType).toBe("arraybuffer");
    for (const data of ["a", "b", new Uint8Array([7]).buffer, "d", "e"]) socket.fire("message", { data });
    expect(socket.closes, "1008 is refused to scripts, so the close has no code").toEqual([[undefined, undefined]]);
    const iterator = connection.messages()[Symbol.asyncIterator]();
    expect((await iterator.next()).value).toEqual(text("a"));
    expect((await iterator.next()).value).toEqual(text("b"));
    expect((await iterator.next()).value).toEqual({ kind: "binary", value: Uint8Array.of(7) });
    await expect(iterator.next()).rejects.toEqual(new WsError.Closed(1008, DID_NOT_KEEP_UP));
  });

  it("a close event is Closed(code, reason), but 1006 (no close frame) is Network; send waits while more than 1 MiB is queued", async () => {
    const opening = browserWebSocket({ WebSocket: Socket }).connect("ws://x.test/", [], []);
    const socket = FakeSocket.last as FakeSocket;
    socket.fire("open");
    const connection = await opening;
    socket.bufferedAmount = 2 << 20;
    let sent = false;
    const sending = connection.send(text("x")).then(() => {
      sent = true;
    });
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(sent, "still over 1 MiB").toBe(false);
    socket.bufferedAmount = 0;
    await sending;
    expect(socket.sent).toEqual(["x"]);
    socket.fire("close", { code: 1006, reason: "" });
    await expect(connection.messages()[Symbol.asyncIterator]().next()).rejects.toBeInstanceOf(WsError.Network);
  });
});

// ---------------------------------------------------------------------------------------------
// fetchSse
// ---------------------------------------------------------------------------------------------

/** Reads events until the stream ends; returns them with the end. */
async function readEvents(api: ReturnType<typeof sseCalls>, stream: number, max = Number.MAX_SAFE_INTEGER): Promise<{ events: SseEvent[]; end: SseError | null }> {
  const events: SseEvent[] = [];
  while (events.length < max) {
    const got = await api.next(stream, Math.min(16, max - events.length));
    if ("err" in got) return { events, end: got.err };
    if (got.ok.length === 0) break;
    events.push(...got.ok);
  }
  return { events, end: null };
}

const FEED: SseEvent[] = [
  { id: "1", event: "message", data: "one", retryMs: 1500 },
  { id: "2", event: "tick", data: "two\nlines", retryMs: null },
  { id: "2", event: "message", data: "three", retryMs: null },
  { id: "4", event: "message", data: "four", retryMs: null },
];

describe("fetchSse against the realtime server", () => {
  let api: ReturnType<typeof sseCalls>;
  beforeEach(() => {
    api = sseCalls(ssePort(fetchSse()));
  });

  it("parses the feed and ends with Ended; the request asked for an event stream, without a Last-Event-ID", async () => {
    const stream = ok(await api.open(`${server.url}/sse/feed`, [{ name: "X-Token", value: "t" }]));
    expect(await readEvents(api, stream)).toEqual({ events: FEED, end: new SseError.Ended() });
    const seen = lastOn(server, "/sse/feed");
    expect(seen?.headers).toMatchObject({ accept: "text/event-stream", "cache-control": "no-cache", "x-token": "t" });
    expect(seen?.headers["last-event-id"]).toBeUndefined();
  });

  it("resumes after a Last-Event-ID, keeping that id for events without one", async () => {
    const stream = ok(await api.open(`${server.url}/sse/feed`, [], "2"));
    expect(await readEvents(api, stream)).toEqual({ events: FEED.slice(2), end: new SseError.Ended() });
    expect(lastOn(server, "/sse/feed")?.headers["last-event-id"]).toBe("2");
  });

  it("refuses 204 and 500 with their status, and another content type with Protocol", async () => {
    for (const code of [204, 500]) {
      const refused = err(await api.open(`${server.url}/sse/status?code=${code}`));
      expect(refused).toBeInstanceOf(SseError.Refused);
      expect((refused as SseError.Refused).status).toBe(code);
    }
    expect(err(await api.open(`${server.url}/sse/html`))).toEqual(new SseError.Protocol("expected text/event-stream, got text/html"));
  });

  it("a stream the core closes is released: the server sees the client leave", async () => {
    const stream = ok(await api.open(`${server.url}/sse/hang`));
    const waiting = api.next(stream, 16);
    ok(await api.close(stream));
    expect(await waiting).toEqual({ ok: [] });
    await until("the server to see the client leave", () => lastOn(server, "/sse/hang")?.clientClosed, 1000);
  });

  it("reads the body only when pulled: a stalled reader stalls the server's writes", async () => {
    const stream = ok(await api.open(`${server.url}/sse/flood?n=2000&size=65536`));
    await until("the server to start writing", () => (lastOn(server, "/sse/flood")?.written ?? 0) > 0);
    await new Promise((resolve) => setTimeout(resolve, 500));
    expect(lastOn(server, "/sse/flood")?.written ?? 0).toBeLessThan(500);
    const { events, end } = await readEvents(api, stream);
    expect(events).toHaveLength(2000);
    expect(events.every((e, i) => e.id === String(i))).toBe(true);
    expect(end).toEqual(new SseError.Ended());
  }, 30_000);

  it("a request that gets no answer is Network", async () => {
    const closed = await startRealtimeServer();
    const url = closed.url;
    await stopRealtimeServer(closed);
    expect(err(await api.open(`${url}/sse/feed`))).toBeInstanceOf(SseError.Network);
  });
});

describe("fetchSse, scripted", () => {
  const answer = (chunks: Uint8Array[], init: ResponseInit = { headers: { "content-type": "text/event-stream; charset=utf-8" } }, fail?: Error) =>
    fetchSse({
      fetch: async () =>
        new Response(
          new ReadableStream<Uint8Array>({
            start(controller) {
              for (const chunk of chunks) controller.enqueue(chunk);
              if (fail === undefined) controller.close();
              else controller.error(fail);
            },
          }),
          init,
        ),
    });
  const utf8 = (s: string) => new TextEncoder().encode(s);

  it("decodes UTF-8 split anywhere and skips a byte order mark", async () => {
    const whole = utf8("﻿data: é😀\n\n");
    const api = sseCalls(ssePort(answer([...whole].map((b) => Uint8Array.of(b)))));
    const stream = ok(await api.open("https://x.test/"));
    expect(await readEvents(api, stream)).toEqual({ events: [{ id: null, event: "message", data: "é😀", retryMs: null }], end: new SseError.Ended() });
  });

  it("bytes that are not UTF-8 end the stream with Protocol, a failing body with Network", async () => {
    const bad = sseCalls(ssePort(answer([utf8("data: a\n\n"), Uint8Array.of(0x64, 0xff, 0x0a, 0x0a)])));
    const s1 = ok(await bad.open("https://x.test/"));
    expect(await readEvents(bad, s1)).toEqual({ events: [{ id: null, event: "message", data: "a", retryMs: null }], end: new SseError.Protocol("the event stream is not UTF-8") });
    const broken = sseCalls(ssePort(answer([utf8("data: a\n\n")], undefined, new Error("connection reset"))));
    const s2 = ok(await broken.open("https://x.test/"));
    const { end } = await readEvents(broken, s2);
    expect(end).toEqual(new SseError.Network("connection reset"));
  });

  it("an answer without a content type is Protocol", async () => {
    const api = sseCalls(ssePort(answer([], { headers: {} })));
    expect(err(await api.open("https://x.test/"))).toEqual(new SseError.Protocol("expected text/event-stream, got no content type"));
  });
});
