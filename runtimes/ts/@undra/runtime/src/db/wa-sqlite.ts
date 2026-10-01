import { DbError } from "../adapters/types.js";
import type { DbAdapter } from "./binding.js";

/** Why {@link waSqliteDb} cannot open anything yet. */
export const WA_SQLITE_PENDING =
  "the wa-sqlite adapter is not built yet: its wa-sqlite dependency awaits approval (ADR-048); use nodeSqliteDb on Node, or pass your own DbAdapter";

/** Options of {@link waSqliteDb} (reserved; nothing is read yet). */
export interface WaSqliteDbOptions {
  /** The OPFS directory the databases will live in. Default `undra/db`. */
  readonly directory?: string;
}

/**
 * The browser's Db adapter of ADR-048 §7: wa-sqlite (MIT) in a dedicated worker over OPFS
 * `AccessHandlePoolVFS`, one file per database under `undra/db/<name>`, no WAL (register it with
 * `dbPort(waSqliteDb(), { wal: false })`).
 *
 * **Not built yet.** The wa-sqlite package is a new dependency that awaits the founder's approval,
 * so today every `open` rejects with `DbError.Unavailable` (the text says so) and a core that uses
 * `Db` on the web sees a typed error instead of a missing port. The shape is final: an app can
 * register it now and get the real adapter by upgrading.
 */
export function waSqliteDb(options: WaSqliteDbOptions = {}): DbAdapter {
  void options;
  return {
    open: () => Promise.reject(new DbError.Unavailable(WA_SQLITE_PENDING)),
  };
}
