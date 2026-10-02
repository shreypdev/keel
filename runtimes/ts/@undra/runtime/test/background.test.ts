import { afterEach, describe, expect, it, vi } from "vitest";
import { BackgroundReportCodec } from "../src/adapters/codecs.js";
import { PortIds } from "../src/adapters/ids.js";
import type { AppState, LifecycleAdapter, UndraBackgroundReport } from "../src/adapters/types.js";
import { UndraCallError, UndraUnhandledError } from "../src/call-error.js";
import { UndraCore } from "../src/core.js";
import { fnv1a32 } from "../src/fnv.js";
import { AppStateCodec } from "../src/adapters/codecs.js";
import { CallTarget, ReplyStatus, decodeValue, encodeValue, codecs } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, macrotask, track } from "./support/harness.js";

/*
 * ADR-046 decision 3 on the TypeScript runtime: `runInBackground` over the standard function `run_background`, the stats
 * that say whether a window is worth asking for, and the web page's own window: when the page goes to the background the core is
 * told (`Lifecycle.Background`) and, if it has work to drain, runs for a second (within the page's life only).
 */

const RUN_BACKGROUND = 0x0e5b14ff;
const DONE: UndraBackgroundReport = { finished: true, replayed: 2, refetched: 1, stillPending: 0 };

afterEach(() => {
  vi.unstubAllGlobals();
});

interface Setup {
  readonly stats?: Record<string, unknown>;
  readonly lifecycle?: LifecycleAdapter;
  readonly backgroundRun?: boolean;
  readonly onError?: (error: UndraUnhandledError) => void;
  readonly mode?: string;
}

async function setup(options: Setup = {}) {
  const fake = new FakeCoreTransport({ mode: options.mode ?? "native" });
  const log = captureLog();
  let stats = options.stats ?? { live_handles: 0 };
  fake.stats = () => Promise.resolve(JSON.stringify(stats));
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: {
        log,
        http: null,
        timer: null,
        kv: null,
        secureStore: null,
        fs: null,
        connectivity: null,
        lifecycle: options.lifecycle ?? null,
      },
      ...(options.backgroundRun !== undefined && { backgroundRun: options.backgroundRun }),
      ...(options.onError && { onError: options.onError }),
    }),
  );
  return { fake, core, log, setStats: (next: Record<string, unknown>) => (stats = next) };
}

/** A lifecycle source the test drives. */
function scripted(): LifecycleAdapter & { emit(state: AppState): void } {
  const emitters = new Set<(state: AppState) => void>();
  return {
    subscribe(emit) {
      emitters.add(emit);
      return () => {
        emitters.delete(emit);
      };
    },
    emit(state) {
      for (const emit of emitters) emit(state);
    },
  };
}

/** Makes this look like a page: the runtime's own window only runs where there is a `document`. */
const inAPage = (): void => {
  vi.stubGlobal("document", { visibilityState: "visible", addEventListener() {}, removeEventListener() {} });
};

describe("the standard ids", () => {
  it("run_background is fn.run_background (the Kotlin and Swift runtimes hard-code the same)", () => {
    expect(fnv1a32("fn.run_background")).toBe(RUN_BACKGROUND);
  });
});

