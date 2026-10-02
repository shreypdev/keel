import { describe, expect, it } from "vitest";
import { UndraError } from "../src/base-error.js";
import { type CallbackInterface, callbackGone, callbacks, giveBack, lend, lending } from "../src/callbacks.js";
import { UndraCore } from "../src/core.js";
import { UndraPortError, UndraReplyError, UndraTransportError } from "../src/errors.js";
import type { UndraUnhandledError } from "../src/call-error.js";
import { ChangeOp, PortStatus, ReplyStatus, UndraWriter, codecs, decodeValue, encodeValue } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { macrotask, track } from "./support/harness.js";
import { CounterStore, str, u32, vecU32 } from "./support/store.js";

// ADR-041 on the host: the registry (one reference per crossing, interned by object identity, given back by
// `__release`), the bridge that only queues, `main` delivery through the mirror in arrival order with the
// change-sets, `coalesce`, `background` delivery, the async answer (value, typed error, unavailable after a
// report), `__cancel`, giving references back for a call that never reached the core, and the weak target.

const PORT = 0x7000_0001;
const PROGRESS = 0x10;
const NOTE = 0x11;
const ASK = 0x12;
const RELEASE = 0x1e;
const CANCEL = 0x1f;

class AskError extends UndraError {
  constructor(message: string) {
    super("ask", message);
  }
}

/** A callback interface, written the way the generated bridge writes one. */
interface Listener {
  progress(done: number): void;
  note(line: string): void;
  ask(question: string, signal: AbortSignal): Promise<boolean>;
}

function bridge(background = false): CallbackInterface<Listener> {
  return {
    name: "Listener",
    portId: background ? PORT + 1 : PORT,
    releaseInstance: RELEASE,
    cancelCall: CANCEL,
    ...(background && { background: true }),
    methods: {
      [PROGRESS]: {
        name: "progress",
        coalesce: true,
        notify: (r) => {
          const done = r.readU32();
          return (impl) => impl.progress(done);
        },
      },
      [NOTE]: {
        name: "note",
        notify: (r) => {
          const line = r.readStr();
          return (impl) => impl.note(line);
        },
      },
      [ASK]: {
        name: "ask",
        call: (r) => {
          const question = r.readStr();
          return async (impl, signal) => {
            try {
              return encodeValue(codecs.bool, await impl.ask(question, signal));
            } catch (error) {
              if (error instanceof AskError) throw new UndraPortError(encodeValue(codecs.string, error.message));
              throw error;
            }
          };
        },
      },
    },
  };
}

const Main = bridge();
const Background = bridge(true);

/** A listener that records, with the store's observed count at each call. */
class Recorder implements Listener {
  readonly heard: string[] = [];
  answer: boolean | "typed" | "bug" | "wait" = true;
  aborted = false;
  constructor(private readonly store?: CounterStore) {}
  progress(done: number): void {
    this.heard.push(`progress ${done}`);
  }
  note(line: string): void {
    this.heard.push(this.store === undefined ? `note ${line}` : `note ${line} @${this.store.count.peek()}`);
  }
  ask(question: string, signal: AbortSignal): Promise<boolean> {
    this.heard.push(`ask ${question}`);
    if (this.answer === "typed") return Promise.reject(new AskError("no way"));
    if (this.answer === "bug") return Promise.reject(new TypeError("broken"));
    if (this.answer === "wait") {
      return new Promise((_, reject) => {
        signal.addEventListener("abort", () => {
          this.aborted = true;
          reject(signal.reason);
        });
      });
    }
    return Promise.resolve(this.answer);
  }
}

async function setup(options: { mode?: string } = {}) {
  const fake = new FakeCoreTransport(options.mode === undefined ? {} : { mode: options.mode });
  const reports: UndraUnhandledError[] = [];
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log: { log() {} }, http: null, timer: null },
      onError: (error) => reports.push(error),
    }),
  );
  return { fake, core, reports };
}

