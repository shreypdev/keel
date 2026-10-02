import { describe, expect, it } from "vitest";
import { UndraCore } from "../src/core.js";
import { UndraReplyError, UndraTransportError } from "../src/errors.js";
import { CallTarget, Kind, ReplyStatus } from "../src/wire/index.js";
import { FakeCoreTransport, type Responder, SCHEMA } from "./support/fake-core.js";
import { macrotask, track } from "./support/harness.js";
import { CounterStore, u32 } from "./support/store.js";

/*
 * The reviewer's attack on ADR-056's direct call (a call on a core that answers inside `send` returns an already
 * settled promise): the change-sets the core emitted before the reply are applied before the caller's continuation, for
 * every outcome, for a reply that is there when `send` returns and for one that comes later, with no frame pacer, with
 * one that never fires, and with one that fires on a timer; and a call made from a change-set listener (inside a drain).
 */

const SET = 0x61;
const HANDLE = 0x1_0000_0002n;
const METHOD = { target: CallTarget.ObjectMethod, handle: HANDLE } as const;

type Outcome = "ok" | "error" | "panic" | "cancelled" | "badRequest";
const OUTCOMES: readonly Outcome[] = ["ok", "error", "panic", "cancelled", "badRequest"];
const STATUS: Record<Outcome, ReplyStatus> = {
  ok: ReplyStatus.Ok,
  error: ReplyStatus.Error,
  panic: ReplyStatus.Panic,
  cancelled: ReplyStatus.Cancelled,
  badRequest: ReplyStatus.BadRequest,
};

function answer(respond: Responder, outcome: Outcome): void {
  switch (outcome) {
    case "ok":
      return respond.ok(u32(1));
    case "error":
      return respond.error(u32(2));
    case "panic":
      return respond.panic("boom");
    case "cancelled":
      return respond.cancelled();
    case "badRequest":
      return respond.badRequest("no");
  }
}

const pacers: Record<string, ((fn: () => void) => void) | undefined> = {
  "no frame ever comes": () => {},
  "a frame 20 ms later": (fn) => void setTimeout(fn, 20),
  "the default (animation frame or a timer)": undefined,
};

async function setup(synchronous: boolean, schedule: ((fn: () => void) => void) | undefined) {
  const fake = new FakeCoreTransport({ synchronous });
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log: { log() {} }, http: null, timer: null },
      ...(schedule !== undefined && { mirror: { schedule } }),
    }),
  );
  fake.store(HANDLE, new Map([[0, u32(1)]]));
  const store = await CounterStore.create(core, HANDLE);
  return { fake, core, store };
}

describe("a call's change-sets are applied before its continuation runs", () => {
  for (const synchronous of [true, false]) {
    for (const [pacer, schedule] of Object.entries(pacers)) {
      for (const outcome of OUTCOMES) {
        it(`${synchronous ? "in process" : "asynchronous"}, ${pacer}, status ${STATUS[outcome]} (${outcome}), reply with the call`, async () => {
          const { fake, core, store } = await setup(synchronous, schedule);
          fake.on(SET, (_call, respond) => {
            fake.setSignal(HANDLE, 0, u32(7));
            answer(respond, outcome);
          });
          let seen = -1;
          const result = await core.call(METHOD, SET, u32(7)).then(
            () => "resolved",
            (e: unknown) => {
              seen = store.count.peek();
              return e;
            },
          );
          if (outcome === "ok") {
            expect(result).toBe("resolved");
            seen = store.count.peek();
          } else {
            expect(result).toBeInstanceOf(UndraReplyError);
            expect((result as UndraReplyError).status).toBe(STATUS[outcome]);
          }
          expect(seen, "the continuation saw the change-set that came before the reply").toBe(7);
          expect((await core.stats()).pendingCalls).toBe(0);
        });
      }
    }
  }

  for (const [pacer, schedule] of Object.entries(pacers)) {
    for (const outcome of OUTCOMES) {
      it(`in process, ${pacer}, ${outcome}: a reply that comes after send returned (the core answers from a poll)`, async () => {
        const { fake, core, store } = await setup(true, schedule);
        let held: Responder | undefined;
        fake.on(SET, (_call, respond) => {
          held = respond;
          respond.defer();
        });
        let seen = -1;
        const pending = core.call(METHOD, SET, u32(9)).then(
          () => (seen = store.count.peek()),
          () => (seen = store.count.peek()),
        );
        await macrotask();
        expect(seen, "still waiting").toBe(-1);
        // What `undra_poll` does from a microtask: the change-set, then the reply, inside one export.
        fake.setSignal(HANDLE, 0, u32(9));
        answer(held as Responder, outcome);
        await pending;
        expect(seen).toBe(9);
      });
    }
  }
});

describe("a call made from a change-set listener (inside a drain)", () => {
  it("is not interleaved with the drain: the drain applies the call's change-set in its next round and the caller resumes after", async () => {
    const { fake, core, store } = await setup(true, () => {});
    fake.on(SET, (_call, respond) => {
      fake.setSignal(HANDLE, 0, u32(50));
      respond.ok();
    });
    const order: string[] = [];
    let called: Promise<unknown> | undefined;
    const stop = store.count.subscribe(() => {
      order.push(`listener(${store.count.peek()})`);
      if (store.count.peek() === 5 && called === undefined) {
        order.push("call");
        called = core.call(METHOD, SET, u32(50)).then(() => order.push(`resumed(${store.count.peek()})`));
        order.push("call returned");
      }
    });
    fake.setSignal(HANDLE, 0, u32(5));
    core.mirror.flush();
    await called;
    stop();
    expect(order).toEqual(["listener(5)", "call", "call returned", "listener(50)", "resumed(50)"]);
    expect(store.count.peek()).toBe(50);
    expect(core.mirror.pending).toBe(0);
  });
});

describe("a send that fails after the core already answered", () => {
  /** An in-process core that answers the call, then traps in the same export (the reply was delivered, `undra_call` did not return). */
  class RepliesThenTraps extends FakeCoreTransport {
    override send(kind: Kind, payload: Uint8Array): void {
      super.send(kind, payload);
      if (kind === Kind.Call) throw new UndraTransportError("trap", "the wasm core trapped after replying");
    }
  }

  it("keeps the answer, as a call with a promise always did (the reply is what the core said; the trap is reported by the transport)", async () => {
    const fake = new RepliesThenTraps({ synchronous: true }).on(SET, (_call, respond) => respond.ok(u32(5)));
    const core = track(
      await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: { log() {} }, http: null, timer: null } }),
    );
    const body = await core.call(METHOD, SET, u32(5)).then(
      (b) => b,
      (e: unknown) => e,
    );
    expect(body, "the reply that arrived before the throw stands").toEqual(u32(5));
    expect((await core.stats()).pendingCalls).toBe(0);
    // The call with a promise built before the send (a call that takes a signal) has always done this.
    const withSignal = await core.call(METHOD, SET, u32(5), new AbortController().signal).then(
      (b) => b,
      (e: unknown) => e,
    );
    expect(withSignal).toEqual(u32(5));
  });
});
