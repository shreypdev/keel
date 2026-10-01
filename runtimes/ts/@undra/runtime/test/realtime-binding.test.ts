import { describe, expect, it } from "vitest";
import { HeaderCodec, WsOpenedCodec } from "../src/adapters/codecs.js";
import { PortIds } from "../src/adapters/ids.js";
import { type Header, SseError, type SseEvent, WsError, type WsMessage } from "../src/adapters/types.js";
import { UndraCore } from "../src/core.js";
import { type SseAdapter, type SseStream, type WebSocketAdapter, type WebSocketConnection, ssePort, webSocketPort } from "../src/realtime.js";
import { PortStatus, codecs, decodeValue } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, track } from "./support/harness.js";
import { args, sseCalls as sse, text, wsCalls as ws } from "./support/port-calls.js";
import { ScriptedSource, settle } from "./support/scripted-source.js";

/*
 * The bindings of `@undra/runtime/realtime` (the brief's "WebSocket binding" and "Sse binding"),
 * driven through their PortImpl methods with the exact argument bytes the core sends, over
 * scripted adapters: ids, the read-ahead window, one pending pull, the close and terminal rules,
 * unknown ids, and the 1001 close when the core goes away.
 */

const headerList = codecs.vec(HeaderCodec);
const stringList = codecs.vec(codecs.string);

const texts = (from: number, to: number): WsMessage[] => Array.from({ length: to - from }, (_, i) => text(String(from + i)));

// ---------------------------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------------------------

class ScriptedConnection implements WebSocketConnection {
  readonly source = new ScriptedSource<WsMessage>();
  readonly sent: WsMessage[] = [];
  readonly closes: Array<[number, string]> = [];
  sendFailure: unknown = null;
  constructor(readonly protocol: string) {}
  messages(): AsyncIterable<WsMessage> {
    return this.source;
  }
  async send(message: WsMessage): Promise<void> {
    if (this.sendFailure !== null) throw this.sendFailure;
    this.sent.push(message);
  }
  async close(code: number, reason: string): Promise<void> {
    this.closes.push([code, reason]);
    this.source.finish();
  }
}

class ScriptedWebSocket implements WebSocketAdapter {
  readonly connections: ScriptedConnection[] = [];
  readonly asked: Array<{ url: string; protocols: readonly string[]; headers: readonly Header[] }> = [];
  refusal: unknown = null;
  async connect(url: string, protocols: readonly string[], headers: readonly Header[]): Promise<WebSocketConnection> {
    this.asked.push({ url, protocols, headers });
    if (this.refusal !== null) throw this.refusal;
    const connection = new ScriptedConnection(protocols[0] ?? "");
    this.connections.push(connection);
    return connection;
  }
}

async function opened(adapter = new ScriptedWebSocket()) {
  const port = webSocketPort(adapter);
  const api = ws(port);
  const first = await api.connect("ws://chat.test/a", ["v2", "v1"], [{ name: "X-Token", value: "t" }]);
  if (!("ok" in first)) throw new Error("connect failed");
  return { adapter, port, api, conn: first.ok.conn, connection: adapter.connections[0] as ScriptedConnection };
}

describe("webSocketPort: connect", () => {
  it("asks the adapter, numbers connections from 1 and never reuses an id", async () => {
    const adapter = new ScriptedWebSocket();
    const api = ws(webSocketPort(adapter));
    expect(await api.connect("ws://chat.test/a", ["v2", "v1"], [{ name: "X-Token", value: "t" }])).toEqual({ ok: { conn: 1, protocol: "v2" } });
    expect(adapter.asked[0]).toEqual({ url: "ws://chat.test/a", protocols: ["v2", "v1"], headers: [{ name: "X-Token", value: "t" }] });
    await api.close(1, 1000, "");
    expect(await api.connect("wss://chat.test/b")).toEqual({ ok: { conn: 2, protocol: "" } });
  });

  it("refuses a URL that is not ws:// or wss:// before the adapter is asked", async () => {
    const adapter = new ScriptedWebSocket();
    const api = ws(webSocketPort(adapter));
    expect(await api.connect("https://chat.test/")).toEqual({ err: new WsError.Refused(null, "invalid URL: https://chat.test/") });
    expect(adapter.asked).toEqual([]);
  });

  it("passes a typed refusal through and turns anything else into Network", async () => {
    const adapter = new ScriptedWebSocket();
    const api = ws(webSocketPort(adapter));
    adapter.refusal = new WsError.Refused(401, "unauthorized");
    expect(await api.connect("ws://x.test/")).toEqual({ err: new WsError.Refused(401, "unauthorized") });
    adapter.refusal = new TypeError("boom");
    expect(await api.connect("ws://x.test/")).toEqual({ err: new WsError.Network("boom") });
  });
});

