import { getEventListeners } from "node:events";
import { describe, expect, it, vi } from "vitest";
import { UndraCore } from "../src/core.js";
import { UndraError, UndraModeError, UndraReplyError, UndraSchemaMismatchError, UndraTransportError } from "../src/errors.js";
import type { Transport } from "../src/transport/transport.js";
import {
  ALL_SIGNALS,
  CallTarget,
  Kind,
  ReplyStatus,
  StreamFlag,
  WireError,
  codecs,
  decodeValue,
  encodeReply,
  encodeValue,
} from "../src/wire/index.js";
import { FakeCoreTransport, type FakeOptions, SCHEMA, type StreamScript } from "./support/fake-core.js";
import { captureLog, deferred, macrotask, microtasks, track } from "./support/harness.js";
import { u32 } from "./support/store.js";

const FREE = { target: CallTarget.FreeFunction } as const;
const M = { ADD: 1, FAIL: 2, SLOW: 3, TICKS: 4, PANIC: 5 } as const;

async function setup(
  options: FakeOptions & { observeTimeoutMs?: number; onClose?: (e: Error) => void; onError?: (e: unknown) => void } = {},
) {
  const fake = new FakeCoreTransport(options);
  const log = captureLog();
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log, http: null, timer: null },
      ...(options.observeTimeoutMs !== undefined && { observeTimeoutMs: options.observeTimeoutMs }),
      ...(options.onClose && { onClose: options.onClose }),
      ...(options.onError && { onError: options.onError }),
    }),
  );
  return { fake, core, log };
}

/** A stream script over an array. */
function items(...values: number[]): (call: unknown) => StreamScript {
  return () => {
    let i = 0;
    return {
      next: () => (i < values.length ? { done: false, value: u32(values[i++] as number) } : { done: true }),
    };
  };
}

/** A stream that never ends. */
function endless(): StreamScript {
  let i = 0;
  return { next: () => ({ done: false, value: u32(i++) }) };
}

async function collect(source: AsyncIterable<Uint8Array>): Promise<number[]> {
  const out: number[] = [];
  for await (const item of source) out.push(decodeValue(codecs.u32, item));
  return out;
}

