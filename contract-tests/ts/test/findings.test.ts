import { expect, test } from "vitest";
import { ChangeOp, Mirror, Signal, encodeChangeSet } from "@keel/runtime";
import { RemoteTodosQueryHandle, configureRemote, createRemoteTodo, setRemoteDone } from "@playground/core";
import { BASE_URL, boot } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { waitFor } from "../src/wait.js";

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

// FINDING keel-query/optimistic rollback: a failed mutation restores the snapshot of the cache
// entry it took before it ran (SPEC 9: "the pre-mutation entries are restored"). A mutation that
// started after it, on the same list, has put its own optimistic item into that entry by then, and
// the restore takes the item away again while its request is still in flight (offline, while it is
// queued, the item is gone until the replay succeeds). The playground web app hits it by turning
// Offline on while a toggle's PATCH is still in flight and then adding an item. Expected: the later
// mutation's placeholder survives the earlier one's rollback.
test.fails("FINDING the rollback of one mutation keeps the placeholder of a later one", async () => {
  const { core, server } = await boot();
  await configureRemote({ baseUrl: BASE_URL }, core);
  const url = `${BASE_URL}/lists/find/todos`;
  server.on("GET", url, replies.json(200, [{ id: 1, title: "Buy milk", done: false }]));
  server.on("PATCH", `${url}/1`, replies.json(500, { error: "no" }, { delayMs: 100 }));
  server.on("POST", url, replies.json(201, { id: 2, title: "Walk", done: false }, { delayMs: 600 }));
  const handle = await RemoteTodosQueryHandle.create("find", core);
  await waitFor("the first list", () => handle.data.peek() !== null);

  const toggled = setRemoteDone("find", 1, true, core).catch(() => undefined); // fails after 100 ms
  const created = createRemoteTodo("find", "Walk", core); // starts at once, answered after 600 ms
  created.catch(() => undefined); // the test may fail before it is awaited, and the core is closed under it then
  await toggled;
  const titles = handle.data.peek()?.map((todo) => todo.title);
  expect(titles, "the item that is still being created").toContain("Walk");
  await created;
});