describe("webSocketPort: the pull is the credit", () => {
  it("reads ahead 16 before the first receive, then only up to the latest max", async () => {
    const { api, conn, connection } = await opened();
    connection.source.push(...texts(0, 100));
    await settle();
    expect(connection.source.requested, "16 read ahead before the first pull").toBe(16);
    expect(await api.receive(conn, 5)).toEqual({ ok: texts(0, 5) });
    await settle();
    expect(connection.source.requested, "11 still buffered, more than the window of 5: nothing read").toBe(16);
    // 11 buffered, fewer than 20 while more arrive: the pull waits for the burst and answers it whole.
    expect(await api.receive(conn, 20)).toEqual({ ok: texts(5, 25) });
    await settle();
    expect(connection.source.requested, "the window is 20 now: 20 read ahead again").toBe(45);
    expect(await api.receive(conn, 20)).toEqual({ ok: texts(25, 45) });
  });

  it("answers a waiting receive with a lone message after 2 ms of quiet", async () => {
    const { api, conn, connection } = await opened();
    const waiting = api.receive(conn, 16);
    await settle();
    const pushed = performance.now();
    connection.source.push(text("late"));
    expect(await waiting).toEqual({ ok: [text("late")] });
    expect(performance.now() - pushed).toBeLessThan(100);
  });

  it("answers a burst that trickles in as one reply: up to max, or 8 ms after its first message", async () => {
    const { api, conn, connection } = await opened();
    const waiting = api.receive(conn, 16);
    await settle();
    // One message a millisecond: never 2 ms of quiet, so the 8 ms linger answers what came by then.
    let sent = 0;
    const timer = setInterval(() => connection.source.push(text(String(sent++))), 1);
    const got = await waiting;
    clearInterval(timer);
    const first = "ok" in got ? got.ok : [];
    expect(first.length, "more than one message per reply").toBeGreaterThan(1);
    expect(first).toEqual(texts(0, first.length));
    connection.source.push(...texts(sent, sent + 40));
    expect(await api.receive(conn, 16), "a full burst answers at max").toMatchObject({ ok: { length: 16 } });
  });

  it("allows one waiting receive per connection", async () => {
    const { api, conn, connection } = await opened();
    const first = api.receive(conn, 16);
    expect(await api.receive(conn, 16)).toEqual({ err: new WsError.Protocol(`a receive is already pending on connection ${conn}`) });
    connection.source.push(text("a"));
    expect(await first).toEqual({ ok: [text("a")] });
  });
});