describe("call", () => {
  it("sends a Call payload and resolves with the reply body", async () => {
    const { fake, core } = await setup();
    fake.echo(M.ADD, (args) => args);
    const body = await core.call(FREE, M.ADD, u32(41));
    expect(decodeValue(codecs.u32, body)).toBe(41);
    expect(fake.calls).toEqual([
      { target: CallTarget.FreeFunction, methodId: M.ADD, callId: 1, args: u32(41) },
    ]);
  });

  it("addresses object methods by handle", async () => {
    const { fake, core } = await setup();
    fake.echo(M.ADD);
    await core.call({ target: CallTarget.ObjectMethod, handle: 0x0000_0003_0000_0011n }, M.ADD, u32(1));
    expect(fake.calls[0]).toMatchObject({ target: CallTarget.ObjectMethod, handle: 0x0000_0003_0000_0011n });
  });

  it("accepts a bare FreeFunction target and refuses any other bare target", async () => {
    const { fake, core } = await setup();
    fake.echo(M.ADD);
    await core.call(CallTarget.FreeFunction, M.ADD, u32(1));
    expect(fake.calls[0]?.target).toBe(CallTarget.FreeFunction);
    await expect(core.call(CallTarget.ObjectMethod as unknown as typeof CallTarget.FreeFunction, M.ADD, u32(1))).rejects.toThrow(TypeError);
  });

  it("uses increasing call ids and routes interleaved replies by id", async () => {
    const { fake, core } = await setup();
    const gates = [deferred(), deferred(), deferred()];
    let n = 0;
    fake.on(M.SLOW, (call, r) => {
      const index = n++;
      void gates[index]?.promise.then(() => {
        r.ok(u32(index));
      });
      void call;
      r.defer();
    });
    const calls = [core.call(FREE, M.SLOW, u32(0)), core.call(FREE, M.SLOW, u32(0)), core.call(FREE, M.SLOW, u32(0))];
    expect(fake.calls.map((c) => c.callId)).toEqual([1, 2, 3]);
    gates[2]?.resolve();
    gates[0]?.resolve();
    gates[1]?.resolve();
    const results = (await Promise.all(calls)).map((b) => decodeValue(codecs.u32, b));
    expect(results).toEqual([0, 1, 2]);
  });

  it("rejects with UndraReplyError carrying status and body for typed errors", async () => {
    const { fake, core } = await setup();
    fake.on(M.FAIL, (_c, r) => {
      r.error(u32(7));
    });
    const failure = (await core.call(FREE, M.FAIL, new Uint8Array(0)).catch((e: unknown) => e)) as UndraReplyError;
    expect(failure).toBeInstanceOf(UndraReplyError);
    expect(failure).toBeInstanceOf(UndraError);
    expect(failure.kind).toBe("reply");
    expect(failure.status).toBe(ReplyStatus.Error);
    expect(decodeValue(codecs.u32, failure.body)).toBe(7);
    expect(failure.reason).toBeUndefined();
  });

  it("rejects with status 2 for a panic and exposes its message and backtrace", async () => {
    const { fake, core } = await setup();
    fake.on(M.PANIC, (_c, r) => {
      r.panic("index out of bounds", "at core::foo\nat core::bar");
    });
    const failure = (await core.call(FREE, M.PANIC, new Uint8Array(0)).catch((e: unknown) => e)) as UndraReplyError;
    expect(failure.status).toBe(ReplyStatus.Panic);
    expect(failure.reason).toBe("index out of bounds");
    expect(failure.backtrace).toBe("at core::foo\nat core::bar");
    expect(failure.message).toBe("the core panicked: index out of bounds");
  });

  it("rejects with status 5 and the reason of a bad request, status 3 when cancelled", async () => {
    const { fake, core } = await setup();
    fake.on(M.FAIL, (_c, r) => {
      r.cancelled();
    });
    const cancelled = (await core.call(FREE, M.FAIL, new Uint8Array(0)).catch((e: unknown) => e)) as UndraReplyError;
    expect(cancelled.status).toBe(ReplyStatus.Cancelled);
    expect(cancelled.body).toEqual(new Uint8Array(0));
    const unknown = (await core.call(FREE, 0xdead, new Uint8Array(0)).catch((e: unknown) => e)) as UndraReplyError;
    expect(unknown.status).toBe(ReplyStatus.BadRequest);
    expect(unknown.reason).toBe("unknown method 0xdead");
  });

  it("copes with a panic or bad request body that does not decode", async () => {
    const error = new UndraReplyError(ReplyStatus.Panic, new Uint8Array([1, 2, 3]));
    expect(error.reason).toBeUndefined();
    expect(error.message).toBe("the core panicked");
    expect(new UndraReplyError(ReplyStatus.BadRequest, new Uint8Array(0)).message).toBe("the core rejected the request");
  });

  it("ignores a reply nobody waits for", async () => {
    const { fake, core, log } = await setup();
    fake.emitRawReply(encodeReply({ callId: 99, status: ReplyStatus.Ok, body: new Uint8Array(0) }));
    await fake.settle();
    expect(core.closed).toBe(false);
    expect(log.records).toEqual([]);
  });

  it("reports a truncated reply and rejects a reply with an impossible status", async () => {
    const { fake, core, log } = await setup();
    fake.emitRawReply(new Uint8Array([1, 0, 0]));
    await fake.settle();
    expect(log.records.some((r) => r.level === 4 && r.message.includes("truncated reply"))).toBe(true);
    fake.on(M.ADD, (_c, r) => {
      r.defer();
      fake.emitRawReply(new Uint8Array([1, 0, 0, 0, 9]));
    });
    const failure = (await core.call(FREE, M.ADD, new Uint8Array(0)).catch((e: unknown) => e)) as UndraTransportError;
    expect(failure).toBeInstanceOf(UndraTransportError);
    expect(failure.reason).toBe("protocol");
  });

  it("a plain call to a stream method fails and cancels the stream the core opened", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, items(1, 2));
    const failure = await core.call(FREE, M.TICKS, new Uint8Array(0)).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(UndraError);
    expect((failure as Error).message).toContain("UndraCore.stream");
    await fake.settle();
    expect(fake.cancelled).toEqual([1]);
  });

  it("rejects when the transport refuses the send, leaving nothing pending", async () => {
    const { fake, core } = await setup();
    const boom = new UndraReplyError(ReplyStatus.BadRequest, new Uint8Array(0));
    vi.spyOn(fake, "send").mockImplementation(() => {
      throw boom;
    });
    await expect(core.call(FREE, M.ADD, new Uint8Array(0))).rejects.toBe(boom);
    expect((await core.stats()).pendingCalls).toBe(0);
  });

  it("rejects on a closed core", async () => {
    const { core } = await setup();
    core.close();
    await expect(core.call(FREE, M.ADD, new Uint8Array(0))).rejects.toMatchObject({ kind: "transport", reason: "closed" });
    await expect(core.construct(1, 2, new Uint8Array(0))).rejects.toBeInstanceOf(UndraTransportError);
    await expect(core.observe(1n, 0, true)).rejects.toBeInstanceOf(UndraTransportError);
    expect(() => core.event(1, 2, new Uint8Array(0))).toThrow(UndraTransportError);
  });
});

