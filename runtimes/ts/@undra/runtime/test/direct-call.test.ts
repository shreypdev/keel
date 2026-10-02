import { describe, expect, it } from "vitest";
import { UndraCore } from "../src/core.js";
import { UndraReplyError, UndraTransportError } from "../src/errors.js";
import { CallTarget, Kind, ReplyStatus, decodeCall, encodeCall } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, macrotask, track } from "./support/harness.js";

/*
 * The call path of ADR-056: a call on a core that answers inside `send` registers no promise before the send (the
 * direct call), and its payload is built in one array (`encodeTarget`) or as a reused header beside the arguments
 * (`Transport.sendCall`, `callSyncParts`). Everything a caller can see stays what `UndraCore.call` always did.
 */

const FREE = { target: CallTarget.FreeFunction } as const;
const ECHO = 7;
const SLOW = 8;

/** An in-process fake core that also offers `sendCall` and `callSyncParts`, and keeps what they were given. */
class PartsTransport extends FakeCoreTransport {
  readonly heads: Uint8Array[] = [];
  readonly joined: Uint8Array[] = [];
  readonly sendCall = (head: Uint8Array, tail: Uint8Array): void => {
    this.heads.push(head.slice());
    const whole = new Uint8Array(head.length + tail.length);
    whole.set(head, 0);
    whole.set(tail, head.length);
    this.joined.push(whole);
    this.send(Kind.Call, whole);
  };
  readonly callSyncParts = (head: Uint8Array, tail: Uint8Array): Uint8Array => {
    const whole = new Uint8Array(head.length + tail.length);
    whole.set(head, 0);
    whole.set(tail, head.length);
    this.joined.push(whole);
    return (this.callSync as (payload: Uint8Array) => Uint8Array)(whole);
  };
}

async function boot<T extends FakeCoreTransport>(fake: T, options: { onError?: (e: unknown) => void } = {}) {
  const log = captureLog();
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log, http: null, timer: null },
      ...(options.onError && { onError: options.onError }),
    }),
  );
  return { core, fake, log };
}

const bytes = (...values: number[]): Uint8Array => Uint8Array.from(values);

describe("a call on a core that answers inside send", () => {
  it("returns a promise that is already settled when the method replied at once, and leaves nothing pending", async () => {
    const { core } = await boot(new FakeCoreTransport({ synchronous: true }).echo(ECHO));
    const promise = core.call(FREE, ECHO, bytes(1, 2, 3));
    // Both are resolved: the promise to beat is the one a settled call returns.
    expect(await Promise.race([promise, Promise.resolve("later")])).toEqual(bytes(1, 2, 3));
    expect((await core.stats()).pendingCalls).toBe(0);
  });

  it("gives a call the core answers later its promise after the send, and settles it when the reply arrives", async () => {
    const fake = new FakeCoreTransport({ synchronous: true }).on(SLOW, (_call, respond) => respond.defer());
    const { core } = await boot(fake);
    let settled: Uint8Array | undefined;
    const pending = core.call(FREE, SLOW, bytes(9)).then((body) => (settled = body));
    await macrotask();
    expect(settled).toBeUndefined();
    expect((await core.stats()).pendingCalls).toBe(1);
    fake.reply(fake.calls[0]?.callId as number, ReplyStatus.Ok, bytes(4, 5));
    await pending;
    expect(settled).toEqual(bytes(4, 5));
    expect((await core.stats()).pendingCalls).toBe(0);
  });

  it("rejects with the reply's status and body when the method failed at once", async () => {
    const fake = new FakeCoreTransport({ synchronous: true }).on(ECHO, (_call, respond) => respond.error(bytes(7, 7)));
    const { core } = await boot(fake);
    const error = await core.call(FREE, ECHO, bytes()).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(UndraReplyError);
    expect((error as UndraReplyError).status).toBe(ReplyStatus.Error);
    expect((error as UndraReplyError).body).toEqual(bytes(7, 7));
  });

  it("rejects, leaving nothing pending, when the transport refuses the send or the core is closed", async () => {
    const fake = new FakeCoreTransport({ synchronous: true }).echo(ECHO);
    const { core } = await boot(fake);
    fake.closed = true; // the transport throws UndraTransportError("closed") from send
    const refused = await core.call(FREE, ECHO, bytes()).catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(UndraTransportError);
    expect((await core.stats()).pendingCalls).toBe(0);
    fake.closed = false;
    core.close();
    const closed = await core.call(FREE, ECHO, bytes()).catch((e: unknown) => e);
    expect(closed).toBeInstanceOf(UndraTransportError);
  });

  it("still takes a signal: an already aborted one sends nothing, a later abort cancels the call in flight", async () => {
    const fake = new FakeCoreTransport({ synchronous: true }).on(SLOW, (_call, respond) => respond.defer());
    const { core } = await boot(fake);
    const aborted = AbortSignal.abort(new Error("no"));
    await expect(core.call(FREE, SLOW, bytes(), aborted)).rejects.toThrow("no");
    expect(fake.calls).toHaveLength(0);
    const controller = new AbortController();
    const inFlight = core.call(FREE, SLOW, bytes(), controller.signal).catch((e: unknown) => e);
    controller.abort(new Error("stop"));
    expect(((await inFlight) as Error).message).toBe("stop");
    expect(fake.cancelled).toEqual([fake.calls[0]?.callId]);
  });

  it("applies the change-sets that arrived before the reply before the caller resumes", async () => {
    const fake = new FakeCoreTransport({ synchronous: true });
    const { core } = await boot(fake);
    const applied: number[] = [];
    core.mirror.register(5n, (_signalId, _op, value) => applied.push(value[0] as number));
    fake.on(ECHO, (_call, respond) => {
      fake.emitChangeSet([{ handle: 5n, signalId: 0, op: 0, value: bytes(42) }]);
      respond.ok(bytes(1));
    });
    await core.call(FREE, ECHO, bytes());
    expect(applied).toEqual([42]);
  });

  it("answers a call made while the onError handler runs through the ordinary path (its failure is only logged)", async () => {
    const errors: unknown[] = [];
    const fake = new FakeCoreTransport({ synchronous: true }).on(ECHO, (_call, respond) => respond.badRequest("no"));
    const { core } = await boot(fake, { onError: (e) => errors.push(e) });
    core.report(new Error("boom"), "test");
    await core.call(FREE, ECHO, bytes()).catch(() => undefined);
    expect(errors).toHaveLength(1);
  });
});