describe("runInBackground", () => {
  it("calls the free function with the deadline in milliseconds as a u64 and returns the decoded report", async () => {
    const { fake, core } = await setup();
    fake.on(RUN_BACKGROUND, (_call, r) => {
      r.ok(encodeValue(BackgroundReportCodec, DONE));
    });
    const report = await core.runInBackground(25_000);
    expect(report).toEqual(DONE);
    expect(fake.calls).toHaveLength(1);
    const call = fake.calls[0];
    expect(call).toMatchObject({ target: CallTarget.FreeFunction, methodId: RUN_BACKGROUND });
    expect(decodeValue(codecs.u64, (call as { args: Uint8Array }).args)).toBe(25_000n);
  });

  it("clamps what is not a deadline: negative, fractional, NaN and infinite", async () => {
    const { fake, core } = await setup();
    fake.on(RUN_BACKGROUND, (_call, r) => {
      r.ok(encodeValue(BackgroundReportCodec, DONE));
    });
    for (const ms of [-5, 1.9, Number.NaN, Number.POSITIVE_INFINITY]) await core.runInBackground(ms);
    const sent = fake.calls.map((c) => decodeValue(codecs.u64, (c as { args: Uint8Array }).args));
    expect(sent).toEqual([0n, 1n, 0n, BigInt(Number.MAX_SAFE_INTEGER)]);
  });

  it("aborting the signal cancels the call in the core and rejects with the signal's reason; the report never comes", async () => {
    const { fake, core } = await setup();
    fake.on(RUN_BACKGROUND, (_call, r) => {
      r.defer();
    });
    const abort = new AbortController();
    const pending = core.runInBackground(30_000, { signal: abort.signal });
    const outcome = pending.then(
      () => undefined,
      (e: unknown) => e,
    );
    await macrotask();
    abort.abort();
    const error = await outcome;
    expect(error).toBeInstanceOf(DOMException);
    expect((error as DOMException).name).toBe("AbortError");
    expect(fake.cancelled).toEqual([(fake.calls[0] as { callId: number }).callId]);
  });

  it("an already aborted signal sends nothing", async () => {
    const { fake, core } = await setup();
    const abort = new AbortController();
    abort.abort(new Error("gone"));
    await expect(core.runInBackground(1000, { signal: abort.signal })).rejects.toThrow("gone");
    expect(fake.calls).toEqual([]);
  });

  it("fails with an UndraCallError, never a raw error: unavailable when the core is closed, cancelled by the core, a panic, a refusal, a body that does not decode", async () => {
    const { fake, core } = await setup();
    const answers: Array<(r: Parameters<Parameters<FakeCoreTransport["on"]>[1]>[1]) => void> = [
      (r) => r.cancelled(),
      (r) => r.panic("kaboom", "bt"),
      (r) => r.badRequest("nope"),
      (r) => r.ok(new Uint8Array([2, 0, 0])),
      (r) => r.error(new Uint8Array([1])),
    ];
    let n = 0;
    fake.on(RUN_BACKGROUND, (_call, r) => (answers[n++] as (typeof answers)[number])(r));
    const kinds: string[] = [];
    for (let i = 0; i < answers.length; i++) {
      const error = await core.runInBackground(1000).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(error).toBeInstanceOf(UndraCallError);
      kinds.push((error as UndraCallError).kind);
    }
    expect(kinds).toEqual(["cancelledByCore", "panicked", "refused", "malformed", "malformed"]);
    core.close();
    const closed = await core.runInBackground(1000).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(closed).toBeInstanceOf(UndraCallError.Unavailable);
  });

  it("keeps the report's counts as numbers and `finished` as a boolean", async () => {
    const { fake, core } = await setup();
    fake.on(RUN_BACKGROUND, (_call, r) => {
      r.ok(encodeValue(BackgroundReportCodec, { finished: false, replayed: 0, refetched: 4_000_000_000, stillPending: 7 }));
    });
    expect(await core.runInBackground(1000)).toEqual({ finished: false, replayed: 0, refetched: 4_000_000_000, stillPending: 7 });
  });
});

describe("the stats of a core", () => {
  it("carry panicReports and the background counters of undra_stats_json", async () => {
    const { core } = await setup({
      stats: { live_handles: 3, panic_reports: 4, background: { tasks: 3, pending: 2, runs: 5, finished: 1, replayed: 6, refetched: 7 } },
    });
    const stats = await core.stats();
    expect(stats.panicReports).toBe(4);
    expect(stats.background).toEqual({ tasks: 3, pending: 2, runs: 5, finished: 1, replayed: 6, refetched: 7 });
    expect(stats.liveHandles).toBe(3);
  });

  it("are zero for a core that does not report them, and for a counter that is not a number", async () => {
    const { core, setStats } = await setup({ stats: { live_handles: 1 } });
    const bare = await core.stats();
    expect(bare.panicReports).toBe(0);
    expect(bare.background).toEqual({ tasks: 0, pending: 0, runs: 0, finished: 0, replayed: 0, refetched: 0 });
    setStats({ panic_reports: "many", background: { pending: null, tasks: 2 } });
    const odd = await core.stats();
    expect(odd.panicReports).toBe(0);
    expect(odd.background).toEqual({ tasks: 2, pending: 0, runs: 0, finished: 0, replayed: 0, refetched: 0 });
  });
});

