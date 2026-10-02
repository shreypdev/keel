import { type RecoveryOptions, Signal, type UndraCoreRestarted } from "@undra/runtime";

/**
 * Crash recovery of the web core (ADR-049): the playground turns it on, so a panic in the wasm core (which traps it)
 * restarts the core from its last snapshot instead of leaving the page dead. A snapshot at most once a second while
 * stores change; at most three restarts a minute, after which the core stays down and the page says so.
 */
export const RECOVERY: RecoveryOptions = { snapshotEveryMs: 1000, maxRestarts: 3, perMs: 60_000 };

/** One restart, as the debug panel shows it. */
export interface RestartEntry {
  /** When the core came back (`Date.now`). */
  readonly at: number;
  /** The panic that trapped the core. */
  readonly message: string;
  /** How old the snapshot the stores came back from was, in ms; `null` when there was none. */
  readonly restoredFromAgeMs: number | null;
  /** Calls and streams in flight that failed with "restarted". */
  readonly rejectedCalls: number;
  /** Objects that went stale (not stores, not query handles). */
  readonly staleObjects: number;
}

/** How many restarts the panel keeps. */
const KEPT = 10;

/** The restarts of this page's core, newest first, as a signal the debug panel renders. */
export class RestartLog {
  /** The restarts, newest first (at most ten). */
  readonly entries = new Signal<readonly RestartEntry[]>([]);

  /** Records a restart; `LoadOptions.onCoreRestarted` calls it. */
  record(event: UndraCoreRestarted, now: number = Date.now()): void {
    const entry: RestartEntry = {
      at: now,
      message: event.report.message,
      restoredFromAgeMs: event.restoredFromAgeMs,
      rejectedCalls: event.rejectedCalls,
      staleObjects: event.staleObjects,
    };
    this.entries._set([entry, ...this.entries.peek()].slice(0, KEPT));
  }
}

const plural = (count: number, one: string, many: string): string => `${count} ${count === 1 ? one : many}`;

/** One line about a restart: what came back and what was lost. */
export function describeRestart(entry: RestartEntry): string {
  const restored =
    entry.restoredFromAgeMs === null ? "no snapshot to restore: the stores started empty" : `stores restored from a snapshot ${entry.restoredFromAgeMs} ms old`;
  return `${restored}; ${plural(entry.rejectedCalls, "call", "calls")} in flight failed; ${plural(entry.staleObjects, "object", "objects")} went stale`;
}
