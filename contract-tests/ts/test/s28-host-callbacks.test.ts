import { expect, test } from "vitest";
import { UndraCallError, type UndraUnhandledError, callbacks, giveBack, lend } from "@undra/runtime";
import { ReportError, type Reporter, ReporterCallback, Watch, Workshop, weakReporter } from "@playground/core";
import { boot } from "../src/harness.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S28 host callbacks (ADR-041): the Workshop calls the app's Reporter back. Invocations arrive in order and after
// the change-sets committed before them, never inside the core's callback; async methods answer, fail with their
// typed error, are cancelled; the registry interns and counts; `coalesce` keeps the newest; a refused call leaves
// the registry unchanged; the registry holds implementations strongly, the weak wrapper does not.

/** The `gc` of a Node started with `--expose-gc` (vitest.config.ts passes it), or `undefined`. */
const gc = (globalThis as { gc?: () => void }).gc;

/** What `confirm` does: answer, fail with the typed error, throw something else, or wait until cancelled. */
type Answer = boolean | "declined-by-error" | "bug" | "wait";

/** A Reporter that records what it is called with, in order, with the workshop's observed `notes` at each call. */
class Recorder implements Reporter {
  readonly heard: Array<{ readonly call: string; readonly notes: number | undefined }> = [];
  readonly lines: string[] = [];
  readonly progressed: Array<[number, number]> = [];
  answer: Answer = true;
  /** Called from inside `note` (a call into the core from a callback). */
  onNote: ((line: string) => void) | undefined;
  throwOnNote = false;
  /** Whether a waiting `confirm` saw its signal abort, and when it started waiting. */
  aborted = false;
  waiting = false;

  constructor(private readonly workshop?: Workshop) {}

  progress(done: number, total: number): void {
    this.progressed.push([done, total]);
    this.heard.push({ call: `progress ${done}/${total}`, notes: this.workshop?.notes.peek() });
  }

  note(line: string): void {
    this.lines.push(line);
    this.heard.push({ call: `note ${line}`, notes: this.workshop?.notes.peek() });
    if (this.throwOnNote) throw new TypeError("the note handler broke");
    this.onNote?.(line);
  }

  confirm(question: string, signal: AbortSignal): Promise<boolean> {
    this.heard.push({ call: `confirm ${question}`, notes: this.workshop?.notes.peek() });
    const answer = this.answer;
    if (answer === "declined-by-error") return Promise.reject(new ReportError.Unavailable("x"));
    if (answer === "bug") return Promise.reject(new TypeError("the confirm handler broke"));
    if (answer === "wait") {
      this.waiting = true;
      return new Promise<boolean>((resolve) => {
        signal.addEventListener("abort", () => {
          this.aborted = true;
          // An answer that arrives late: the core discards it.
          setTimeout(() => resolve(true), 20);
        });
      });
    }
    return Promise.resolve(answer);
  }
}