/** The arguments of a call of the core into `instance`. */
function args(instance: bigint, write: (w: UndraWriter) => void = () => {}): Uint8Array {
  const w = new UndraWriter();
  w.writeU64(instance);
  write(w);
  return w.finish();
}

describe("the registry", () => {
  it("interns by object identity, counts one reference per crossing, and frees at zero", async () => {
    const { core } = await setup();
    const registry = callbacks(core);
    const one = new Recorder();
    const two = new Recorder();
    const a = lend(core, one, Main);
    expect(lend(core, one, Main)).toBe(a);
    const b = lend(core, two, Main);
    expect(b).not.toBe(a);
    expect([registry.count(one), registry.count(two), registry.liveCount]).toEqual([2, 1, 2]);
    giveBack(core, a);
    registry.release(a);
    expect(registry.count(one)).toBe(0);
    expect(registry.liveCount).toBe(1);
    expect(lend(core, one, Main), "a fresh handle, never reused").not.toBe(a);
  });

  it("reports an over-release and never frees early", async () => {
    const { core, reports } = await setup();
    const registry = callbacks(core);
    const listener = new Recorder();
    const instance = lend(core, listener, Main);
    registry.release(instance);
    registry.release(instance);
    expect(reports).toHaveLength(1);
    expect(reports[0]?.operation).toBe("__release");
  });

  it("registers one bridge per interface, the first time one of its instances is lent", async () => {
    const { fake, core } = await setup();
    expect(await fake.notifyPort(PORT, NOTE, args(1n, (w) => w.writeStr("x")))).toBe("unavailable");
    lend(core, new Recorder(), Main);
    lend(core, new Recorder(), Main);
    expect(await fake.notifyPort(PORT, NOTE, args(99n, (w) => w.writeStr("x"))), "registered, but no such instance").toBe("async");
  });

  it("drops every entry when the connection is lost", async () => {
    const { fake, core } = await setup({ mode: "remote" });
    const listener = new Recorder();
    lend(core, listener, Main);
    fake.fail();
    await macrotask();
    expect(callbacks(core).liveCount).toBe(0);
  });
});

