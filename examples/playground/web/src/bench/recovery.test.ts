import { describe, expect, it } from "vitest";
import { COUNTERS, RECOVERY_FULL, RECOVERY_QUICK, TODOS, titleOf } from "./recovery";

describe("the recovery bench's workload (ADR-049)", () => {
  it("is about 100 KB of to-dos and 52 observed signals", () => {
    // A to-do on the wire: a 16-byte id, a 4-byte length and the title, a 1-byte flag.
    const bytes = TODOS * (16 + 4 + titleOf(0).length + 1);
    expect(bytes).toBeGreaterThanOrEqual(100_000);
    expect(bytes).toBeLessThan(110_000);
    expect(new Set(Array.from({ length: TODOS }, (_, i) => titleOf(i))).size).toBe(TODOS);
    expect(titleOf(999)).toHaveLength(79);
    expect(4 + 3 * COUNTERS).toBe(52);
  });

  it("has a quick run that is a fraction of the full one", () => {
    expect(RECOVERY_QUICK.restarts).toBeLessThan(RECOVERY_FULL.restarts);
    expect(RECOVERY_QUICK.snapshotBatches).toBeLessThan(RECOVERY_FULL.snapshotBatches);
  });
});
