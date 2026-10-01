import { expect, test } from "vitest";
import { HttpError, UndraReader, codecs, emitConnectivity } from "@undra/runtime";
import {
  type RemoteTodo,
  RemoteError,
  RemoteTodosQueryHandle,
  configureRemote,
  createRemoteTodo,
  saveNote,
  setRemoteDone,
  storageStatus,
  tagNote,
} from "@playground/core";
import { BASE_URL, boot } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { MemoryKv, Persisted } from "../src/memory-kv.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S14 offline queue replay: while the device is offline an idempotent mutation keeps waiting
// (queued, persisted, its placeholder on show) and is replayed, with the same Idempotency-Key,
// when the network returns; a mutation that is not idempotent fails at once. Steps 7 to 9 (ADR-037):
// what build A queued is read by build B, loaded in this same process over a copy of build A's Kv:
// `save_note` migrates by parameter name and replays with its key, `tag_note` is a dead letter.

const LIST = "s14";
const URL = `${BASE_URL}/lists/${LIST}/todos`;
const PATCH_URL = `${URL}/1`;
const NOTES = `${BASE_URL}/lists/s14m/notes`;
const QUEUE_KEY = Persisted.queueKey;
const created: RemoteTodo = { id: 9, title: "Offline item", done: false };
const isPlaceholder = (todo: RemoteTodo): boolean => todo.id >= 0x8000_0000;

/**
 * What a persisted queue holds (format 2 of ADR-037): `format u16 = 2, schema_hash u64, count u32, count x { mutation_id
 * u32, fingerprint u64, params bytes, idempotency_key Uuid }`.
 */
function decodeQueue(bytes: Uint8Array): { readonly keys: string[]; readonly fingerprints: bigint[] } {
  const r = new UndraReader(bytes);
  expect(r.readU16(), "the queue's format").toBe(2);
  r.readU64();
  const count = r.readU32();
  const keys: string[] = [];
  const fingerprints: bigint[] = [];
  for (let i = 0; i < count; i++) {
    r.readU32();
    fingerprints.push(r.readU64());
    codecs.bytes.decode(r);
    keys.push(r.readUuid());
  }
  r.finish();
  return { keys, fingerprints };
}

