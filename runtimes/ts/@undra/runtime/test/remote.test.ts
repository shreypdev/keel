import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  type ConnectionState,
  UndraCore,
  UndraSchemaMismatchError,
  UndraSessionLostError,
  UndraTransportError,
} from "../src/index.js";
import { RemoteTransport, reconnectDelayMs } from "../src/transport/remote.js";
import type { TransportHandler } from "../src/transport/transport.js";
import {
  ALL_SIGNALS,
  CallTarget,
  ChangeOp,
  type HelloPayload,
  Kind,
  ReplyStatus,
  StreamFlag,
  decodeCall,
  decodeObserve,
  decodeRelease,
  encodeStreamItem,
  encodeReply,
} from "../src/wire/index.js";
import { SCHEMA } from "./support/fake-core.js";
import { FakeServer } from "./support/fake-websocket.js";
import { captureLog, track } from "./support/harness.js";
import { CounterStore, u32 } from "./support/store.js";

const URL = "ws://127.0.0.1:7443";
const FREE = { target: CallTarget.FreeFunction } as const;

/** Lets the microtasks the fake sockets use run, without moving the clock. */
async function settle(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

/** What a transport tells its handler, in order. */
class Recorder implements TransportHandler {
  readonly events: string[] = [];
  readonly errors: Error[] = [];
  readonly hellos: HelloPayload[] = [];
  holds = false;
  reply = (): void => {};
  changeSet = (): void => {};
  streamItem = (): void => {};
  portCall = (): { kind: "unavailable" } => ({ kind: "unavailable" });
  log = (): void => {};
  closed = (error: Error): void => {
    this.events.push("closed");
    this.errors.push(error);
  };
  reconnecting = (attempt: number, error: Error): void => {
    this.events.push(`reconnecting ${attempt}`);
    this.errors.push(error);
  };
  reconnected = (hello: HelloPayload): void => {
    this.events.push("reconnected");
    this.hellos.push(hello);
  };
  holdsObjects = (): boolean => this.holds;
}

/** A transport started against a fake server, with the jitter fixed so the schedule is exact. */
async function started(
  server: FakeServer,
  options: Partial<ConstructorParameters<typeof RemoteTransport>[0]> = {},
): Promise<{ transport: RemoteTransport; handler: Recorder }> {
  const transport = new RemoteTransport({
    url: URL,
    expectedSchemaHash: SCHEMA,
    webSocket: server.Socket,
    reconnect: { random: () => 0 },
    session: "tok-test",
    ...options,
  });
  const handler = new Recorder();
  const hello = transport.start(handler);
  await vi.advanceTimersByTimeAsync(0);
  await hello;
  return { transport, handler };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("the reconnect schedule", () => {
  it("doubles from 250 ms to a cap of 5 s", () => {
    const longest = { random: () => 0 };
    expect([1, 2, 3, 4, 5, 6, 7, 20].map((n) => reconnectDelayMs(n, longest))).toEqual([250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
  });

  it("takes up to half off for jitter, never more", () => {
    const shortest = { random: () => 0.999999 };
    expect([1, 2, 3, 4, 5, 6, 20].map((n) => reconnectDelayMs(n, shortest))).toEqual([125, 250, 500, 1000, 2000, 2500, 2500]);
    for (let n = 1; n < 12; n++) {
      const waited = reconnectDelayMs(n);
      const longest = Math.min(5000, 250 * 2 ** (n - 1));
      expect(waited).toBeGreaterThanOrEqual(Math.floor(longest / 2));
      expect(waited).toBeLessThanOrEqual(longest);
    }
  });

  it("follows the options", () => {
    const tuned = { initialDelayMs: 100, maxDelayMs: 300, jitter: 0, random: () => 0.7 };
    expect([1, 2, 3, 4].map((n) => reconnectDelayMs(n, tuned))).toEqual([100, 200, 300, 300]);
    expect(reconnectDelayMs(1, { jitter: 1, random: () => 0.5 })).toBe(125);
  });
});

describe("RemoteTransport reconnecting", () => {
  it("announces a lost connection at once and retries after the first backoff, not before", async () => {
    const server = new FakeServer();
    const { handler } = await started(server);
    expect(server.sockets).toHaveLength(1);

    server.current.serverClose(1006);
    expect(handler.events).toEqual(["reconnecting 1"]);
    expect(handler.errors[0]).toMatchObject({ reason: "closed" });

    await vi.advanceTimersByTimeAsync(249);
    expect(server.sockets, "no attempt before 250 ms").toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(server.sockets).toHaveLength(2);
    await settle();
    expect(handler.events).toEqual(["reconnecting 1", "reconnected"]);
    expect(handler.hellos[0]?.schemaHash).toBe(SCHEMA);
  });

  it("backs off 250, 500, 1000, 2000, 4000, 5000, 5000 ms while the server is down, then heals", async () => {
    const server = new FakeServer();
    const { handler } = await started(server);
    server.behaviour = "refuse";
    server.current.serverClose(1006);

    const attempts: number[] = [];
    let last = server.sockets.length;
    let clock = 0;
    // One millisecond at a time is slow; step by the schedule and check each wait is exact.
    for (const wait of [250, 500, 1000, 2000, 4000, 5000, 5000]) {
      await vi.advanceTimersByTimeAsync(wait - 1);
      expect(server.sockets.length, `no attempt ${wait - 1} ms into a ${wait} ms wait`).toBe(last);
      await vi.advanceTimersByTimeAsync(1);
      await settle();
      expect(server.sockets.length, `an attempt after ${wait} ms`).toBe(last + 1);
      last = server.sockets.length;
      clock += wait;
      attempts.push(clock);
    }
    expect(handler.events.filter((e) => e.startsWith("reconnecting"))).toEqual([
      "reconnecting 1",
      "reconnecting 2",
      "reconnecting 3",
      "reconnecting 4",
      "reconnecting 5",
      "reconnecting 6",
      "reconnecting 7",
      "reconnecting 8",
    ]);

    server.behaviour = "answer";
    await vi.advanceTimersByTimeAsync(5000);
    await settle();
    expect(handler.events.at(-1)).toBe("reconnected");
    expect(handler.events).not.toContain("closed");
  });

  it("puts the same session token on every connection and asks to resume only when objects are held", async () => {
    const server = new FakeServer();
    const { handler } = await started(server);
    expect(server.sockets[0]?.query.get("undra_session")).toBe("tok-test");
    expect(server.sockets[0]?.query.has("undra_resume")).toBe(false);

    handler.holds = false;
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(server.sockets[1]?.query.get("undra_session")).toBe("tok-test");
    expect(server.sockets[1]?.query.has("undra_resume"), "nothing to resume").toBe(false);

    handler.holds = true;
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(server.sockets[2]?.query.get("undra_session")).toBe("tok-test");
    expect(server.sockets[2]?.query.get("undra_resume")).toBe("1");
  });

  it("keeps the rest of the URL, and sends no token when session is off", async () => {
    const server = new FakeServer();
    await started(server, { url: `${URL}/dev?x=1` });
    expect(server.sockets[0]?.url).toBe(`ws://127.0.0.1:7443/dev?x=1&undra_session=tok-test`);

    const quiet = new FakeServer();
    await started(quiet, { session: false });
    expect(quiet.sockets[0]?.url, "untouched").toBe("ws://127.0.0.1:7443");
    expect(quiet.sockets[0]?.query.has("undra_session")).toBe(false);
  });

  it("generates a token of its own, the same for every connection of one transport", async () => {
    const server = new FakeServer();
    const transport = new RemoteTransport({ url: URL, expectedSchemaHash: SCHEMA, webSocket: server.Socket, reconnect: { random: () => 0 } });
    const handler = new Recorder();
    const hello = transport.start(handler);
    await vi.advanceTimersByTimeAsync(0);
    await hello;
    const token = server.sockets[0]?.query.get("undra_session") ?? "";
    expect(token).toMatch(/^[0-9a-f]{32}$/);
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    expect(server.sockets[1]?.query.get("undra_session")).toBe(token);
  });

  it("restarts the sequence numbers on every connection", async () => {
    const server = new FakeServer();
    const { transport } = await started(server);
    transport.send(Kind.Cancel, new Uint8Array([1, 0, 0, 0]));
    expect(server.sockets[0]?.envelopes.map((e) => e.seq)).toEqual([0, 1]);
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(server.sockets[1]?.envelopes.map((e) => [e.kind, e.seq])).toEqual([[Kind.Hello, 0]]);
  });

  it("refuses sends while it reconnects, with a message that says why, and works again after", async () => {
    const server = new FakeServer();
    const { transport } = await started(server);
    server.current.serverClose(1006);
    expect(() => {
      transport.send(Kind.Cancel, new Uint8Array([1, 0, 0, 0]));
    }).toThrowError(expect.objectContaining({ reason: "closed", message: expect.stringContaining("reconnecting") }) as Error);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    transport.send(Kind.Cancel, new Uint8Array([1, 0, 0, 0]));
    expect(server.current.sent(Kind.Cancel)).toHaveLength(1);
  });

  it("a core with another schema hash is final: the error comes once and nothing retries", async () => {
    const server = new FakeServer();
    const { handler } = await started(server);
    server.schemaHash = SCHEMA + 1n; // `undra dev` rebuilt the core with another schema
    server.current.serverClose(1001);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(handler.events).toEqual(["reconnecting 1", "closed"]);
    expect(handler.errors[1]).toBeInstanceOf(UndraSchemaMismatchError);
    const sockets = server.sockets.length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(server.sockets, "no loop").toHaveLength(sockets);
    expect(server.current.closedByClient, "the attempt's socket was let go").not.toBeNull();
  });

  it("a session the server lost is final too", async () => {
    const server = new FakeServer();
    const { handler } = await started(server);
    handler.holds = true;
    server.current.serverClose(1001);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    // `undra dev` answered the Hello and closed with 4001.
    server.current.serverClose(4001, "session lost: this core has no session tok-test");
    await settle();
    expect(handler.events.at(-1)).toBe("closed");
    expect(handler.errors.at(-1)).toBeInstanceOf(UndraSessionLostError);
    expect(handler.errors.at(-1)?.message).toContain("session lost");
    const sockets = server.sockets.length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(server.sockets).toHaveLength(sockets);
  });

  it("a text message is a protocol error, which retrying would not fix", async () => {
    const server = new FakeServer();
    const { handler } = await started(server);
    server.current.deliverText("hello");
    expect(handler.events).toEqual(["closed"]);
    expect(handler.errors[0]).toMatchObject({ reason: "protocol" });
  });

  it("an attempt that gets no Hello times out after at most 5 s and is retried", async () => {
    const server = new FakeServer();
    const { handler } = await started(server, { handshakeTimeoutMs: 30_000 });
    server.behaviour = "silent";
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    expect(server.sockets).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(4999);
    expect(handler.events).toEqual(["reconnecting 1"]);
    await vi.advanceTimersByTimeAsync(1);
    expect(handler.events).toEqual(["reconnecting 1", "reconnecting 2"]);
    expect(handler.errors[1]).toMatchObject({ reason: "timeout" });
    expect(server.sockets[1]?.closedByClient, "the silent socket was let go").not.toBeNull();
  });

  it("gives up after maxAttempts and says why", async () => {
    const server = new FakeServer();
    const { handler } = await started(server, { reconnect: { random: () => 0, maxAttempts: 2 } });
    server.behaviour = "refuse";
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    await vi.advanceTimersByTimeAsync(500);
    await settle();
    await vi.advanceTimersByTimeAsync(1000);
    await settle();
    expect(handler.events).toEqual(["reconnecting 1", "reconnecting 2", "closed"]);
    expect(handler.errors.at(-1)).toBeInstanceOf(UndraTransportError);
    expect(server.sockets).toHaveLength(3);
  });

  it("with reconnect off a lost connection is final at once, as it always was", async () => {
    const server = new FakeServer();
    const { handler } = await started(server, { reconnect: false });
    server.current.serverClose(1006);
    expect(handler.events).toEqual(["closed"]);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(server.sockets).toHaveLength(1);
  });

  it("close() during the backoff cancels the retry, and nobody is told", async () => {
    const server = new FakeServer();
    const { transport, handler } = await started(server);
    server.current.serverClose(1006);
    transport.close();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(server.sockets).toHaveLength(1);
    expect(handler.events).toEqual(["reconnecting 1"]);
  });

  it("close() during an attempt lets go of its socket and never reports it", async () => {
    const server = new FakeServer();
    const { transport, handler } = await started(server);
    server.behaviour = "silent";
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    const attempt = server.current;
    transport.close();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(attempt.closedByClient).toMatchObject({ code: 1000 });
    expect(handler.events).toEqual(["reconnecting 1"]);
  });

  it("lets go of the listeners of every socket it leaves", async () => {
    const server = new FakeServer();
    await started(server);
    const first = server.current;
    first.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(first.listenerCount).toBe(0);
  });
});

// ----- the core on top: what an app sees --------------------------------------------------------

/** A core on a fake dev server, its connection changes recorded. */
async function loaded(
  server: FakeServer,
  extra: Partial<Parameters<typeof UndraCore.load>[0]> = {},
): Promise<{ core: UndraCore; states: ConnectionState[]; closes: Error[]; errors: unknown[] }> {
  const states: ConnectionState[] = [];
  const closes: Error[] = [];
  const errors: unknown[] = [];
  const log = captureLog();
  const loading = UndraCore.load({
    mode: "remote",
    url: URL,
    expectedSchemaHash: SCHEMA,
    webSocket: server.Socket,
    reconnect: { random: () => 0 },
    shared: false,
    adapters: { log, http: null, timer: null },
    onClose: (e) => closes.push(e),
    onError: (e) => errors.push(e),
    onConnectionChange: (s) => states.push(s),
    ...extra,
  });
  await vi.advanceTimersByTimeAsync(0);
  const core = track(await loading);
  return { core, states, closes, errors };
}

const kinds = (states: ConnectionState[]): string[] => states.map((s) => (s.kind === "reconnecting" ? `reconnecting ${s.attempt}` : s.kind === "closed" ? `closed:${s.reason}` : s.kind));

describe("UndraCore over a reconnecting remote transport", () => {
  it("shows connecting, then connected, as a signal and through onConnectionChange", async () => {
    const server = new FakeServer();
    const { core, states } = await loaded(server);
    expect(kinds(states)).toEqual(["connecting", "connected"]);
    expect(core.connection.peek()).toEqual({ kind: "connected" });
    const seen: string[] = [];
    core.connection.subscribe((s) => seen.push(s.kind));
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(seen).toEqual(["reconnecting", "connected"]);
  });

  it("a drop fails what is in flight with a typed error, at once, and the core stays open", async () => {
    const server = new FakeServer();
    const { core, states, closes } = await loaded(server);
    const call = core.call(FREE, 1, u32(1));
    const observing = core.observe(7n, ALL_SIGNALS, true);
    const stream = core.stream(FREE, 2, new Uint8Array(0))[Symbol.asyncIterator]();
    const streamNext = stream.next();
    const assertions = [
      expect(call).rejects.toMatchObject({ kind: "transport", reason: "closed", message: expect.stringContaining("reconnecting") }),
      expect(observing).rejects.toMatchObject({ reason: "closed" }),
      expect(streamNext).rejects.toMatchObject({ reason: "closed" }),
    ];
    server.current.serverClose(1006);
    await Promise.all(assertions);
    expect(core.closed).toBe(false);
    expect(closes, "onClose is for a core that is gone for good").toEqual([]);
    expect(kinds(states)).toEqual(["connecting", "connected", "reconnecting 1"]);
    expect((await core.stats()).pendingCalls).toBe(0);
  });

  it("calls made while reconnecting fail at once instead of waiting", async () => {
    const server = new FakeServer();
    const { core } = await loaded(server);
    server.current.serverClose(1006);
    await expect(core.call(FREE, 1, u32(1))).rejects.toMatchObject({ reason: "closed", message: expect.stringContaining("reconnecting") });
    await expect(core.observe(1n, ALL_SIGNALS, true)).rejects.toMatchObject({ reason: "closed" });
    expect(() => {
      core.event(1, 2, new Uint8Array(0));
    }).toThrowError(UndraTransportError);
  });

  it("works again after the reconnect: a call is answered", async () => {
    const server = new FakeServer();
    const { core } = await loaded(server);
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    const call = core.call(FREE, 1, u32(1));
    await settle();
    const sent = server.current.sent(Kind.Call);
    expect(sent).toHaveLength(1);
    const { callId } = decodeCall((sent[0] as { payload: Uint8Array }).payload);
    server.reply(callId, u32(42));
    expect(await call).toEqual(u32(42));
  });

  it("observes every store again after a reconnect, and the mirror resyncs from the answer", async () => {
    const server = new FakeServer();
    const { core, states } = await loaded(server);
    const a = await observedStore(core, server, 0x1_0000_0001n, 5);
    const b = await observedStore(core, server, 0x1_0000_0002n, 6);
    expect([a.count.peek(), b.count.peek()]).toEqual([5, 6]);

    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(kinds(states).slice(-2)).toEqual(["reconnecting 1", "connected"]);

    const observes = server.current.sent(Kind.Observe).map((e) => decodeObserve(e.payload));
    expect(observes).toEqual([
      { handle: 0x1_0000_0001n, signalId: ALL_SIGNALS, on: true },
      { handle: 0x1_0000_0002n, signalId: ALL_SIGNALS, on: true },
    ]);

    // The core answers with the current values (changed while the client was away): the mirrors converge.
    server.changeSet([
      { handle: 0x1_0000_0001n, signalId: 0, op: ChangeOp.FullValue, value: u32(50) },
      { handle: 0x1_0000_0002n, signalId: 0, op: ChangeOp.FullValue, value: u32(60) },
    ]);
    await vi.advanceTimersByTimeAsync(200);
    expect([a.count.peek(), b.count.peek()]).toEqual([50, 60]);
  });

  it("does not observe again what the app stopped observing, or released", async () => {
    const server = new FakeServer();
    const { core } = await loaded(server);
    const kept = await observedStore(core, server, 0x1_0000_0001n, 1);
    const stopped = await observedStore(core, server, 0x1_0000_0002n, 2);
    const released = await observedStore(core, server, 0x1_0000_0003n, 3);
    await core.observe(stopped.handle, ALL_SIGNALS, false);
    released.close();
    expect(kept.closed).toBe(false);

    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(server.current.sent(Kind.Observe).map((e) => decodeObserve(e.payload).handle)).toEqual([0x1_0000_0001n]);
    expect(server.current.sent(Kind.Release), "released before the drop: nothing left to release").toHaveLength(0);
  });

  it("releases at the core, once the connection is back, what the app released while it was down", async () => {
    const server = new FakeServer();
    const { core, errors } = await loaded(server);
    const store = await observedStore(core, server, 0x1_0000_0001n, 1);
    server.current.serverClose(1006);
    store.close();
    expect(errors, "releasing while down is not an error").toEqual([]);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(server.current.sent(Kind.Release).map((e) => decodeRelease(e.payload).handle)).toEqual([store.handle]);
    expect(server.current.sent(Kind.Observe)).toHaveLength(0);
  });

  it("asks to resume only when it holds objects the core made", async () => {
    const server = new FakeServer();
    const { core } = await loaded(server);
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(server.current.query.has("undra_resume"), "it constructed nothing").toBe(false);

    const constructing = core.construct(1, 2, new Uint8Array(0));
    await settle();
    const { callId } = decodeCall((server.current.sent(Kind.Call)[0] as { payload: Uint8Array }).payload);
    server.reply(callId, new Uint8Array([9, 0, 0, 0, 1, 0, 0, 0]));
    await constructing;
    server.current.serverClose(1006);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(server.current.query.get("undra_resume")).toBe("1");
  });

  it("a schema change on reconnect is the mismatch error, once, and the core is closed for good", async () => {
    const server = new FakeServer();
    const { core, states, closes } = await loaded(server);
    server.schemaHash = SCHEMA ^ 0xffn;
    server.current.serverClose(1001);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(closes).toHaveLength(1);
    expect(closes[0]).toBeInstanceOf(UndraSchemaMismatchError);
    expect(closes[0]).toMatchObject({ expected: SCHEMA, got: SCHEMA ^ 0xffn });
    expect(kinds(states)).toEqual(["connecting", "connected", "reconnecting 1", "closed:schemaMismatch"]);
    expect(core.closed).toBe(true);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(closes, "once, not on every retry").toHaveLength(1);
    expect(server.sockets).toHaveLength(2);
    await expect(core.call(FREE, 1, u32(1))).rejects.toBeInstanceOf(UndraTransportError);
  });

  it("a lost session closes the core with its own reason", async () => {
    const server = new FakeServer();
    const { core, states, closes } = await loaded(server);
    server.current.serverClose(1001);
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    server.current.serverClose(4001, "session lost");
    await settle();
    expect(kinds(states).at(-1)).toBe("closed:sessionLost");
    expect(closes[0]).toBeInstanceOf(UndraSessionLostError);
    expect(core.closed).toBe(true);
    const state = core.connection.peek();
    expect(state.kind === "closed" && state.error).toBeInstanceOf(UndraSessionLostError);
  });

  it("closing the core is `closed` with reason requested, and onClose stays quiet", async () => {
    const server = new FakeServer();
    const { core, states, closes } = await loaded(server);
    server.current.serverClose(1006);
    core.close();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(kinds(states)).toEqual(["connecting", "connected", "reconnecting 1", "closed:requested"]);
    expect(closes).toEqual([]);
    expect(server.sockets).toHaveLength(1);
  });

  it("an initial connection that fails still rejects load, as before", async () => {
    const server = new FakeServer();
    server.behaviour = "refuse";
    const loading = UndraCore.load({
      mode: "remote",
      url: URL,
      expectedSchemaHash: SCHEMA,
      webSocket: server.Socket,
      shared: false,
      adapters: { http: null, timer: null },
    });
    const assertion = expect(loading).rejects.toMatchObject({ reason: "handshake" });
    await vi.advanceTimersByTimeAsync(0);
    await assertion;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(server.sockets, "the first attempt is not retried").toHaveLength(1);
  });

  it("with reconnect: false the core closes at the first drop", async () => {
    const server = new FakeServer();
    const { core, states, closes } = await loaded(server, { reconnect: false });
    server.current.serverClose(1006);
    expect(core.closed).toBe(true);
    expect(closes).toHaveLength(1);
    expect(kinds(states).at(-1)).toBe("closed:failed");
  });

  it("an in-flight stream ends with the typed error and a fresh stream works after the reconnect", async () => {
    const server = new FakeServer();
    const { core } = await loaded(server);
    const iterator = core.stream(FREE, 2, new Uint8Array(0))[Symbol.asyncIterator]();
    const first = iterator.next();
    await settle();
    const { callId } = decodeCall((server.current.sent(Kind.Call)[0] as { payload: Uint8Array }).payload);
    server.current.deliver(Kind.Reply, encodeReply({ callId, status: ReplyStatus.StreamOpened }));
    server.current.deliver(Kind.StreamItem, encodeStreamItem({ callId, flag: StreamFlag.Item, body: u32(7) }));
    expect((await first).value).toEqual(u32(7));
    const second = iterator.next();
    const assertion = expect(second).rejects.toMatchObject({ reason: "closed" });
    server.current.serverClose(1006);
    await assertion;
  });
});

/** A store constructed by hand and observed through the core, answered by the fake server. */
async function observedStore(core: UndraCore, server: FakeServer, handle: bigint, count: number): Promise<CounterStore> {
  const created = CounterStore.create(core, handle);
  await settle();
  server.changeSet([{ handle, signalId: 0, op: ChangeOp.FullValue, value: u32(count) }]);
  await vi.advanceTimersByTimeAsync(200);
  return created;
}