describe("the call payload", () => {
  const handles = [0n, 1n, 0x0000_0001_0000_0007n, 0xffff_ffff_ffff_ffffn, 0x8000_0000_0000_0000n, 0x1_0000_0000n, 0xdead_beef_cafe_f00dn];
  const methodIds = [0, 1, 0xdead_beef, 0xffff_ffff];

  it("is encodeCall's, byte for byte, for a method on any handle and a free function (the joined payload of send)", async () => {
    const fake = new FakeCoreTransport({ synchronous: true }).echo(ECHO);
    const { core } = await boot(fake);
    for (const handle of handles) {
      for (const methodId of methodIds) {
        await core.call({ target: CallTarget.ObjectMethod, handle }, methodId, bytes(1, 2, 3)).catch(() => undefined); // an unknown method is refused: the payload is what is checked
        const sent = fake.sent.at(-1) as { kind: Kind; payload: Uint8Array };
        const callId = (fake.calls.at(-1) as { callId: number }).callId;
        expect(sent.payload).toEqual(encodeCall({ target: CallTarget.ObjectMethod, handle, methodId, callId, args: bytes(1, 2, 3) }));
      }
    }
    await core.call(FREE, 5, bytes()).catch(() => undefined);
    await core.call(CallTarget.FreeFunction, 6, bytes(9, 9)).catch(() => undefined);
    const [a, b] = fake.sent.slice(-2);
    expect(decodeCall((a as { payload: Uint8Array }).payload)).toMatchObject({ target: CallTarget.FreeFunction, methodId: 5 });
    expect(decodeCall((b as { payload: Uint8Array }).payload)).toMatchObject({ target: CallTarget.FreeFunction, methodId: 6 });
  });

  it("is the same through sendCall: the reused header and the arguments make encodeCall's payload", async () => {
    const fake = new PartsTransport({ synchronous: true }).echo(ECHO);
    const { core } = await boot(fake);
    // More handles than the cache of recent ones holds, in an order that evicts and revisits.
    for (const handle of [...handles, ...handles.slice(2), handles[3] as bigint]) {
      await core.call({ target: CallTarget.ObjectMethod, handle }, 0xdead_beef, bytes(1, 2, 3, 4)).catch(() => undefined);
      const callId = (fake.calls.at(-1) as { callId: number }).callId;
      expect(fake.joined.at(-1)).toEqual(
        encodeCall({ target: CallTarget.ObjectMethod, handle, methodId: 0xdead_beef, callId, args: bytes(1, 2, 3, 4) }),
      );
    }
    // A free function after a method: the header's handle and target bytes are cleared.
    await core.call(FREE, 3, bytes()).catch(() => undefined);
    expect(fake.joined.at(-1)).toEqual(
      encodeCall({ target: CallTarget.FreeFunction, methodId: 3, callId: (fake.calls.at(-1) as { callId: number }).callId, args: bytes() }),
    );
    expect(fake.heads.every((head) => head.length === 17)).toBe(true);
  });

  it("is the same through callSyncParts, and callSync still returns the body and applies the change-sets", async () => {
    const fake = new PartsTransport({ synchronous: true }).echo(ECHO);
    const { core } = await boot(fake);
    const handle = 0x0000_0002_0000_0009n;
    const body = core.callSync({ target: CallTarget.ObjectMethod, handle }, ECHO, bytes(8, 7, 6));
    expect(body).toEqual(bytes(8, 7, 6));
    const joined = fake.joined.at(-1) as Uint8Array;
    expect(decodeCall(joined)).toMatchObject({ target: CallTarget.ObjectMethod, handle, methodId: ECHO, args: bytes(8, 7, 6) });
    expect(joined).toEqual(encodeCall({ target: CallTarget.ObjectMethod, handle, methodId: ECHO, callId: (decodeCall(joined) as { callId: number }).callId, args: bytes(8, 7, 6) }));
  });

  it("refuses a handle or a method id that does not fit, as encodeCall does, and sends nothing", async () => {
    const fake = new FakeCoreTransport({ synchronous: true }).echo(ECHO);
    const { core } = await boot(fake);
    await expect(core.call({ target: CallTarget.ObjectMethod, handle: -1n }, ECHO, bytes())).rejects.toThrow(/u64 out of range/);
    await expect(core.call({ target: CallTarget.ObjectMethod, handle: 1n << 64n }, ECHO, bytes())).rejects.toThrow(/u64 out of range/);
    await expect(core.call(FREE, -1, bytes())).rejects.toThrow(/u32 out of range/);
    await expect(core.call(FREE, 2 ** 32, bytes())).rejects.toThrow(/u32 out of range/);
    expect(fake.sent.filter((s) => s.kind === Kind.Call)).toHaveLength(0);
    expect((await core.stats()).pendingCalls).toBe(0);
  });
});
