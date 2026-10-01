import { expect, test } from "vitest";
import { HttpError, UndraReader, codecs, emitConnectivity } from "@undra/runtime";
import { type RemoteTodo, RemoteError, RemoteTodosQueryHandle, configureRemote, createRemoteTodo, setRemoteDone } from "@playground/core";
import { BASE_URL, boot } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S14 offline queue replay: while the device is offline an idempotent mutation keeps waiting
// (queued, persisted, its placeholder on show) and is replayed, with the same Idempotency-Key,
// when the network returns; a mutation that is not idempotent fails at once.

const LIST = "s14";
const URL = `${BASE_URL}/lists/${LIST}/todos`;
const PATCH_URL = `${URL}/1`;
const QUEUE_KEY = "undra.query.queue";
const created: RemoteTodo = { id: 9, title: "Offline item", done: false };
const isPlaceholder = (todo: RemoteTodo): boolean => todo.id >= 0x8000_0000;

/** What the persisted queue holds: `{ schema_hash u64, count u32, count x { mutation_id u32, params bytes, idempotency_key Uuid } }`. */
function decodeQueue(bytes: Uint8Array): { readonly keys: string[] } {
  const r = new UndraReader(bytes);
  r.readU64();
  const count = r.readU32();
  const keys: string[] = [];
  for (let i = 0; i < count; i++) {
    r.readU32();
    codecs.bytes.decode(r);
    keys.push(r.readUuid());
  }
  r.finish();
  return { keys };
}

test("S14 offline queue replay", async () => {
  const { core, server, kv } = await boot();
  await configureRemote({ baseUrl: BASE_URL }, core);
  server.on("GET", URL, replies.json(200, []));
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

  const queuedKeys = await step("persisted: the queue was written to Kv while offline", () => {
    const writes = kv.writesTo(QUEUE_KEY);
    expect(writes.length, "the queue was written").toBeGreaterThanOrEqual(1);
    const last = writes[writes.length - 1];
    expect(last?.op).toBe("set");
    const { keys } = decodeQueue(last?.value as Uint8Array);
    expect(keys, "one queued mutation").toHaveLength(1);
    return keys;
  });

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
    expect(keys[0], "and it is the key the queue persisted").toBe(queuedKeys[0]);
    await waitFor("the list to hold the server's item", () => {
      const data = handle.data.peek();
      return data?.length === 1 && data[0]?.id === 9;
    });
    expect(handle.data.peek()).toEqual([created]);
  });

  await step("6. the queue was emptied after the replay", async () => {
    await waitFor("the queue to be emptied", () => {
      const last = kv.writesTo(QUEUE_KEY).at(-1);
      return last !== undefined && (last.op === "delete" || decodeQueue(last.value as Uint8Array).keys.length === 0);
    });
    expect(kv.peek(QUEUE_KEY) === undefined || decodeQueue(kv.peek(QUEUE_KEY) as Uint8Array).keys.length === 0).toBe(true);
  });

  handle.close();
});
