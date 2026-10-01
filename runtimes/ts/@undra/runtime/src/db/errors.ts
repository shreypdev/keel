import { type DbConstraint, DbError } from "../adapters/types.js";

/*
 * SQLite's result codes as `DbError`s (ADR-048 §2), the one mapping every TypeScript Db adapter
 * uses (`nodeSqliteDb`, the wa-sqlite engine of `waSqliteDb`).
 */

/** The constraint kind of an extended `SQLITE_CONSTRAINT_*` code. */
function constraintKind(extended: number): DbConstraint {
  switch (extended) {
    case 2067: // SQLITE_CONSTRAINT_UNIQUE
    case 1555: // SQLITE_CONSTRAINT_PRIMARYKEY
      return "unique";
    case 1299: // SQLITE_CONSTRAINT_NOTNULL
      return "notNull";
    case 787: // SQLITE_CONSTRAINT_FOREIGNKEY
      return "foreignKey";
    case 275: // SQLITE_CONSTRAINT_CHECK
      return "check";
    default:
      return "other";
  }
}

/**
 * The {@link DbError} of an SQLite failure, chosen by its (extended) result code, never its text
 * (ADR-048 §2): BUSY/LOCKED `Busy`, CONSTRAINT by extended code, CORRUPT/NOTADB `Corrupt`, FULL
 * `Full`, CANTOPEN/PERM/READONLY/IOERR `Unavailable`, the rest `Sql`.
 */
export function sqliteError(code: number, message: string): DbError {
  switch (code & 0xff) {
    case 5: // SQLITE_BUSY
    case 6: // SQLITE_LOCKED
      return new DbError.Busy();
    case 19: // SQLITE_CONSTRAINT
      return new DbError.Constraint(constraintKind(code), message);
    case 11: // SQLITE_CORRUPT
    case 26: // SQLITE_NOTADB
      return new DbError.Corrupt(message);
    case 13: // SQLITE_FULL
      return new DbError.Full();
    case 14: // SQLITE_CANTOPEN
    case 3: // SQLITE_PERM
    case 8: // SQLITE_READONLY
    case 10: // SQLITE_IOERR
      return new DbError.Unavailable(message);
    default:
      return new DbError.Sql(message);
  }
}