describe("call cancellation", () => {
  it("an already aborted signal rejects without sending anything", async () => {
    const { fake, core } = await setup();
    const controller = new AbortController();
    controller.abort();
    await expect(core.call(FREE, M.ADD, new Uint8Array(0), controller.signal)).rejects.toBe(controller.signal.reason);
    expect(fake.calls).toHaveLength(0);
  });

  it("aborting while in flight sends Cancel and rejects with the reason at once", async () => {
    const { fake, core } = await setup();
    fake.on(M.SLOW, (_c, r) => {
      r.defer();
    });
    const controller = new AbortController();
    const reason = new Error("user navigated away");
    const call = core.call(FREE, M.SLOW, new Uint8Array(0), controller.signal);
    const assertion = expect(call).rejects.toBe(reason);
    controller.abort(reason);
    await assertion;
    expect(fake.cancelled).toEqual([1]);
    expect((await core.stats()).pendingCalls).toBe(0);
    // The core's answer to the cancelled call (status 3, or a result that raced the cancel) is ignored.
    fake.reply(1, ReplyStatus.Ok, u32(1));
    await fake.settle();
    expect(core.closed).toBe(false);
  });

  it("the default abort reason is an AbortError", async () => {
    const { fake, core } = await setup();
    fake.on(M.SLOW, (_c, r) => {
      r.defer();
    });
    const controller = new AbortController();
    const call = core.call(FREE, M.SLOW, new Uint8Array(0), controller.signal);
    controller.abort();
    await expect(call).rejects.toMatchObject({ name: "AbortError" });
  });

  it("removes its abort listener once the call completes, and a late abort cancels nothing", async () => {
    const { fake, core } = await setup();
    fake.echo(M.ADD);
    const controller = new AbortController();
    await core.call(FREE, M.ADD, u32(1), controller.signal);
    expect(getEventListeners(controller.signal, "abort")).toHaveLength(0);
    controller.abort();
    expect(fake.cancelled).toEqual([]);
  });

  it("a failed send removes the abort listener", async () => {
    const { fake, core } = await setup();
    vi.spyOn(fake, "send").mockImplementation(() => {
      throw new UndraTransportError("closed", "gone");
    });
    const controller = new AbortController();
    await expect(core.call(FREE, M.ADD, new Uint8Array(0), controller.signal)).rejects.toBeInstanceOf(UndraTransportError);
    expect(getEventListeners(controller.signal, "abort")).toHaveLength(0);
  });
});