describe("main delivery", () => {
  it("only queues in the core's callback, and runs in arrival order with the change-sets at the drain", async () => {
    const { fake, core } = await setup();
    fake.store(0x40n, new Map([[0, u32(0)], [1, str("")], [2, vecU32([])]]));
    const store = await CounterStore.create(core, 0x40n);
    const listener = new Recorder(store);
    const instance = lend(core, listener, Main);
    const delivered = core.mirror.stats().callbacksDelivered;
    fake.burst(() => {
      fake.emitChangeSet([{ handle: 0x40n, signalId: 0, op: ChangeOp.FullValue, value: u32(1) }]);
      void fake.notifyPort(PORT, NOTE, args(instance, (w) => w.writeStr("one")));
      fake.emitChangeSet([{ handle: 0x40n, signalId: 0, op: ChangeOp.FullValue, value: u32(2) }]);
      void fake.notifyPort(PORT, NOTE, args(instance, (w) => w.writeStr("two")));
      fake.emitChangeSet([{ handle: 0x40n, signalId: 0, op: ChangeOp.FullValue, value: u32(3) }]);
    });
    await macrotask();
    expect(listener.heard, "each note sees the stores as they were when the core called it").toEqual(["note one @1", "note two @2"]);
    expect(store.count.peek()).toBe(3);
    expect(core.mirror.stats().callbacksDelivered - delivered).toBe(2);
  });

  it("keeps only the newest pending invocation of a coalesce method per instance", async () => {
    const { fake, core } = await setup();
    const listener = new Recorder();
    const other = new Recorder();
    const instance = lend(core, listener, Main);
    const second = lend(core, other, Main);
    fake.burst(() => {
      for (let i = 1; i <= 5; i++) {
        void fake.notifyPort(PORT, PROGRESS, args(instance, (w) => w.writeU32(i)));
        void fake.notifyPort(PORT, NOTE, args(instance, (w) => w.writeStr(`n${i}`)));
      }
      void fake.notifyPort(PORT, PROGRESS, args(second, (w) => w.writeU32(9)));
    });
    await macrotask();
    expect(listener.heard).toEqual(["note n1", "note n2", "note n3", "note n4", "progress 5", "note n5"]);
    expect(other.heard, "per instance").toEqual(["progress 9"]);
  });

  it("reports what a fire-and-forget implementation throws, and goes on", async () => {
    const { fake, core, reports } = await setup();
    const listener = new Recorder();
    listener.note = () => {
      throw new TypeError("the note broke");
    };
    const asyncThrow = { ...new Recorder(), progress: async () => Promise.reject(new Error("later")) } as unknown as Listener;
    const instance = lend(core, listener, Main);
    const later = lend(core, asyncThrow, Main);
    await fake.notifyPort(PORT, NOTE, args(instance, (w) => w.writeStr("x")));
    await fake.notifyPort(PORT, PROGRESS, args(later, (w) => w.writeU32(1)));
    await macrotask();
    expect(reports.map((r) => r.operation)).toEqual(["Listener.note", "Listener.progress"]);
  });

  it("gives a reference back when the queue reaches the core's __release", async () => {
    const { fake, core } = await setup();
    const listener = new Recorder();
    const instance = lend(core, listener, Main);
    lend(core, listener, Main);
    fake.burst(() => {
      void fake.notifyPort(PORT, NOTE, args(instance, (w) => w.writeStr("before")));
      void fake.notifyPort(PORT, RELEASE, args(instance));
      void fake.notifyPort(PORT, RELEASE, args(instance));
    });
    await macrotask();
    expect(listener.heard, "the invocation queued before the release ran").toEqual(["note before"]);
    expect(callbacks(core).liveCount).toBe(0);
  });
});

describe("async methods", () => {
  it("answer with the value, the typed error (status 1), or unavailable after a report (status 2)", async () => {
    const { fake, core, reports } = await setup();
    const listener = new Recorder();
    const instance = lend(core, listener, Main);
    const ok = await fake.callPort(PORT, ASK, args(instance, (w) => w.writeStr("q")));
    expect(ok.status).toBe(PortStatus.Ok);
    expect(decodeValue(codecs.bool, ok.body)).toBe(true);
    listener.answer = "typed";
    const typed = await fake.callPort(PORT, ASK, args(instance, (w) => w.writeStr("q")));
    expect(typed.status).toBe(PortStatus.Error);
    expect(decodeValue(codecs.string, typed.body)).toBe("no way");
    expect(reports).toEqual([]);
    listener.answer = "bug";
    const bug = await fake.callPort(PORT, ASK, args(instance, (w) => w.writeStr("q")));
    expect(bug.status).toBe(PortStatus.Unavailable);
    expect(reports.map((r) => r.operation), "reported once, under the callback's name").toEqual(["Listener.ask"]);
    // An instance the registry does not hold: unavailable, nothing to report.
    const gone = await fake.callPort(PORT, ASK, args(12345n, (w) => w.writeStr("q")));
    expect(gone.status).toBe(PortStatus.Unavailable);
    expect(reports).toHaveLength(1);
  });

  it("__cancel drops an invocation that has not started and aborts the signal of one that has", async () => {
    const { fake, core, reports } = await setup();
    const listener = new Recorder();
    listener.answer = "wait";
    const instance = lend(core, listener, Main);
    let dropped: Promise<unknown> | undefined;
    fake.burst(() => {
      const id = fake.nextPortCallId;
      dropped = fake.callPort(PORT, ASK, args(instance, (w) => w.writeStr("never")));
      void fake.notifyPort(PORT, CANCEL, args(instance, (w) => w.writeU32(id)));
    });
    await macrotask();
    expect(listener.heard, "the cancelled invocation never ran").toEqual([]);
    expect(((await dropped) as { status: number }).status).toBe(PortStatus.Unavailable);
    const id = fake.nextPortCallId;
    const running = fake.callPort(PORT, ASK, args(instance, (w) => w.writeStr("waiting")));
    await macrotask();
    expect(listener.heard).toEqual(["ask waiting"]);
    await fake.notifyPort(PORT, CANCEL, args(instance, (w) => w.writeU32(id)));
    expect(listener.aborted, "the implementation's signal aborted at once").toBe(true);
    expect((await running).status).toBe(PortStatus.Unavailable);
    expect(reports, "a cancelled call's abort is not a failure").toEqual([]);
  });

  it("answer unavailable without a report once a weak wrapper's target is gone", async () => {
    const { fake, core, reports } = await setup();
    const gone: Listener = { progress() {}, note() {}, ask: () => callbackGone() };
    const instance = lend(core, gone, Main);
    const reply = await fake.callPort(PORT, ASK, args(instance, (w) => w.writeStr("q")));
    expect(reply.status).toBe(PortStatus.Unavailable);
    expect(reports).toEqual([]);
  });
});