describe("the page going to the background (ADR-046 decision 3.4)", () => {
  const lifecycleEvents = (fake: FakeCoreTransport): AppState[] =>
    fake.events.filter((e) => e.portId === PortIds.Lifecycle.portId).map((e) => decodeValue(AppStateCodec, e.payload));

  it("reports Lifecycle.Background and, with work pending, runs for one second without waiting for it", async () => {
    inAPage();
    const lifecycle = scripted();
    const { fake } = await setup({ lifecycle, stats: { background: { pending: 2 } } });
    fake.on(RUN_BACKGROUND, (_call, r) => {
      r.defer(); // the run is still going: nothing waits for it
    });
    lifecycle.emit("background");
    await macrotask();
    expect(lifecycleEvents(fake)).toEqual(["background"]);
    expect(fake.calls).toHaveLength(1);
    expect(decodeValue(codecs.u64, (fake.calls[0] as { args: Uint8Array }).args)).toBe(1000n);
  });

  it("starts nothing when no work is pending, when the page comes back, or on a state that is not the background", async () => {
    inAPage();
    const lifecycle = scripted();
    const { fake } = await setup({ lifecycle, stats: { background: { pending: 0 } } });
    fake.on(RUN_BACKGROUND, (_call, r) => r.ok(encodeValue(BackgroundReportCodec, DONE)));
    lifecycle.emit("background");
    lifecycle.emit("inactive");
    lifecycle.emit("active");
    await macrotask();
    expect(fake.calls).toEqual([]);
  });

  it("is one run at a time: a second hide while it runs does not start another", async () => {
    inAPage();
    const lifecycle = scripted();
    const { fake } = await setup({ lifecycle, stats: { background: { pending: 1 } } });
    let answer: (() => void) | undefined;
    fake.on(RUN_BACKGROUND, (call, r) => {
      answer = () => fake.reply((call as { callId: number }).callId, ReplyStatus.Ok, encodeValue(BackgroundReportCodec, DONE));
      r.defer();
    });
    lifecycle.emit("background");
    await macrotask();
    lifecycle.emit("background");
    await macrotask();
    expect(fake.calls).toHaveLength(1);
    answer?.();
    await macrotask();
    lifecycle.emit("background");
    await macrotask();
    expect(fake.calls).toHaveLength(2);
  });

  it("can be turned off: the Lifecycle report stays, the run does not happen", async () => {
    inAPage();
    const lifecycle = scripted();
    const { fake } = await setup({ lifecycle, backgroundRun: false, stats: { background: { pending: 3 } } });
    lifecycle.emit("background");
    await macrotask();
    expect(lifecycleEvents(fake)).toEqual(["background"]);
    expect(fake.calls).toEqual([]);
  });

  it("is for a page only: with no document (Node, a worker, React Native) nothing runs", async () => {
    const lifecycle = scripted();
    const { fake } = await setup({ lifecycle, stats: { background: { pending: 3 } } });
    lifecycle.emit("background");
    await macrotask();
    expect(lifecycleEvents(fake)).toEqual(["background"]);
    expect(fake.calls).toEqual([]);
  });

  it("hands a failure of the run to onError (once), and never into the page", async () => {
    inAPage();
    const lifecycle = scripted();
    const errors: UndraUnhandledError[] = [];
    const { fake } = await setup({ lifecycle, stats: { background: { pending: 1 } }, onError: (e) => errors.push(e) });
    fake.on(RUN_BACKGROUND, (_call, r) => r.panic("kaboom"));
    lifecycle.emit("background");
    await vi.waitFor(() => expect(errors).toHaveLength(1));
    await macrotask();
    expect(errors).toHaveLength(1);
    expect(errors[0]?.operation).toBe("runInBackground");
    expect(errors[0]?.error).toBeInstanceOf(UndraCallError.Panicked);
  });

  it("does not report the core that the app closed itself", async () => {
    inAPage();
    const lifecycle = scripted();
    const errors: UndraUnhandledError[] = [];
    const { fake, core } = await setup({ lifecycle, stats: { background: { pending: 1 } }, onError: (e) => errors.push(e) });
    fake.on(RUN_BACKGROUND, (_call, r) => r.defer());
    lifecycle.emit("background");
    core.close();
    await macrotask();
    expect(errors).toEqual([]);
  });
});