describe("callSync", () => {
  it("returns the reply body when the core runs in process", async () => {
    const { fake, core } = await setup({ synchronous: true });
    fake.echo(M.ADD, (a) => a);
    expect(core.callSync(FREE, M.ADD, u32(5))).toEqual(u32(5));
  });

  it("throws UndraReplyError for every non-ok status", async () => {
    const { fake, core } = await setup({ synchronous: true });
    fake.on(M.FAIL, (_c, r) => {
      r.error(u32(3));
    });
    fake.on(M.PANIC, (_c, r) => {
      r.panic("oops");
    });
    const fail = (fn: () => unknown): UndraReplyError => {
      try {
        fn();
      } catch (error) {
        return error as UndraReplyError;
      }
      throw new Error("did not throw");
    };
    expect(fail(() => core.callSync(FREE, M.FAIL, new Uint8Array(0))).status).toBe(ReplyStatus.Error);
    expect(fail(() => core.callSync(FREE, M.PANIC, new Uint8Array(0))).status).toBe(ReplyStatus.Panic);
    expect(fail(() => core.callSync(FREE, 0xbeef, new Uint8Array(0))).status).toBe(ReplyStatus.BadRequest);
  });

  it("is a mode error in every asynchronous mode", async () => {
    const { core } = await setup({ mode: "wasm-worker" });
    expect(() => core.callSync(FREE, M.ADD, new Uint8Array(0))).toThrow(UndraModeError);
    try {
      core.callSync(FREE, M.ADD, new Uint8Array(0));
    } catch (error) {
      expect(error).toMatchObject({ kind: "mode", operation: "callSync", mode: "wasm-worker" });
    }
  });
});

describe("construct", () => {
  it("returns the handle from the reply body", async () => {
    const { fake, core } = await setup();
    fake.on(M.ADD, (call, r) => {
      expect(call).toMatchObject({ target: CallTarget.Constructor, typeId: 0xabc, methodId: M.ADD });
      r.ok(encodeValue(codecs.u64, 0x0000_0001_0000_0009n));
    });
    await expect(core.construct(0xabc, M.ADD, new Uint8Array(0))).resolves.toBe(0x0000_0001_0000_0009n);
    expect((await core.stats()).liveHandles).toBe(0); // the fake reports its store count
    core.release(0x0000_0001_0000_0009n);
  });

  it("rejects with the typed error of a fallible constructor", async () => {
    const { fake, core } = await setup();
    fake.on(M.FAIL, (_c, r) => {
      r.error(u32(1));
    });
    await expect(core.construct(1, M.FAIL, new Uint8Array(0))).rejects.toMatchObject({ status: ReplyStatus.Error });
  });

  it("rejects a reply body that is not a handle", async () => {
    const { fake, core } = await setup();
    fake.on(M.ADD, (_c, r) => {
      r.ok(u32(1));
    });
    await expect(core.construct(1, M.ADD, new Uint8Array(0))).rejects.toBeInstanceOf(WireError);
  });

  it("counts handles when the core reports no statistics of its own", async () => {
    const fake = new FakeCoreTransport();
    fake.stats = () => Promise.resolve(null);
    const core = track(await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false }));
    let next = 4n;
    fake.on(M.ADD, (_c, r) => {
      r.ok(encodeValue(codecs.u64, ++next));
    });
    await core.construct(1, M.ADD, new Uint8Array(0));
    await core.construct(1, M.ADD, new Uint8Array(0));
    expect((await core.stats()).liveHandles).toBe(2);
    core.release(5n);
    expect((await core.stats()).liveHandles).toBe(1);
    expect((await core.stats()).core).toBeNull();
  });
});