describe("webSocketPort: how a connection ends", () => {
  it("delivers what arrived, then the adapter's typed end, sticky; send gets the end too", async () => {
    const { api, conn, connection } = await opened();
    connection.source.push(text("a"), text("b"));
    connection.source.fail(new WsError.Closed(4000, "bye"));
    await settle();
    expect(await api.receive(conn, 16)).toEqual({ ok: [text("a"), text("b")] });
    expect(await api.receive(conn, 16)).toEqual({ err: new WsError.Closed(4000, "bye") });
    expect(await api.receive(conn, 16)).toEqual({ err: new WsError.Closed(4000, "bye") });
    expect(await api.send(conn, text("x"))).toEqual({ err: new WsError.Closed(4000, "bye") });
  });

  it("an iterable that just finishes is Network(\"the connection ended\"); anything thrown is Network(text)", async () => {
    const a = await opened();
    a.connection.source.finish();
    expect(await a.api.receive(a.conn, 16)).toEqual({ err: new WsError.Network("the connection ended") });
    const b = await opened();
    b.connection.source.fail(new RangeError("socket hang up"));
    expect(await b.api.receive(b.conn, 16)).toEqual({ err: new WsError.Network("socket hang up") });
  });

  it("the core's close answers a waiting receive with [], drops the buffer, and later sends get its code", async () => {
    const { api, conn, connection } = await opened();
    const waiting = api.receive(conn, 16);
    await settle();
    expect(await api.close(conn, 4000, "bye")).toEqual({ ok: undefined });
    expect(await waiting).toEqual({ ok: [] });
    expect(connection.closes).toEqual([[4000, "bye"]]);
    connection.source.push(text("after"));
    expect(await api.receive(conn, 16)).toEqual({ ok: [] });
    expect(await api.send(conn, text("x"))).toEqual({ err: new WsError.Closed(4000, "bye") });
    expect(await api.close(conn, 1000, "again"), "closing twice is not an error").toEqual({ ok: undefined });
    expect(connection.closes, "the adapter is closed once").toEqual([[4000, "bye"]]);
  });

  it("send waits for the adapter and reports its typed failure", async () => {
    const { api, conn, connection } = await opened();
    expect(await api.send(conn, { kind: "binary", value: Uint8Array.of(1, 2) })).toEqual({ ok: undefined });
    expect(connection.sent).toEqual([{ kind: "binary", value: Uint8Array.of(1, 2) }]);
    connection.sendFailure = new WsError.Network("reset");
    expect(await api.send(conn, text("x"))).toEqual({ err: new WsError.Network("reset") });
  });

  it("an unknown connection is Network(\"no WebSocket connection <id>\") for every method", async () => {
    const { api } = await opened();
    const unknown = { err: new WsError.Network("no WebSocket connection 99") };
    expect(await api.receive(99, 1)).toEqual(unknown);
    expect(await api.send(99, text("x"))).toEqual(unknown);
    expect(await api.close(99, 1000, "")).toEqual(unknown);
  });
});

describe("webSocketPort: the core goes away", () => {
  it("dispose closes every open connection with 1001 and answers waiting receives with []", async () => {
    const adapter = new ScriptedWebSocket();
    const port = webSocketPort(adapter);
    const api = ws(port);
    await api.connect("ws://a.test/");
    await api.connect("ws://b.test/");
    await api.close(2, 1000, "done");
    const waiting = api.receive(1, 16);
    await settle();
    port.dispose?.();
    expect(await waiting).toEqual({ ok: [] });
    expect(adapter.connections.map((c) => c.closes)).toEqual([[[1001, ""]], [[1000, "done"]]]);
    expect(await api.send(1, text("x"))).toEqual({ err: new WsError.Closed(1001, "") });
  });

  it("UndraCore.close disposes its ports, and registerPort disposes the port it replaces", async () => {
    const fake = new FakeCoreTransport({ mode: "remote" });
    const adapter = new ScriptedWebSocket();
    const port = webSocketPort(adapter);
    const core = track(
      await UndraCore.attach(fake, {
        expectedSchemaHash: SCHEMA,
        shared: false,
        adapters: { log: captureLog(), http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle: null },
        ports: { [PortIds.WebSocket.portId]: port },
      }),
    );
    const opened = await fake.callPort(
      PortIds.WebSocket.portId,
      PortIds.WebSocket.connect,
      args((w) => {
        w.writeStr("ws://core.test/");
        stringList.encode(w, []);
        headerList.encode(w, []);
      }),
    );
    expect(opened.status).toBe(PortStatus.Ok);
    expect(decodeValue(WsOpenedCodec, opened.body)).toEqual({ conn: 1, protocol: "" });
    core.registerPort(PortIds.WebSocket.portId, port);
    expect(adapter.connections[0]?.closes, "re-registering the same port keeps it").toEqual([]);
    const second = webSocketPort(adapter);
    core.registerPort(PortIds.WebSocket.portId, second);
    expect(adapter.connections[0]?.closes, "the replaced port closed its connection").toEqual([[1001, ""]]);
    await fake.callPort(
      PortIds.WebSocket.portId,
      PortIds.WebSocket.connect,
      args((w) => {
        w.writeStr("ws://core.test/2");
        stringList.encode(w, []);
        headerList.encode(w, []);
      }),
    );
    core.close();
    expect(adapter.connections[1]?.closes, "closing the core closed the live connection").toEqual([[1001, ""]]);
  });
});

// ---------------------------------------------------------------------------------------------
// Sse
// ---------------------------------------------------------------------------------------------

class ScriptedStream implements SseStream {
  readonly source = new ScriptedSource<SseEvent>();
  closed = 0;
  events(): AsyncIterable<SseEvent> {
    return this.source;
  }
  async close(): Promise<void> {
    this.closed++;
    this.source.finish();
  }
}