test("S28 host callbacks", async () => {
  const { core, runtimeErrors } = await boot();
  const reported = (): UndraUnhandledError[] => runtimeErrors.splice(0) as UndraUnhandledError[];
  const registry = callbacks(core);
  const w = await Workshop.create(core);
  const rep = new Recorder(w);
  const watch = await w.watch(rep);

  await step("1. ordered, after the change-sets committed before them, never inside the core's callback", async () => {
    expect(watch).toBeInstanceOf(Watch);
    expect(await w.announce("one")).toBe(1);
    expect(await w.announce("two")).toBe(1);
    await waitFor("both notes", () => rep.lines.length === 2);
    expect(rep.heard).toEqual([
      { call: "note one", notes: 1 },
      { call: "note two", notes: 2 },
    ]);
    // A call into the core from inside the callback: queued and run, never E_REENTRANT.
    const fromNote: Array<Promise<number>> = [];
    rep.onNote = (line) => {
      if (line === "three") fromNote.push(w.announce("from the note"));
    };
    await w.announce("three");
    await waitFor("the note to call into the core", () => fromNote.length === 1);
    expect(await fromNote[0]).toBe(1);
    await waitFor("the note it caused", () => rep.lines.includes("from the note"));
    rep.onNote = undefined;
    expect(rep.lines).toEqual(["one", "two", "three", "from the note"]);
  });

  await step("2. an async callback returns a value and throws its typed error", async () => {
    rep.heard.length = 0;
    rep.lines.length = 0;
    rep.progressed.length = 0;
    rep.answer = true;
    expect(await w.run(3, rep)).toBe(3);
    // Progress is `coalesce`: the newest report of what one drain found; every note, in order.
    expect(rep.progressed.at(-1)).toEqual([3, 3]);
    for (let i = 1; i < rep.progressed.length; i++) expect(rep.progressed[i]?.[0]).toBeGreaterThan(rep.progressed[i - 1]?.[0] ?? 0);
    expect(rep.lines).toEqual(["step 1 of 3", "step 2 of 3", "step 3 of 3"]);
    expect(rep.heard.at(-1)?.call).toBe("confirm ran 3 steps; go on?");
    rep.answer = false;
    await expect(w.run(1, rep)).rejects.toBeInstanceOf(ReportError.Declined);
    rep.answer = "declined-by-error";
    const typed = await w.run(1, rep).catch((e: unknown) => e);
    expect(typed).toBeInstanceOf(ReportError.Unavailable);
    expect((typed as ReportError.Unavailable).value).toBe("x");
    expect(reported(), "a typed error is an answer, not a failure").toEqual([]);
  });

  await step("3. any other throw is reported, not propagated", async () => {
    rep.answer = "bug";
    const failed = await w.run(1, rep).catch((e: unknown) => e);
    expect(failed, "the core hears unavailable").toBeInstanceOf(ReportError.Unavailable);
    let reports = reported();
    expect(reports.length).toBe(1);
    expect(reports[0]?.operation).toBe("Reporter.confirm");
    expect(reports[0]?.cause).toBeInstanceOf(TypeError);
    rep.throwOnNote = true;
    expect(await w.announce("boom")).toBe(1);
    await waitFor("the note to be reported", () => runtimeErrors.length === 1);
    reports = reported();
    expect(reports[0]?.operation).toBe("Reporter.note");
    rep.throwOnNote = false;
  });

  await step("4. cancellation reaches the host task", async () => {
    rep.answer = "wait";
    rep.waiting = false;
    const live = registry.liveCount;
    const controller = new AbortController();
    const running = w.run(1, rep, controller.signal);
    running.catch(() => {});
    await waitFor("confirm to wait", () => rep.waiting);
    controller.abort();
    await expect(running).rejects.toBe(controller.signal.reason);
    await waitFor("the host's confirm to see its signal abort", () => rep.aborted, { timeoutMs: 1_000 });
    await sleep(50); // the late answer goes out, and is discarded
    expect(reported(), "nothing leaks").toEqual([]);
    expect(registry.liveCount, "the watch still holds the instance").toBe(live);
    expect(registry.count(rep)).toBeGreaterThan(0);
    rep.answer = true;
  });

  await step("5. interning and the registry", async () => {
    const w5 = await Workshop.create(core);
    const rep5 = new Recorder();
    const first = lend(core, rep5, ReporterCallback);
    const second = lend(core, rep5, ReporterCallback);
    expect(second, "the same listener is one instance handle").toBe(first);
    expect(registry.count(rep5)).toBe(2);
    giveBack(core, first);
    giveBack(core, second);
    expect(registry.count(rep5)).toBe(0);
    const live = registry.liveCount;
    const watch1 = await w5.watch(rep5);
    const watch2 = await w5.watch(rep5);
    await waitFor("the core to give the duplicate back", () => registry.count(rep5) === 1);
    expect(registry.liveCount - live).toBe(1);
    expect(await w5.watching(), "two subscriptions, one proxy").toBe(2);
    watch1.close();
    watch2.close();
    await waitFor("the registry to let go of the reporter", () => registry.count(rep5) === 0, { timeoutMs: 1_000 });
    expect(registry.liveCount).toBe(live);
  });

  await step("6. coalesce delivers only the newest per drain", async () => {
    const rep6 = new Recorder();
    const delivered = core.mirror.stats().callbacksDelivered;
    await w.burst(50, rep6);
    expect(rep6.progressed).toEqual([[50, 50]]);
    expect(rep6.lines).toEqual(Array.from({ length: 50 }, (_, i) => `burst ${i + 1}`));
    expect(core.mirror.stats().callbacksDelivered - delivered, "50 notes and one progress, never folded").toBe(51);
  });

  await step("7. a refused call leaves the registry unchanged", async () => {
    const stale = await Workshop.create(core);
    stale.close();
    const rep2 = new Recorder();
    await expect(stale.watch(rep2)).rejects.toBeInstanceOf(UndraCallError.Refused);
    expect(registry.count(rep2)).toBe(0);
    await sleep(50);
    expect(reported(), "no __release came for it (an over-release would be reported)").toEqual([]);
  });

  await step("8. strong by default, with the weak wrapper available", async () => {
    const w8 = await Workshop.create(core);
    const heard: string[] = [];
    await w8.watch({
      progress: () => {},
      note: (line) => heard.push(line),
      confirm: () => Promise.resolve(true),
    });
    gc?.();
    await sleep(10);
    await w8.announce("still there");
    await waitFor("the inline reporter to hear it", () => heard.length === 1);
    expect(heard).toEqual(["still there"]);

    const w9 = await Workshop.create(core);
    const lines: string[] = [];
    const watchWeak = await (async () => {
      const target = new Recorder();
      target.onNote = (line) => lines.push(line);
      const watched = await w9.watch(weakReporter(target));
      await w9.announce("while alive");
      await waitFor("the target to hear it", () => target.lines.length === 1);
      return watched;
    })();
    expect(lines).toEqual(["while alive"]);
    if (gc !== undefined) {
      // The target is unreachable now: once collected, the wrapper does nothing.
      await waitFor("the weak target to be collected", async () => {
        gc();
        await sleep(10);
        lines.length = 0;
        await w9.announce("after");
        await sleep(10);
        return lines.length === 0;
      });
    }
    watchWeak.close();
  });

  await step("10. a stream takes a callback: held by the core while it runs, given back when it ends or is refused", async () => {
    const rep10 = new Recorder();
    const walked: number[] = [];
    for await (const stepNumber of w.walk(3, rep10)) walked.push(stepNumber);
    expect(walked).toEqual([1, 2, 3]);
    await waitFor("the notes of walk(3)", () => rep10.lines.length === 3);
    expect(rep10.lines).toEqual(["walk 1 of 3", "walk 2 of 3", "walk 3 of 3"]);
    await waitFor("the registry to let go of the walk's reporter", () => registry.count(rep10) === 0, { timeoutMs: 1_000 });
    const closed = await Workshop.create(core);
    closed.close();
    const rep10b = new Recorder();
    const refused = (async () => {
      for await (const _ of closed.walk(1, rep10b)) void _;
    })();
    await expect(refused).rejects.toBeInstanceOf(UndraCallError.Refused);
    expect(registry.count(rep10b), "the refused stream's reporter is not in the registry").toBe(0);
    await sleep(50);
    expect(reported(), "no __release came for it (an over-release would be reported)").toEqual([]);
  });

  await step("9. a background callback is not delivered through the drain", () => {
    // The playground has no background-delivery interface (scenarios.md); the golden case `callbacks` and the
    // runtime's own tests cover `background`.
    expect(ReporterCallback.background).toBeUndefined();
  });
});