describe("stream", () => {
  it("yields the items in order and ends", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, items(10, 20, 30));
    expect(await collect(core.stream(FREE, M.TICKS, new Uint8Array(0)))).toEqual([10, 20, 30]);
    expect((await core.stats()).openStreams).toBe(0);
  });

  it("sends nothing until the iteration starts, and each iteration is its own call", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, items(1));
    const source = core.stream(FREE, M.TICKS, new Uint8Array(0));
    expect(fake.calls).toHaveLength(0);
    expect(await collect(source)).toEqual([1]);
    expect(await collect(source)).toEqual([1]);
    expect(fake.calls.map((c) => c.callId)).toEqual([1, 2]);
  });

  it("grants 16 credit when the core opens the stream, not before", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, items(1, 2, 3));
    const iterator = core.stream(FREE, M.TICKS, new Uint8Array(0))[Symbol.asyncIterator]();
    expect(fake.credits).toEqual([]);
    await macrotask();
    await macrotask();
    expect(fake.credits).toEqual([{ callId: 1, credit: 16 }]);
    await iterator.return?.();
  });

  it("tops the credit up to 16 when it falls below 8, and never lets the core run further ahead", async () => {
    const { fake, core } = await setup();
    let produced = 0;
    fake.stream(M.TICKS, () => ({
      next: () => ({ done: false, value: u32(produced++) }),
    }));
    const iterator = core.stream(FREE, M.TICKS, new Uint8Array(0))[Symbol.asyncIterator]();
    const first = await iterator.next();
    expect(decodeValue(codecs.u32, first.value as Uint8Array)).toBe(0);
    await fake.settle();
    // 16 delivered (credit 16), plus the one item the core has polled and holds back.
    expect(produced).toBeLessThanOrEqual(17);
    expect(fake.credits).toEqual([{ callId: 1, credit: 16 }]);
    // Consume up to 8 items: the credit left is 8, which is not below the low-water mark.
    for (let i = 1; i < 8; i++) await iterator.next();
    await fake.settle();
    expect(fake.credits).toEqual([{ callId: 1, credit: 16 }]);
    // The ninth consumption leaves 7: grant 9, back to 16.
    await iterator.next();
    await fake.settle();
    expect(fake.credits).toEqual([
      { callId: 1, credit: 16 },
      { callId: 1, credit: 9 },
    ]);
    // The core never got more than consumed + 16 (+1 held back).
    expect(produced).toBeLessThanOrEqual(9 + 16 + 1);
    await iterator.return?.();
  });

  it("a consumer that leaves the loop early cancels the stream", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, endless);
    for await (const item of core.stream(FREE, M.TICKS, new Uint8Array(0))) {
      expect(decodeValue(codecs.u32, item)).toBe(0);
      break;
    }
    expect(fake.cancelled).toEqual([1]);
    expect((await core.stats()).openStreams).toBe(0);
  });

  it("an exception in the loop body cancels the stream too", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, endless);
    await expect(
      (async () => {
        for await (const _item of core.stream(FREE, M.TICKS, new Uint8Array(0))) throw new Error("body failed");
      })(),
    ).rejects.toThrow("body failed");
    expect(fake.cancelled).toEqual([1]);
  });

  it("does not cancel a stream that already ended", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, items(1));
    await collect(core.stream(FREE, M.TICKS, new Uint8Array(0)));
    expect(fake.cancelled).toEqual([]);
  });

  it("delivers the buffered items before a stream error, which rejects like a failed call", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, () => {
      let i = 0;
      return { next: () => (i++ < 2 ? { done: false, value: u32(i) } : { done: true, error: u32(99) }) };
    });
    const seen: number[] = [];
    const failure = await (async () => {
      try {
        for await (const item of core.stream(FREE, M.TICKS, new Uint8Array(0))) seen.push(decodeValue(codecs.u32, item));
      } catch (error) {
        return error as UndraReplyError;
      }
      return undefined;
    })();
    expect(seen).toEqual([1, 2]);
    expect(failure).toBeInstanceOf(UndraReplyError);
    expect(failure?.status).toBe(ReplyStatus.Error);
    expect(decodeValue(codecs.u32, failure?.body as Uint8Array)).toBe(99);
  });

  it("rejects the first next() when the core refuses to open the stream", async () => {
    const { core } = await setup();
    const failure = await collect(core.stream(FREE, 0xbad, new Uint8Array(0))).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(UndraReplyError);
    expect((failure as UndraReplyError).status).toBe(ReplyStatus.BadRequest);
  });

  it("rejects a stream call answered with a plain result", async () => {
    const { fake, core } = await setup();
    fake.echo(M.ADD);
    const failure = await collect(core.stream(FREE, M.ADD, new Uint8Array(0))).catch((e: unknown) => e);
    expect(failure).toMatchObject({ kind: "transport", reason: "protocol" });
  });

  it("items for a stream nobody iterates any more are dropped quietly", async () => {
    const { fake, core } = await setup();
    fake.emitStreamItem(77, StreamFlag.Item, u32(1));
    fake.emitStreamItem(77, StreamFlag.End);
    fake.emitStreamItem(77, StreamFlag.Error, u32(1));
    await fake.settle();
    expect(core.closed).toBe(false);
  });

  it("an unknown stream flag fails the stream and a truncated item is reported", async () => {
    const { fake, core, log } = await setup();
    fake.stream(M.TICKS, () => ({ next: () => ({ done: false, value: u32(1) }) }));
    const iterator = core.stream(FREE, M.TICKS, new Uint8Array(0))[Symbol.asyncIterator]();
    await iterator.next();
    fake.emitRawStreamItem(new Uint8Array([1, 0]));
    fake.emitRawStreamItem(new Uint8Array([1, 0, 0, 0, 7]));
    await fake.settle();
    expect(log.records.some((r) => r.message.includes("truncated stream item"))).toBe(true);
    let outcome: unknown;
    for (let i = 0; i < 40 && outcome === undefined; i++) outcome = await iterator.next().then(() => undefined, (e: unknown) => e);
    expect(outcome).toMatchObject({ kind: "transport", reason: "protocol" });
  });

  it("fails the stream when the channel is lost, after the buffered items", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, () => ({ next: () => ({ done: false, value: u32(1) }) }));
    const iterator = core.stream(FREE, M.TICKS, new Uint8Array(0))[Symbol.asyncIterator]();
    await iterator.next();
    fake.fail();
    await fake.settle();
    // The item the core had already delivered is still there; then the loss surfaces.
    let outcome: unknown;
    for (let i = 0; i < 40; i++) {
      try {
        const step = await iterator.next();
        if (step.done) break;
      } catch (error) {
        outcome = error;
        break;
      }
    }
    expect(outcome).toBeInstanceOf(UndraTransportError);
  });

  it("a stream started on a closed core fails on the first next()", async () => {
    const { core } = await setup();
    core.close();
    await expect(collect(core.stream(FREE, M.TICKS, new Uint8Array(0)))).rejects.toBeInstanceOf(UndraTransportError);
  });

  it("failing to grant credit fails the stream", async () => {
    const { fake, core } = await setup();
    fake.stream(M.TICKS, () => ({ next: () => ({ done: false, value: u32(1) }) }));
    const original = fake.send.bind(fake);
    vi.spyOn(fake, "send").mockImplementation((kind, payload) => {
      if (kind === Kind.StreamCredit) throw new UndraTransportError("closed", "no credit for you");
      original(kind, payload);
    });
    const outcome = await collect(core.stream(FREE, M.TICKS, new Uint8Array(0))).catch((e: unknown) => e);
    expect(outcome).toBeInstanceOf(UndraTransportError);
  });
});

