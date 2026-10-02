import { UndraCoreRestarted, UndraTransportError } from "@undra/runtime";
import { describe, expect, it } from "vitest";
import { RECOVERY, RestartLog, describeRestart } from "./recovery-log";

/** A restart event as the runtime hands it to `onCoreRestarted`. */
function restarted(message: string, info: { restoredFromAgeMs: number | null; rejectedCalls: number; staleObjects: number }): UndraCoreRestarted {
  const report = { message, location: "", operation: "wasm-main: RuntimeError: unreachable", frames: [], schemaHash: 1n, trap: "RuntimeError: unreachable" };
  return new UndraCoreRestarted({ report, ...info }, new UndraTransportError("trap", "the wasm core trapped: unreachable"));
}

describe("the restart log of the debug panel", () => {
  it("records each restart, newest first, and keeps the last ten", () => {
    const log = new RestartLog();
    expect(log.entries.peek()).toEqual([]);
    log.record(restarted("first", { restoredFromAgeMs: 420, rejectedCalls: 1, staleObjects: 0 }), 1000);
    log.record(restarted("second", { restoredFromAgeMs: null, rejectedCalls: 0, staleObjects: 2 }), 2000);
    expect(log.entries.peek()).toEqual([
      { at: 2000, message: "second", restoredFromAgeMs: null, rejectedCalls: 0, staleObjects: 2 },
      { at: 1000, message: "first", restoredFromAgeMs: 420, rejectedCalls: 1, staleObjects: 0 },
    ]);
    for (let i = 0; i < 12; i++) log.record(restarted(`n${i}`, { restoredFromAgeMs: 1, rejectedCalls: 0, staleObjects: 0 }), 3000 + i);
    expect(log.entries.peek()).toHaveLength(10);
    expect(log.entries.peek()[0]?.message).toBe("n11");
  });

  it("says what came back and what was lost", () => {
    expect(describeRestart({ at: 0, message: "x", restoredFromAgeMs: 420, rejectedCalls: 1, staleObjects: 0 })).toBe(
      "stores restored from a snapshot 420 ms old; 1 call in flight failed; 0 objects went stale",
    );
    expect(describeRestart({ at: 0, message: "x", restoredFromAgeMs: null, rejectedCalls: 2, staleObjects: 1 })).toBe(
      "no snapshot to restore: the stores started empty; 2 calls in flight failed; 1 object went stale",
    );
  });

  it("turns recovery on with a snapshot a second at most and three restarts a minute", () => {
    expect(RECOVERY).toEqual({ snapshotEveryMs: 1000, maxRestarts: 3, perMs: 60_000 });
  });
});
