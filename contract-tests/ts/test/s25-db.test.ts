import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, expect, test } from "vitest";
import { DbError, type PortImpl, type UndraCore } from "@undra/runtime";
import { type DbWorkerLike, type DbWorkerScope, OptInPortIds, dbPort, nodeSqliteDb, waSqliteDb } from "@undra/runtime/db";
import { startDbWorker } from "@undra/runtime/db-worker";
import { Notes, dbCells, dbMigrate, dbRun } from "@playground/core";
import { boot, bootWorker } from "../src/harness.js";
import { step, waitFor } from "../src/wait.js";

// S25 Db (ADR-048): the opt-in Db port through the platform's real SQLite adapter (`nodeSqliteDb`
// over `node:sqlite`), rooted in a fresh temporary directory: migrations at open, typed cells, typed
// constraint and SQL errors, a transaction that rolls back, a failed migration that leaves nothing,
// persistence across a reopen, an invalid name.
//
// The same steps also run once on the browser's adapter, `waSqliteDb` (wa-sqlite behind its worker
// protocol), with the worker served in this process over a `MessageChannel` and wa-sqlite's in-memory
// VFS (OPFS exists only in a browser's dedicated workers). That run is not named after the scenario:
// `node:sqlite` is the TypeScript column's adapter for S25.

let directory: string;

beforeAll(() => {
  directory = mkdtempSync(join(tmpdir(), "undra-s25-"));
});

afterAll(() => {
  rmSync(directory, { recursive: true, force: true });
});

const ports = (): Record<number, PortImpl> => ({ [OptInPortIds.Db.portId]: dbPort(nodeSqliteDb({ directory })) });

async function failure(run: () => Promise<unknown>): Promise<unknown> {
  try {
    await run();
  } catch (error) {
    return error;
  }
  throw new Error("expected the call to fail, but it succeeded");
}

/** The eight steps of S25 on `core`, whose Db port is the adapter under test. */
async function s25(core: UndraCore): Promise<void> {
  const notes = await Notes.create(core);

  await step("1. open runs the two migrations; the list is empty", async () => {
    expect(await notes.open("contract-s25")).toBe(2);
    expect(notes.notes.peek()).toEqual([]);
  });

  await step("2. add twice (ids 1 and 2, in the mirror); toggle 1; count is 2", async () => {
    expect((await notes.add("milk")).id).toBe(1n);
    expect((await notes.add("eggs")).id).toBe(2n);
    await waitFor("both notes in the mirror", () => notes.notes.peek().length === 2);
    await notes.toggle(1n);
    await waitFor("note 1 done in the mirror", () => notes.notes.peek()[0]?.done === true);
    expect(notes.notes.peek()).toEqual([
      { id: 1n, title: "milk", done: true },
      { id: 2n, title: "eggs", done: false },
    ]);
    expect(await notes.count()).toBe(2);
  });

  await step("3. every storage class there and back, the integer beyond JavaScript's safe range as a bigint", async () => {
    expect(await dbCells(-9007199254740993n, 1.5, "é😀", Uint8Array.of(0, 255, 7), null, core)).toEqual({
      int: -9007199254740993n,
      real: 1.5,
      text: "é😀",
      blob: Uint8Array.of(0, 255, 7),
      none: null,
      types: ["integer", "real", "text", "blob", "null"],
    });
  });

  await step("4. constraints, typed: Unique; NotNull rolls the batch back; a good batch adds two", async () => {
    const unique = await failure(() => notes.addWithId(1n, "dup"));
    expect(unique).toBeInstanceOf(DbError.Constraint);
    expect((unique as DbError.Constraint).kind_).toBe("unique");
    const notNull = await failure(() => notes.addAll(["a", null]));
    expect(notNull).toBeInstanceOf(DbError.Constraint);
    expect((notNull as DbError.Constraint).kind_).toBe("notNull");
    expect(await notes.count(), "the transaction rolled back").toBe(2);
    expect(notes.notes.peek()).toHaveLength(2);
    expect(await notes.addAll(["a", "b"])).toBe(2);
    expect(await notes.count()).toBe(4);
  });

  await step("5. SQL errors, typed", async () => {
    expect(await failure(() => dbRun(":memory:", "INSERT INTO missing VALUES (1)", core))).toBeInstanceOf(DbError.Sql);
    expect(await failure(() => dbRun(":memory:", "SELEC 1", core))).toBeInstanceOf(DbError.Sql);
  });

  await step("6. migrations run in one transaction: a broken second one leaves nothing behind", async () => {
    const broken = await failure(() => dbMigrate("contract-s25-m", true, core));
    expect(broken).toBeInstanceOf(DbError.Migration);
    expect((broken as DbError.Migration).version).toBe(2);
    expect(await dbMigrate("contract-s25-m", false, core), "migration 1 did not survive the failed open").toBe(2);
  });

  await step("7. persistence: a new store reopens the database with its four notes, note 1 done", async () => {
    await notes.closeDatabase();
    const again = await Notes.create(core);
    expect(await again.open("contract-s25")).toBe(2);
    await waitFor("the four notes in the new store's mirror", () => again.notes.peek().length === 4);
    expect(again.notes.peek().map((n) => [n.id, n.done])).toEqual([
      [1n, true],
      [2n, false],
      [3n, false],
      [4n, false],
    ]);
  });

  await step("8. an invalid name is Unavailable", async () => {
    const third = await Notes.create(core);
    expect(await failure(() => third.open("../escape"))).toBeInstanceOf(DbError.Unavailable);
  });
}

test("S25 Db", async () => {
  const { core } = await boot({ ports: ports() });
  await s25(core);
});

test("wa-sqlite (the browser's adapter, in-memory VFS, through its worker protocol): the S25 steps", async () => {
  const { port1, port2 } = new MessageChannel();
  port1.start();
  port2.start();
  const stop = startDbWorker(port1 as unknown as DbWorkerScope, { storage: "memory" });
  try {
    const { core } = await boot({ ports: { [OptInPortIds.Db.portId]: dbPort(waSqliteDb({ worker: port2 as unknown as DbWorkerLike }), { wal: false }) } });
    await s25(core);
  } finally {
    stop();
    port1.close();
    port2.close();
  }
});

test("wasm-worker mode: the Db port is served on the main thread (ADR-049 §2)", async () => {
  const { core } = await bootWorker({ ports: ports() });
  const cells = await dbCells(2n ** 63n - 1n, -0.25, "worker", new Uint8Array(0), "x", core);
  expect(cells).toEqual({ int: 2n ** 63n - 1n, real: -0.25, text: "worker", blob: new Uint8Array(0), none: "x", types: ["integer", "real", "text", "blob", "text"] });
});
