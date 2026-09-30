import { expect, test } from "vitest";
import { ChangeOp, Mirror, Signal, encodeChangeSet } from "@keel/runtime";
import { BigList, ListError } from "@playground/core";
import { boot } from "../src/harness.js";

// Defects found in the merged runtime while writing the scenarios. Each is a minimal repro that
// is written as the behaviour it should have and marked `fails`: it passes while the defect is
// there, and starts failing (telling whoever fixes it to delete the mark) once it is fixed.
// They are not scenarios: the reporter ignores them.

const fullValue = (n: number): Uint8Array =>
  encodeChangeSet({ txnId: 1n, entries: [{ handle: 1n, signalId: 0, op: ChangeOp.FullValue, value: Uint8Array.of(n) }] });

// FINDING ts-runtime/Mirror: `Mirror.flush` sets `#flushing` while it runs `batch(...)`, and the
// signal subscribers are notified when the batch ends, which is still inside that window. A
// subscriber that makes a core call whose change-set arrives synchronously (every synchronous
// core method in wasm-main) enqueues it while `#flushing` is true, so no flush is scheduled, and
// nothing looks at the queue again: the change-set waits for an unrelated one. The fix is to
// re-check the queue after `#flushing` is cleared.
test.fails("FINDING a change-set enqueued by a subscriber during the flush is applied", async () => {
  const mirror = new Mirror();
  const count = new Signal(0);
  const seen: number[] = [];
  mirror.register(1n, (_signalId, _op, value) => {
    count._set(value[0] as number);
  });
  count.subscribe((n) => {
    seen.push(n);
    if (n === 1) mirror.enqueue(fullValue(2));
  });
  mirror.enqueue(fullValue(1));
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(seen).toEqual([1, 2]);
});

// FINDING playground-core/BigList: `insert_at` checks the index against `len + 1` (an insert may
// append) and hands that number to `ListError::OutOfRange { len }`, so a refused `insert_at(10001)`
// on the 10,000-item list reports `len = 10001`, and its message says "outside a list of 10001
// items". scenarios.md S05.3 expects `len = 10000`. The fix is one line in biglist.rs: report
// `list.len()`, not the bound.
test.fails("FINDING BigList.insertAt out of range reports the length of the list", async () => {
  const { core } = await boot();
  const list = await BigList.create(core);
  const error = await list.insertAt(10_001, "x").catch((e: unknown) => e);
  expect(error).toBeInstanceOf(ListError.OutOfRange);
  expect(error).toMatchObject({ index: 10_001, len: 10_000 });
});