class ScriptedSse implements SseAdapter {
  readonly streams: ScriptedStream[] = [];
  readonly asked: Array<{ url: string; headers: readonly Header[]; lastEventId: string | null }> = [];
  refusal: unknown = null;
  async open(url: string, headers: readonly Header[], lastEventId: string | null): Promise<SseStream> {
    this.asked.push({ url, headers, lastEventId });
    if (this.refusal !== null) throw this.refusal;
    const stream = new ScriptedStream();
    this.streams.push(stream);
    return stream;
  }
}

const event = (data: string, id: string | null = null): SseEvent => ({ id, event: "message", data, retryMs: null });

describe("ssePort", () => {
  it("opens with the headers and the last event id, numbering streams from 1", async () => {
    const adapter = new ScriptedSse();
    const api = sse(ssePort(adapter));
    expect(await api.open("https://feed.test/", [{ name: "Authorization", value: "Bearer t" }], "7")).toEqual({ ok: 1 });
    expect(adapter.asked).toEqual([{ url: "https://feed.test/", headers: [{ name: "Authorization", value: "Bearer t" }], lastEventId: "7" }]);
    expect(await api.open("http://feed.test/")).toEqual({ ok: 2 });
    expect(await api.open("ws://feed.test/")).toEqual({ err: new SseError.Refused(null, "invalid URL: ws://feed.test/") });
    adapter.refusal = new SseError.Refused(500, "the server answered 500");
    expect(await api.open("https://feed.test/")).toEqual({ err: new SseError.Refused(500, "the server answered 500") });
  });

  it("reads ahead up to the window, answers up to max, and ends with Ended when the iterable finishes", async () => {
    const adapter = new ScriptedSse();
    const api = sse(ssePort(adapter));
    await api.open("https://feed.test/");
    const stream = adapter.streams[0] as ScriptedStream;
    stream.source.push(...Array.from({ length: 40 }, (_, i) => event(String(i), String(i))));
    await settle();
    expect(stream.source.requested).toBe(16);
    const first = await api.next(1, 3);
    expect(first).toEqual({ ok: [event("0", "0"), event("1", "1"), event("2", "2")] });
    // 13 buffered and more arriving: the pull reads on until nothing new comes (2 ms), then answers all 37.
    const rest = await api.next(1, 100);
    expect("ok" in rest && rest.ok.map((e) => e.data)).toEqual(Array.from({ length: 37 }, (_, i) => String(3 + i)));
    stream.source.finish();
    expect(await api.next(1, 100)).toEqual({ err: new SseError.Ended() });
    expect(await api.next(1, 100), "the end is sticky").toEqual({ err: new SseError.Ended() });
  });

  it("one next at a time; close answers it with []; unknown streams are Network; dispose closes the rest", async () => {
    const adapter = new ScriptedSse();
    const port = ssePort(adapter);
    const api = sse(port);
    await api.open("https://a.test/");
    await api.open("https://b.test/");
    const waiting = api.next(1, 16);
    expect(await api.next(1, 16)).toEqual({ err: new SseError.Protocol("a next is already pending on stream 1") });
    expect(await api.close(1)).toEqual({ ok: undefined });
    expect(await waiting).toEqual({ ok: [] });
    expect(await api.close(1)).toEqual({ ok: undefined });
    expect(adapter.streams[0]?.closed).toBe(1);
    expect(await api.next(9, 1)).toEqual({ err: new SseError.Network("no event stream 9") });
    port.dispose?.();
    expect(adapter.streams[1]?.closed).toBe(1);
    expect(await api.next(2, 1)).toEqual({ ok: [] });
  });

  it("an error thrown by the stream reaches the core typed (Network for anything that is not an SseError)", async () => {
    const adapter = new ScriptedSse();
    const api = sse(ssePort(adapter));
    await api.open("https://a.test/");
    await api.open("https://b.test/");
    adapter.streams[0]?.source.fail(new SseError.Protocol("the event stream is not UTF-8"));
    adapter.streams[1]?.source.fail(new Error("socket closed"));
    expect(await api.next(1, 16)).toEqual({ err: new SseError.Protocol("the event stream is not UTF-8") });
    expect(await api.next(2, 16)).toEqual({ err: new SseError.Network("socket closed") });
  });
});
