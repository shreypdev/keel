import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { writeHead } from "../src/call-head.js";
import { UndraCore } from "../src/core.js";
import { framed } from "../src/transport/framed.js";
import type { CoreTransport, Transport } from "../src/transport/transport.js";
import { CallTarget, Kind, UndraWriter, encodeCancel, encodeEvent, encodeObserve, encodeRelease, encodeStreamCredit, encodeTimerFired } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, track } from "./support/harness.js";
import { fromHex, toHex } from "./helpers.js";

/*
 * `framed(transport)` drives a transport that has only `send(kind, payload)` through the typed channel `UndraCore` speaks
 * (ADR-057). What it must keep is every byte `send` always received: each control message is checked against the layout of
 * SPEC 3.3 to 3.8 written out by hand, and the `Call` against the shared vector of contract-tests/wire-vectors.json.
 */

interface Sent {
  readonly kind: Kind;
  readonly payload: Uint8Array;
}

/** A transport with `send` only, recording what it is given. */
function recording(extra: Partial<Transport> = {}): { transport: Transport; sent: Sent[] } {
  const sent: Sent[] = [];
  const transport: Transport = {
    mode: "remote",
    synchronous: false,
    start: () => Promise.resolve({ undraVersion: "x", schemaHash: SCHEMA, platform: "p", mode: "m" }),
    send: (kind, payload) => {
      sent.push({ kind, payload });
    },
    close: () => {},
    ...extra,
  };
  return { transport, sent };
}

describe("framed: each control message is the payload send always received", () => {
  it("Observe: handle u64, signal id u32, on u8", () => {
    const { transport, sent } = recording();
    const channel = framed(transport);
    channel.observe(0x0000_0002_0000_0009n, 3, true);
    channel.observe(0x0000_0002_0000_0009n, 0xffff_ffff, false);
    expect(sent.map((s) => [s.kind, toHex(s.payload)])).toEqual([
      [Kind.Observe, "09000000020000000300000001"],
      [Kind.Observe, "0900000002000000ffffffff00"],
    ]);
    expect(sent[0]?.payload).toEqual(encodeObserve({ handle: 0x0000_0002_0000_0009n, signalId: 3, on: true }));
  });

  it("Release: handle u64", () => {
    const { transport, sent } = recording();
    framed(transport).release(0x0000_0002_0000_0009n);
    expect(sent).toEqual([{ kind: Kind.Release, payload: fromHex("0900000002000000") }]);
    expect(sent[0]?.payload).toEqual(encodeRelease({ handle: 0x0000_0002_0000_0009n }));
  });

  it("Cancel: call id u32; StreamCredit: call id u32, credit u32", () => {
    const { transport, sent } = recording();
    const channel = framed(transport);
    channel.cancel(0xffff_fff0);
    channel.streamCredit(7, 16);
    expect(sent.map((s) => [s.kind, toHex(s.payload)])).toEqual([
      [Kind.Cancel, "f0ffffff"],
      [Kind.StreamCredit, "0700000010000000"],
    ]);
    expect(sent[0]?.payload).toEqual(encodeCancel({ callId: 0xffff_fff0 }));
    expect(sent[1]?.payload).toEqual(encodeStreamCredit({ callId: 7, credit: 16 }));
  });

  it("Event: port id u32, method id u32, the payload as the tail; TimerFired: timer id u32", () => {
    const { transport, sent } = recording();
    const channel = framed(transport);
    channel.event(0xdead_beef, 0xfeed_f00d, Uint8Array.of(1, 2, 3));
    channel.event(1, 2, new Uint8Array(0));
    channel.timerFired(0x8000_0007);
    expect(sent.map((s) => [s.kind, toHex(s.payload)])).toEqual([
      [Kind.Event, "efbeadde0df0edfe010203"],
      [Kind.Event, "0100000002000000"],
      [Kind.TimerFired, "07000080"],
    ]);
    expect(sent[0]?.payload).toEqual(encodeEvent({ portId: 0xdead_beef, methodId: 0xfeed_f00d, payload: Uint8Array.of(1, 2, 3) }));
    expect(sent[2]?.payload).toEqual(encodeTimerFired({ timerId: 0x8000_0007 }));
  });

  it("PortReply: the reply as it is, the same array", () => {
    const { transport, sent } = recording();
    const reply = fromHex("0100000000aabb");
    framed(transport).portReply(reply);
    expect(sent).toHaveLength(1);
    expect(sent[0]?.kind).toBe(Kind.PortReply);
    expect(sent[0]?.payload).toBe(reply);
  });

  it("Call: header and arguments apart make the vector's payload, in a new array that is not the reused header", () => {
    const vector = (JSON.parse(readFileSync(new URL("../../../../../contract-tests/wire-vectors.json", import.meta.url), "utf8")) as {
      vectors: Array<{ name: string; value: { target: number; handle: string; method_id: string; call_id: number; args: number[] }; hex: string }>;
    }).vectors.find((v) => v.name === "call_method");
    if (vector === undefined) throw new Error("the call_method vector is gone");
    const { transport, sent } = recording();
    const channel = framed(transport);
    const head = new Uint8Array(17);
    writeHead(head, { target: CallTarget.ObjectMethod, handle: BigInt(vector.value.handle) }, Number(vector.value.method_id), vector.value.call_id);
    // The vector's arguments are two u32 (2 and 3).
    const args = new UndraWriter();
    for (const n of vector.value.args) args.writeU32(n);
    channel.sendCall(head, args.finish());
    expect(sent[0]?.kind).toBe(Kind.Call);
    expect(toHex(sent[0]?.payload as Uint8Array)).toBe(vector.hex);
    // The core reuses its header for the next call: what was sent must not change under the transport.
    head.fill(0xee);
    expect(toHex(sent[0]?.payload as Uint8Array)).toBe(vector.hex);
  });

  it("Call with no arguments still copies the reused header; a head without a tail is a whole payload and goes as it is", () => {
    const { transport, sent } = recording();
    const channel = framed(transport);
    const head = Uint8Array.of(0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 9, 0, 0, 0);
    channel.sendCall(head, new Uint8Array(0));
    expect(sent[0]?.payload).toEqual(head);
    expect(sent[0]?.payload).not.toBe(head);
    const whole = Uint8Array.of(3, 1, 2, 3);
    channel.sendCall(whole);
    expect(sent[1]?.payload).toBe(whole);
  });
});

