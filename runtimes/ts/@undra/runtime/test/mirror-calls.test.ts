import { describe, expect, it } from "vitest";
import { Mirror } from "../src/mirror.js";
import { ChangeOp, encodeChangeSet } from "../src/wire/index.js";

// ADR-041 decision 6 in the mirror: a host callback's invocation (`main` delivery) is queued with the change-sets in
// arrival order and never folded with them. The entries before it are folded and applied first, the entries after it
// after; a compaction keeps the order; `callbacksDelivered` counts what delivered.

const set = (handle: bigint, signalId: number, value: number): Uint8Array =>
  encodeChangeSet({ txnId: 1n, entries: [{ handle, signalId, op: ChangeOp.FullValue, value: Uint8Array.of(value) }] });

function recording(): { mirror: Mirror; seen: string[]; errors: unknown[] } {
  const seen: string[] = [];
  const errors: unknown[] = [];
  const mirror = new Mirror({ schedule: () => {}, onError: (e) => errors.push(e) });
  mirror.register(1n, (signalId, _op, value) => seen.push(`set ${signalId}=${value[0]}`));
  return { mirror, seen, errors };
}

describe("Mirror callback invocations", () => {
  it("runs each in its place: entries before it are applied first, entries after it are not folded into them", () => {
    const { mirror, seen } = recording();
    mirror.enqueue(set(1n, 0, 1));
    mirror.enqueue(set(1n, 0, 2));
    mirror.enqueueCall(() => (seen.push("call a"), true));
    mirror.enqueue(set(1n, 0, 3));
    mirror.enqueueCall(() => (seen.push("call b"), true));
    mirror.enqueue(set(1n, 0, 4));
    mirror.enqueue(set(1n, 0, 5));
    expect(seen).toEqual([]);
    mirror.flush();
    expect(seen).toEqual(["set 0=2", "call a", "set 0=3", "call b", "set 0=5"]);
    expect(mirror.stats().callbacksDelivered).toBe(2);
  });

  it("counts only the invocations that delivered, and reports what one throws", () => {
    const { mirror, seen, errors } = recording();
    mirror.enqueueCall(() => false);
    mirror.enqueueCall(() => {
      throw new Error("broken");
    });
    mirror.enqueueCall(() => (seen.push("after"), true));
    mirror.flush();
    expect(seen).toEqual(["after"]);
    expect(errors).toHaveLength(1);
    expect(mirror.stats().callbacksDelivered).toBe(1);
  });

  it("schedules a drain for an invocation that arrives alone", () => {
    const scheduled: Array<() => void> = [];
    const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn) });
    let ran = false;
    mirror.enqueueCall(() => (ran = true));
    expect(scheduled).toHaveLength(1);
    scheduled[0]?.();
    expect(ran).toBe(true);
  });

  it("runs an invocation queued by an invocation in a later round of the same drain", () => {
    const { mirror, seen } = recording();
    mirror.enqueueCall(() => {
      seen.push("outer");
      mirror.enqueue(set(1n, 0, 7));
      mirror.enqueueCall(() => (seen.push("inner"), true));
      return true;
    });
    mirror.flush();
    expect(seen).toEqual(["outer", "set 0=7", "inner"]);
  });

  it("keeps the order through a compaction of the backlog", () => {
    const seen: string[] = [];
    const mirror = new Mirror({ schedule: () => {}, maxPendingEntries: 4 });
    mirror.register(1n, (signalId, _op, value) => seen.push(`set ${signalId}=${value[0]}`));
    mirror.enqueue(set(1n, 0, 1));
    mirror.enqueue(set(1n, 0, 2));
    mirror.enqueueCall(() => (seen.push("call"), true));
    mirror.enqueue(set(1n, 0, 3));
    mirror.enqueue(set(1n, 0, 4));
    mirror.enqueue(set(1n, 0, 5));
    expect(mirror.stats().compactions).toBeGreaterThan(0);
    mirror.flush();
    expect(seen).toEqual(["set 0=2", "call", "set 0=5"]);
  });
});
