import { UndraCore, WasmMainTransport } from "@undra/runtime";
import { Counter, Todos, UndraIds, explode } from "@playground/core";
import { memoryKv } from "../memory-kv";
import { type OpResult, opResult } from "./stats";

/*
 * The web recovery benchmark (ADR-049, implementation brief item 9), in this page's own thread (`wasm-main`):
 *
 * * `ts/snapshot_take_100kb`: what the snapshot keeper pays to keep a snapshot of about 100 KB of store state for a
 *   restart: `undra_snapshot`, the copy out of wasm memory into an `ArrayBuffer`, the size check. Budget 2 ms on
 *   desktop Chromium.
 * * `ts/recovery_restart_100kb`: a trap until `onCoreRestarted`: the panic report, failing what was in flight, a new
 *   instance of the same compiled module (`_initialize`, `undra_init`), the restore of the 100 KB snapshot, and the
 *   mirror observing every store again (52 signals in 17 stores). Budget 50 ms.
 *
 * The state is 1,000 to-dos of about 100 bytes each in one `Todos` store (its 4 signals) and 16 `Counter` stores (3
 * signals each). Each restart sample traps the core on purpose (`explode`), with a fresh 100 KB snapshot kept just
 * before; the restart budget is lifted so that every sample restarts.
 */

/** The to-dos that make the 100 KB of state. */
export const TODOS = 1_000;
/** The counter stores besides the to-do list: 4 + 16 x 3 = 52 signals observed again by a restart. */
export const COUNTERS = 16;
/** The title of to-do `i`: 79 characters, so that a to-do is about 100 bytes on the wire (16-byte id, 4 + 79, 1). */
export const titleOf = (i: number): string => `to-do ${String(i).padStart(4, "0")} `.padEnd(79, "abcdefghij");

/** How much of a run each part gets. */
export interface RecoveryBenchConfig {
  /** Timed batches of `snapshot_take`. */
  readonly snapshotBatches: number;
  /** Snapshots per batch. */
  readonly snapshotBatchSize: number;
  /** Batches thrown away first. */
  readonly warmupBatches: number;
  /** Timed restarts (one trap each). */
  readonly restarts: number;
  /** Restarts thrown away first. */
  readonly warmupRestarts: number;
}

/** The default run. */
export const RECOVERY_FULL: RecoveryBenchConfig = { snapshotBatches: 50, snapshotBatchSize: 10, warmupBatches: 5, restarts: 30, warmupRestarts: 3 };
/** A run that checks the plumbing. */
export const RECOVERY_QUICK: RecoveryBenchConfig = { snapshotBatches: 3, snapshotBatchSize: 2, warmupBatches: 1, restarts: 3, warmupRestarts: 1 };

/** What `measureRecovery` returns. */
export interface RecoveryBenchResult {
  readonly schema: "undra-recovery-bench/1";
  readonly runtime: "wasm-main";
  /** The size of the snapshot each row works with. */
  readonly snapshot_bytes: number;
  /** Stores and signals a restart observes again. */
  readonly stores: number;
  readonly signals: number;
  readonly config: RecoveryBenchConfig;
  readonly ops: readonly OpResult[];
}

function fail(message: string): never {
  throw new Error(`recovery bench check failed: ${message}`);
}

/** Measures the two rows on a fresh core of `module`. */
export async function measureRecovery(module: WebAssembly.Module, config: RecoveryBenchConfig, now: () => number): Promise<RecoveryBenchResult> {
  const silent = { log() {} };
  const waiting: Array<() => void> = [];
  const transport = new WasmMainTransport({
    wasm: module,
    expectedSchemaHash: UndraIds.schemaHash,
    // The bench keeps its snapshots itself (`keepSnapshotNow`), so the schedule never fires during a measurement.
    recovery: { snapshotEveryMs: 3_600_000, maxSnapshotBytes: 16 * 1024 * 1024 },
  });
  const core = await UndraCore.attach(transport, {
    expectedSchemaHash: UndraIds.schemaHash,
    shared: false,
    adapters: { kv: memoryKv(), log: silent, http: null, connectivity: null, lifecycle: null },
    recovery: { snapshotEveryMs: 3_600_000, maxRestarts: Number.MAX_SAFE_INTEGER, perMs: 1 },
    onCoreRestarted: () => waiting.shift()?.(),
  });
  try {
    const todos = await Todos.create(core);
    for (let i = 0; i < TODOS; i++) await todos.add(titleOf(i));
    const counters = await Promise.all(Array.from({ length: COUNTERS }, () => Counter.create(core)));
    for (const [i, counter] of counters.entries()) for (let k = 0; k <= i; k++) await counter.increment();
    const bytes = transport.keepSnapshotNow() ?? fail("no snapshot was kept");
    if (bytes < 95 * 1024 || bytes > 120 * 1024) fail(`the snapshot is ${bytes} bytes, not about 100 KB`);

    // ---- snapshot_take_100kb ------------------------------------------------------------------------
    const takes: number[] = [];
    for (let b = 0; b < config.warmupBatches + config.snapshotBatches; b++) {
      const t0 = now();
      for (let i = 0; i < config.snapshotBatchSize; i++) transport.keepSnapshotNow();
      const t1 = now();
      if (b >= config.warmupBatches) takes.push(((t1 - t0) * 1e6) / config.snapshotBatchSize);
    }
    if (transport.keptSnapshot?.data.byteLength !== bytes) fail("the kept snapshot changed size");

    // ---- recovery_restart_100kb ---------------------------------------------------------------------
    const restarts: number[] = [];
    for (let r = 0; r < config.warmupRestarts + config.restarts; r++) {
      transport.keepSnapshotNow();
      const restarted = new Promise<void>((resolve) => waiting.push(resolve));
      const t0 = now();
      await explode("recovery bench", core).then(
        () => fail("explode returned"),
        () => undefined,
      );
      await restarted;
      const t1 = now();
      if (r >= config.warmupRestarts) restarts.push((t1 - t0) * 1e6);
      // Every store came back with its values, through the mirror.
      if (todos.todos.peek().length !== TODOS) fail(`the to-do list holds ${todos.todos.peek().length} items after a restart`);
      if (counters.at(-1)?.count.peek() !== COUNTERS) fail("the last counter lost its value in a restart");
      if (core.closed) fail("the core closed");
    }

    return {
      schema: "undra-recovery-bench/1",
      runtime: "wasm-main",
      snapshot_bytes: bytes,
      stores: 1 + COUNTERS,
      signals: 4 + 3 * COUNTERS,
      config,
      ops: [
        opResult(
          "ts/snapshot_take_100kb",
          "batched",
          config.snapshotBatchSize,
          takes,
          `\`undra_snapshot\` of ${bytes} bytes of store state, copied out of wasm memory and kept for a restart (the recovery keeper's work), on the page's thread`,
        ),
        opResult(
          "ts/recovery_restart_100kb",
          "each",
          1,
          restarts,
          `a trap (\`explode\`) until \`onCoreRestarted\`: the same compiled module instantiated again, \`undra_init\`, the ${bytes}-byte snapshot restored, ${1 + COUNTERS} stores (${4 + 3 * COUNTERS} signals) observed again into the mirror`,
        ),
      ],
    };
  } finally {
    core.close();
  }
}