describe("background delivery", () => {
  it("runs from a microtask in call order, without the mirror's drain", async () => {
    const { fake, core } = await setup();
    const listener = new Recorder();
    const instance = lend(core, listener, Background);
    const delivered = core.mirror.stats().callbacksDelivered;
    fake.burst(() => {
      void fake.notifyPort(PORT + 1, NOTE, args(instance, (w) => w.writeStr("a")));
      void fake.notifyPort(PORT + 1, NOTE, args(instance, (w) => w.writeStr("b")));
    });
    const answer = fake.callPort(PORT + 1, ASK, args(instance, (w) => w.writeStr("c")));
    expect(decodeValue(codecs.bool, (await answer).body)).toBe(true);
    expect(listener.heard).toEqual(["note a", "note b", "ask c"]);
    expect(core.mirror.stats().callbacksDelivered).toBe(delivered);
  });
});

describe("lending", () => {
  const replyError = (status: ReplyStatus): UndraReplyError => new UndraReplyError(status, new Uint8Array(0));

  it("gives the references back for a call the core refused or never got, and leaves them otherwise", async () => {
    const { core } = await setup();
    const registry = callbacks(core);
    const listener = new Recorder();
    const send = (outcome: unknown, signal?: AbortSignal) =>
      lending(
        core,
        (lendIt) => {
          lendIt(listener, Main);
          lendIt(listener, Main);
          return Promise.reject(outcome);
        },
        signal,
      ).catch(() => undefined);
    await send(replyError(ReplyStatus.BadRequest));
    expect(registry.count(listener), "refused: status 5 transfers nothing").toBe(0);
    await send(new UndraTransportError("closed", "gone"));
    expect(registry.count(listener), "never reached the core").toBe(0);
    await send(replyError(ReplyStatus.Panic));
    expect(registry.count(listener), "the core owns what reached it").toBe(2);
    const aborted = new AbortController();
    aborted.abort();
    await send(aborted.signal.reason, aborted.signal);
    expect(registry.count(listener), "aborted before it was sent").toBe(2);
    const later = new AbortController();
    await lending(
      core,
      (lendIt) => {
        lendIt(listener, Main);
        later.abort();
        return Promise.reject(later.signal.reason);
      },
      later.signal,
    ).catch(() => undefined);
    expect(registry.count(listener), "aborted after it was sent: the core owns it").toBe(3);
    const failed = await lending(core, (lendIt) => {
      lendIt(listener, Main);
      throw new RangeError("an argument the wire cannot carry");
    }).catch((e: unknown) => e);
    expect(failed).toBeInstanceOf(RangeError);
    expect(registry.count(listener), "the encoding failed after the lend").toBe(3);
    expect(await lending(core, async (lendIt) => lendIt(listener, Main))).toBeTypeOf("bigint");
    expect(registry.count(listener)).toBe(4);
  });
});
