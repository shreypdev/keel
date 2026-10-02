import { describe, expect, it } from "vitest";
import { JobLog, describeStep } from "./workshop";

// The page's `Reporter`: what the core's calls back during a job do to the signals the Workshop tab renders.

describe("the job log (the page's Reporter)", () => {
  it("keeps the newest progress and the newest notes", () => {
    const log = new JobLog();
    expect(describeStep(log.step.peek())).toBe("no job yet");
    log.progress(1, 3);
    log.progress(3, 3);
    expect(describeStep(log.step.peek())).toBe("step 3 of 3");
    for (let i = 1; i <= 10; i++) log.note(`line ${i}`);
    expect(log.lines.peek()).toEqual(["line 3", "line 4", "line 5", "line 6", "line 7", "line 8", "line 9", "line 10"]);
    log.clear();
    expect([log.step.peek(), log.lines.peek()]).toEqual([null, []]);
  });

  it("asks, and answers the core with what the person chose", async () => {
    const log = new JobLog();
    const yes = log.confirm("go on?", new AbortController().signal);
    expect(log.asking.peek()?.text).toBe("go on?");
    log.asking.peek()?.answer(true);
    await expect(yes).resolves.toBe(true);
    expect(log.asking.peek()).toBeNull();
    const no = log.confirm("again?", new AbortController().signal);
    log.asking.peek()?.answer(false);
    await expect(no).resolves.toBe(false);
  });

  it("drops the question when the core stops waiting (the job was cancelled)", async () => {
    const log = new JobLog();
    const controller = new AbortController();
    const pending = log.confirm("go on?", controller.signal);
    controller.abort();
    await expect(pending).rejects.toBe(controller.signal.reason);
    expect(log.asking.peek()).toBeNull();
  });
});