describe("handshake and lifetime", () => {
  it("refuses a core with another schema hash", async () => {
    const fake = new FakeCoreTransport({ schemaHash: 0xdead_beef_dead_beefn });
    const failure = await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false }).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(UndraSchemaMismatchError);
    expect(failure).toMatchObject({ expected: SCHEMA, got: 0xdead_beef_dead_beefn, kind: "schemaMismatch" });
    expect(fake.closed).toBe(true);
  });

  it("exposes the core's Hello, mode and closed state", async () => {
    const { core } = await setup({ mode: "wasm-worker" });
    expect(core.hello).toMatchObject({ undraVersion: "fake", schemaHash: SCHEMA });
    expect(core.mode).toBe("wasm-worker");
    expect(core.closed).toBe(false);
  });

  it("the first core loaded is shared, later ones are not, and closing it frees the slot", async () => {
    expect(UndraCore.current).toBeNull();
    expect(UndraCore.shared.closed).toBe(true);
    await expect(UndraCore.shared.call(FREE, M.ADD, new Uint8Array(0))).rejects.toThrow(/UndraCore\.load/);
    const first = track(await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA }));
    const second = track(await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA }));
    expect(UndraCore.shared).toBe(first);
    expect(UndraCore.current).toBe(first);
    second.close();
    expect(UndraCore.shared).toBe(first);
    first.close();
    expect(UndraCore.current).toBeNull();
    expect(UndraCore.shared.closed).toBe(true);
    const third = track(await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA }));
    expect(UndraCore.shared).toBe(third);
  });

  it("shared: false keeps a core out of the slot; a failed start never takes it", async () => {
    await UndraCore.attach(new FakeCoreTransport({ schemaHash: 1n }), { expectedSchemaHash: SCHEMA }).catch(() => undefined);
    expect(UndraCore.current).toBeNull();
    track(await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false }));
    expect(UndraCore.current).toBeNull();
  });

  it("load validates its options", async () => {
    await expect(UndraCore.load({ mode: "wasm-main", expectedSchemaHash: SCHEMA })).rejects.toMatchObject({ kind: "options" });
    await expect(UndraCore.load({ mode: "wasm-worker", expectedSchemaHash: SCHEMA })).rejects.toMatchObject({ kind: "options" });
    await expect(UndraCore.load({ mode: "remote", expectedSchemaHash: SCHEMA })).rejects.toMatchObject({ kind: "options" });
    await expect(UndraCore.load({ mode: "nope" as "remote", expectedSchemaHash: SCHEMA })).rejects.toThrow(/unknown mode 'nope'/);
  });

  it("a lost channel rejects what is in flight, closes the core and calls onClose once", async () => {
    const onClose = vi.fn();
    const { fake, core } = await setup({ onClose });
    fake.on(M.SLOW, (_c, r) => {
      r.defer();
    });
    const pending = core.call(FREE, M.SLOW, new Uint8Array(0));
    const failure = new UndraTransportError("closed", "socket reset");
    const assertion = expect(pending).rejects.toBe(failure);
    fake.fail(failure);
    await assertion;
    expect(core.closed).toBe(true);
    expect(onClose).toHaveBeenCalledExactlyOnceWith(failure);
    expect(fake.closed).toBe(true);
    fake.fail(new UndraTransportError("closed", "again"));
    await fake.settle();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("close rejects in-flight calls and pending observes, stops nothing twice, and does not call onClose", async () => {
    const onClose = vi.fn();
    const { fake, core } = await setup({ onClose, observeTimeoutMs: 0 });
    fake.on(M.SLOW, (_c, r) => {
      r.defer();
    });
    const pending = core.call(FREE, M.SLOW, new Uint8Array(0));
    const observing = core.observe(9n, ALL_SIGNALS, true);
    core.close();
    core.close();
    await expect(pending).rejects.toMatchObject({ reason: "closed" });
    await expect(observing).rejects.toMatchObject({ reason: "closed" });
    expect(onClose).not.toHaveBeenCalled();
    expect(fake.closed).toBe(true);
  });

  it("survives an onClose callback that throws", async () => {
    const { fake, log } = await setup({
      onClose: () => {
        throw new Error("handler bug");
      },
    });
    fake.fail();
    await fake.settle();
    expect(log.records.some((r) => r.message.includes("handler bug"))).toBe(true);
  });
});