describe("framed: what is not a control message", () => {
  it("keeps the transport as it is: mode, synchronous, start and close go through", async () => {
    const { transport } = recording({ mode: "wasm-worker" });
    let closed = 0;
    const closing: Transport = { ...transport, close: () => void closed++ };
    const channel = framed(closing);
    expect([channel.mode, channel.synchronous]).toEqual(["wasm-worker", false]);
    expect((await channel.start({} as never)).schemaHash).toBe(SCHEMA);
    channel.close();
    expect(closed).toBe(1);
  });

  it("has the optional members exactly when the transport has them, and calls the transport's current one", async () => {
    const bare = framed(recording().transport);
    for (const name of ["callSync", "callSyncParts", "stats", "snapshot", "restore", "portAdded", "restart"] as const) {
      expect(bare[name], name).toBeUndefined();
    }
    let statsCalls = 0;
    const calls: string[] = [];
    const rich: Transport = {
      ...recording().transport,
      synchronous: true,
      callSync: (payload) => {
        calls.push(`callSync ${payload.length}`);
        return new Uint8Array(5);
      },
      stats: () => {
        statsCalls++;
        return Promise.resolve("{}");
      },
      snapshot: () => Promise.resolve(Uint8Array.of(1)),
      restore: () => Promise.resolve(),
      portAdded: (portId) => void calls.push(`portAdded ${portId}`),
      restart: () => Promise.reject(new Error("no")),
    };
    const channel = framed(rich);
    expect(channel.callSync?.(new Uint8Array(3))).toEqual(new Uint8Array(5));
    channel.portAdded?.(9, { sync: false, methods: {} });
    expect(calls).toEqual(["callSync 3", "portAdded 9"]);
    await channel.stats?.();
    // A method replaced after the wrap is the one that runs (the transport stays the transport).
    (rich as { stats: Transport["stats"] }).stats = () => Promise.resolve("replaced");
    expect(await channel.stats?.()).toBe("replaced");
    expect(statsCalls).toBe(1);
  });

  it("uses a transport's own typed methods and sendCall where it has them, and frames the rest", () => {
    const { transport, sent } = recording();
    const calls: string[] = [];
    const partial: Transport = {
      ...transport,
      observe: (handle, signalId, on) => void calls.push(`observe ${handle} ${signalId} ${on}`),
      sendCall: (head, tail) => void calls.push(`sendCall ${head.length} ${tail?.length ?? "-"}`),
    };
    const channel = framed(partial);
    channel.observe(5n, 1, true);
    channel.sendCall(new Uint8Array(17), new Uint8Array(2));
    channel.release(5n);
    expect(calls).toEqual(["observe 5 1 true", "sendCall 17 2"]);
    expect(sent.map((s) => s.kind)).toEqual([Kind.Release]);
  });

  it("does not modify the transport it wraps", () => {
    const { transport } = recording();
    const before = Object.keys(transport).sort();
    framed(transport);
    expect(Object.keys(transport).sort()).toEqual(before);
    expect((transport as { observe?: unknown }).observe).toBeUndefined();
  });
});

describe("UndraCore.attach: a transport with only send is wrapped, one with the typed methods is not", () => {
  it("attaches a send-only transport and drives it through framed (the fake core's own typed members are absent)", async () => {
    const fake = new FakeCoreTransport({ mode: "remote" });
    expect((fake as unknown as { observe?: unknown }).observe).toBeUndefined();
    const core = track(await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: captureLog(), http: null, timer: null } }));
    fake.store(5n, new Map([[0, Uint8Array.of(7, 0, 0, 0)]]));
    core.mirror.register(5n, () => {});
    const observed = core.observe(5n, 0, true);
    core.release(6n);
    await observed.catch(() => {});
    expect(fake.observed.map((o) => [o.handle, o.signalId, o.on])).toEqual([[5n, 0, true]]);
    expect(fake.released).toEqual([6n]);
  });

  it("passes a transport that has the typed methods to the core as it is", async () => {
    const calls: string[] = [];
    const typed: CoreTransport = {
      mode: "wasm-main",
      synchronous: false,
      start: () => Promise.resolve({ undraVersion: "x", schemaHash: SCHEMA, platform: "p", mode: "m" }),
      close: () => {},
      sendCall: () => void calls.push("sendCall"),
      observe: (handle) => void calls.push(`observe ${handle}`),
      release: (handle) => void calls.push(`release ${handle}`),
      cancel: () => {},
      streamCredit: () => {},
      event: () => {},
      timerFired: () => {},
      portReply: () => {},
    };
    const core = track(await UndraCore.attach(typed as unknown as Transport, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: captureLog(), http: null, timer: null } }));
    core.release(4n);
    expect(calls).toEqual(["release 4"]);
  });
});