test("S14 offline queue replay", async () => {
  const { core, server, kv } = await boot();
  await configureRemote({ baseUrl: BASE_URL }, core);
  server.on("GET", URL, replies.json(200, []));
  await waitFor("the offline queue to be readable", async () => (await storageStatus(core)).queueReadable);
  const handle = await RemoteTodosQueryHandle.create(LIST, core);
  await waitFor("the first list", () => handle.data.peek() !== null);

  await step("1. the device goes offline", async () => {
    emitConnectivity(core, false, "none");
    await sleep(50);
  });

  // The create call, and what becomes of it; its rejection must never go unobserved.
  let outcome: { readonly value: RemoteTodo } | { readonly error: unknown } | undefined;
  server.on("POST", URL, replies.networkError("offline"));
  const pending = createRemoteTodo(LIST, "Offline item", core).then(
    (value) => {
      outcome = { value };
    },
    (error: unknown) => {
      outcome = { error };
    },
  );

  await step("2. an idempotent create does not finish while offline", async () => {
    await waitFor("the placeholder in the list", () => handle.data.peek()?.some(isPlaceholder) === true);
    await sleep(200);
    expect(outcome, "still pending after 200 ms").toBeUndefined();
    expect(server.count("POST", URL), "the server saw exactly one POST").toBe(1);
    expect(handle.data.peek()?.map((todo) => todo.title)).toEqual(["Offline item"]);
    expect(handle.data.peek()?.every(isPlaceholder)).toBe(true);
  });

  await step("3. a mutation that is not idempotent fails at once", async () => {
    server.on("PATCH", PATCH_URL, replies.networkError("offline"));
    const started = performance.now();
    const error = await setRemoteDone(LIST, 1, true, core).then(
      () => {
        throw new Error("expected the PATCH to fail, but it succeeded");
      },
      (e: unknown) => e,
    );
    expect(performance.now() - started, "at once, not after a retry's backoff").toBeLessThan(1_000);
    expect(error).toBeInstanceOf(RemoteError.Http);
    const cause = (error as RemoteError.Http).cause;
    expect(cause).toBeInstanceOf(HttpError.Network);
    expect(cause).toMatchObject({ kind: "network", value: "offline" });
    expect(outcome, "the queued create is still waiting").toBeUndefined();
  });

  // What the Kv saw while offline, for step 6.
  const whileOffline = kv.operations.slice();

  await step("4. the network returns", () => {
    server.on("POST", URL, replies.json(201, created));
    server.on("GET", URL, replies.json(200, [created]));
    emitConnectivity(core, true, "wifi");
  });

  await step("5. the pending create resolves; the POST was repeated with the same Idempotency-Key", async () => {
    await waitFor("the queued create to finish", () => outcome !== undefined);
    await pending;
    expect(outcome).toEqual({ value: created });
    expect(server.count("POST", URL), "the original attempt and one replay").toBe(2);
    const keys = server.requestsTo("POST", URL).map((request) => request.header("Idempotency-Key"));
    expect(keys[0]).toMatch(/^[0-9a-f-]{36}$/);
    expect(keys[1]).toBe(keys[0]);
    await waitFor("the list to hold the server's item", () => {
      const data = handle.data.peek();
      return data?.length === 1 && data[0]?.id === 9;
    });
    expect(handle.data.peek()).toEqual([created]);
  });

  await step("6. the queue was persisted while offline (format 2, after the description of its input), and emptied after the replay", async () => {
    const first = whileOffline.findIndex(
      (operation) => operation.key === QUEUE_KEY && operation.op === "set" && operation.failure === undefined && (Persisted.queueCount(operation.value) ?? 0) > 0,
    );
    expect(first, `a write of ${QUEUE_KEY} while offline: ${whileOffline.map((o) => `${o.op} ${o.key}`).join(", ")}`).toBeGreaterThanOrEqual(0);
    const queued = whileOffline[first]?.value as Uint8Array;
    expect(Persisted.queueCount(queued), "the count of the queue written while offline").toBe(1);
    const { keys, fingerprints } = decodeQueue(queued);
    const sent = server.requestsTo("POST", URL)[0]?.header("Idempotency-Key");
    expect(keys[0], "the queue holds the key both POSTs carried").toBe(sent);
    const typesKey = Persisted.typesKey(fingerprints[0] as bigint);
    expect(
      whileOffline.slice(0, first).some((operation) => operation.key === typesKey && operation.op === "set" && operation.failure === undefined),
      `${typesKey} (the description of create's input) was written before the queue that needs it`,
    ).toBe(true);
    await waitFor("the queue to be emptied", () => {
      const last = kv.writesTo(QUEUE_KEY).at(-1);
      return last !== undefined && (last.op === "delete" || Persisted.queueCount(last.value) === 0);
    });
  });

  // Step 7: build A queues what build B changes. The notes stay pending in build A, whose core is closed after the
  // scenario (their rejections are observed here).
  let handedOver = new Map<string, Uint8Array>();
  let idempotencyKey = "";
  const notes: Array<Promise<unknown>> = [];
  await step("7. build A queues save_note and tag_note offline, and keeps the Kv and save_note's Idempotency-Key", async () => {
    emitConnectivity(core, false, "none");
    await sleep(50);
    server.on("POST", NOTES, replies.networkError("offline"));
    notes.push(saveNote("s14m", "a", core).catch((e: unknown) => e));
    notes.push(tagNote("s14m", 7, core).catch((e: unknown) => e));
    await waitFor("both notes to wait in the queue", async () => (await storageStatus(core)).pending === 2);
    await waitFor("the queue of two to be persisted", () => Persisted.queueCount(kv.peek(QUEUE_KEY)) === 2);
    const attempt = server.requestsTo("POST", NOTES).find((request) => request.text() === "save:a");
    expect(attempt, "the failed POST of save_note").toBeDefined();
    idempotencyKey = attempt?.header("Idempotency-Key") ?? "";
    expect(idempotencyKey).toMatch(/^[0-9a-f-]{36}$/);
    handedOver = kv.entries();
  });

  // Steps 8 and 9: build B over a Kv holding exactly what build A left.
  const kvB = new MemoryKv(handedOver);
  // The queue is read once build B knows it is offline and where its server is (the order the scenario means by
  // "right after load"; without it a replay could start before `configure_remote`).
  const releaseQueue = kvB.hold("get", QUEUE_KEY);
  const b = await boot({ build: "B", kv: kvB });
  b.server.on("POST", NOTES, replies.networkError("offline"));
  emitConnectivity(b.core, false, "none");
  await configureRemote({ baseUrl: BASE_URL }, b.core);
  releaseQueue();

  const deadLetters = await step("8. build B reads the queue: save_note migrates, tag_note is a dead letter, nothing is replayed", async () => {
    const status = await waitFor("build B to read build A's queue", async () => {
      const s = await storageStatus(b.core);
      return s.queueReadable && s.pending === 1 && s.migrated === 1n && s.deadLettered === 1n ? s : false;
    });
    expect(status.deadLetters).toHaveLength(1);
    const letter = status.deadLetters[0] as string;
    expect(letter.startsWith("tag_note: "), letter).toBe(true);
    expect(letter, "the reason says its input does not migrate").toContain("does not migrate");
    expect(b.server.requests, "no request reached build B's server").toHaveLength(0);
    return status.deadLetters;
  });

  await step("9. online, build B replays save_note once (pinned is None) with build A's key; the dead letter stays", async () => {
    b.server.on("POST", NOTES, replies.text(201, "{}"));
    emitConnectivity(b.core, true, "wifi");
    await waitFor("the migrated save_note to replay", async () => (await storageStatus(b.core)).pending === 0 && b.server.count("POST", NOTES) > 0);
    await sleep(200);
    const posts = b.server.requestsTo("POST", NOTES);
    expect(posts, "POSTs build B replayed").toHaveLength(1);
    expect(posts[0]?.text(), "build B's save_note with pinned == None").toBe("save:a");
    expect(posts[0]?.header("Idempotency-Key"), "the Idempotency-Key of build A's attempt").toBe(idempotencyKey);
    const after = await storageStatus(b.core);
    expect(after.pending).toBe(0);
    expect(after.deadLetters, "the dead letter is still there: never lost, never replayed").toEqual(deadLetters);
    expect(kvB.peek(Persisted.deadLetterKey), `the Kv holds ${Persisted.deadLetterKey}`).toBeDefined();
  });

  handle.close();
  // Build A's notes end when its core closes after the scenario; nothing waits for them.
  void Promise.all(notes);
});