describe("events, timers, release, log, statistics", () => {
  it("event sends an Event envelope", async () => {
    const { fake, core } = await setup();
    core.event(0xaaaa_bbbb, 0xcccc_dddd, new Uint8Array([1, 0]));
    expect(fake.events).toEqual([{ portId: 0xaaaa_bbbb, methodId: 0xcccc_dddd, payload: new Uint8Array([1, 0]) }]);
  });

  it("timerFired sends TimerFired", async () => {
    const { fake, core } = await setup();
    core.timerFired(0x8000_0001);
    expect(fake.timersFired).toEqual([0x8000_0001]);
  });

  it("release unregisters the store and sends Release; unknown handles are harmless", async () => {
    const { fake, core } = await setup();
    core.mirror.register(3n, () => {});
    core.release(3n);
    core.release(3n);
    core.release(12345n);
    expect(core.mirror.has(3n)).toBe(false);
    expect(fake.released).toEqual([3n, 3n, 12345n]);
  });

  it("release after close does nothing and does not throw", async () => {
    const { fake, core } = await setup();
    core.close();
    core.release(3n);
    expect(fake.released).toEqual([]);
  });

  it("routes the core's log records to the log adapter", async () => {
    const { fake, log } = await setup();
    fake.emitLog(3, "undra::query", "cache miss");
    await fake.settle();
    expect(log.records).toEqual([{ level: 3, target: "undra::query", message: "cache miss" }]);
  });

  it("a log adapter that throws does not break the core", async () => {
    const fake = new FakeCoreTransport();
    const core = track(
      await UndraCore.attach(fake, {
        expectedSchemaHash: SCHEMA,
        shared: false,
        adapters: {
          http: null,
          timer: null,
          log: {
            log() {
              throw new Error("sink failed");
            },
          },
        },
      }),
    );
    fake.emitLog(2, "t", "m");
    await fake.settle();
    expect(core.closed).toBe(false);
  });

  it("statistics count pending calls and open streams", async () => {
    const { fake, core } = await setup();
    fake.on(M.SLOW, (_c, r) => {
      r.defer();
    });
    fake.stream(M.TICKS, () => ({ next: () => ({ done: false, value: u32(1) }) }));
    const call = core.call(FREE, M.SLOW, new Uint8Array(0));
    const iterator = core.stream(FREE, M.TICKS, new Uint8Array(0))[Symbol.asyncIterator]();
    await iterator.next();
    const stats = await core.stats();
    expect(stats).toMatchObject({ pendingCalls: 1, openStreams: 1, mirroredStores: 0 });
    expect(stats.core).toMatchObject({ platform: "fake" });
    await iterator.return?.();
    core.close();
    await call.catch(() => undefined);
    expect((await core.stats()).core).toBeNull();
  });

  it("statistics tolerate a core whose JSON is not an object", async () => {
    const { fake, core } = await setup();
    fake.stats = () => Promise.resolve("not json");
    expect((await core.stats()).core).toBeNull();
    fake.stats = () => Promise.resolve("[1,2]");
    expect((await core.stats()).core).toEqual([1, 2]);
  });

  it("works against a transport that has no stats", async () => {
    const transport: Transport = {
      mode: "remote",
      synchronous: false,
      start: () => Promise.resolve({ undraVersion: "x", schemaHash: SCHEMA, platform: "p", mode: "m" }),
      send: () => {},
      close: () => {},
    };
    const core = track(await UndraCore.attach(transport, { expectedSchemaHash: SCHEMA, shared: false }));
    expect((await core.stats()).core).toBeNull();
    await microtasks();
  });
});
