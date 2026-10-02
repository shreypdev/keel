package dev.undra.runtime.adapters

import dev.undra.runtime.UndraEnum
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraRecord
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException

// The records of the opt-in `Db` port (ADR-048), hand-written like StandardRecords.kt: what generated code for
// `undra-ports` with the `db` feature would contain. Variants are numbered as crates/undra-ports/src/db.rs declares
// them; every error's message is the Rust `#[error]` text.

/**
 * One schema migration, as the port carries it: `DbMigration { version: u32, sql: String }`.
 *
 * @property version the version this migration brings the database to; versions strictly increase from 1.
 * @property sql the SQL, possibly several statements.
 */
public data class DbMigration(val version: UInt, val sql: String) : UndraRecord {
    /** The wire codec of [DbMigration]. */
    public companion object : UndraCodec<DbMigration> {
        override fun encode(w: UndraWriter, v: DbMigration) {
            w.writeU32(v.version)
            w.writeStr(v.sql)
        }

        override fun decode(r: UndraReader): DbMigration = DbMigration(version = r.readU32(), sql = r.readStr())
    }
}

/**
 * What `Db.open` answers: `DbOpened { db: u32, version: u32 }`.
 *
 * @property db the database's id, chosen by the binding; never reused by it.
 * @property version `PRAGMA user_version` after the migrations ran (0 for a database without any).
 */
public data class DbOpened(val db: UInt, val version: UInt) : UndraRecord {
    /** The wire codec of [DbOpened]. */
    public companion object : UndraCodec<DbOpened> {
        override fun encode(w: UndraWriter, v: DbOpened) {
            w.writeU32(v.db)
            w.writeU32(v.version)
        }

        override fun decode(r: UndraReader): DbOpened = DbOpened(db = r.readU32(), version = r.readU32())
    }
}

/** One SQLite value, one of the five storage classes: `DbValue { Null, Integer(i64), Real(f64), Text(String), Blob(Bytes) }`. */
public sealed interface DbValue : UndraEnum {
    /** `NULL`. */
    public data object Null : DbValue

    /** A 64-bit signed integer. */
    public data class Integer(val value: Long) : DbValue

    /** A 64-bit float. */
    public data class Real(val value: Double) : DbValue

    /** UTF-8 text. */
    public data class Text(val value: String) : DbValue

    /** Bytes. */
    public data class Blob(val value: ByteArray) : DbValue {
        override fun equals(other: Any?): Boolean = this === other || (other is Blob && value.contentEquals(other.value))

        override fun hashCode(): Int = value.contentHashCode()

        override fun toString(): String = "Blob(${value.size} bytes)"
    }

    /** The wire codec of [DbValue]. */
    public companion object : UndraCodec<DbValue> {
        override fun encode(w: UndraWriter, v: DbValue) {
            when (v) {
                Null -> w.writeU16(0u)
                is Integer -> {
                    w.writeU16(1u)
                    w.writeI64(v.value)
                }
                is Real -> {
                    w.writeU16(2u)
                    w.writeF64(v.value)
                }
                is Text -> {
                    w.writeU16(3u)
                    w.writeStr(v.value)
                }
                is Blob -> {
                    w.writeU16(4u)
                    w.writeBytes(v.value)
                }
            }
        }

        override fun decode(r: UndraReader): DbValue {
            val at = r.position
            return when (val tag = r.readU16().toInt()) {
                0 -> Null
                1 -> Integer(r.readI64())
                2 -> Real(r.readF64())
                3 -> Text(r.readStr())
                4 -> Blob(r.readBytes())
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "DbValue")
            }
        }
    }
}

/**
 * What `Db.execute` answers: `DbExecuted { changes: u64, last_insert_id: i64 }`.
 *
 * @property changes rows inserted, updated or deleted by the statement (`sqlite3_changes64`).
 * @property lastInsertId the rowid of the last insert on the connection (`sqlite3_last_insert_rowid`).
 */
public data class DbExecuted(val changes: ULong, val lastInsertId: Long) : UndraRecord {
    /** The wire codec of [DbExecuted]. */
    public companion object : UndraCodec<DbExecuted> {
        override fun encode(w: UndraWriter, v: DbExecuted) {
            w.writeU64(v.changes)
            w.writeI64(v.lastInsertId)
        }

        override fun decode(r: UndraReader): DbExecuted = DbExecuted(changes = r.readU64(), lastInsertId = r.readI64())
    }
}

private val stringList: UndraCodec<List<String>> = Codecs.vec(Codecs.string)
private val rowList: UndraCodec<List<List<DbValue>>> = Codecs.vec(Codecs.vec(DbValue))

/**
 * What `Db.query` answers: `DbRows { columns: Vec<String>, rows: Vec<Vec<DbValue>> }`.
 *
 * @property columns the result's column names, in order.
 * @property rows the rows, each with one value per column.
 */
public data class DbRows(val columns: List<String>, val rows: List<List<DbValue>>) : UndraRecord {
    /** The wire codec of [DbRows]. */
    public companion object : UndraCodec<DbRows> {
        override fun encode(w: UndraWriter, v: DbRows) {
            stringList.encode(w, v.columns)
            rowList.encode(w, v.rows)
        }

        override fun decode(r: UndraReader): DbRows = DbRows(columns = stringList.decode(r), rows = rowList.decode(r))
    }
}

/** Which constraint a statement broke: `DbConstraint { Unique, NotNull, ForeignKey, Check, Other }`. */
public enum class DbConstraint(public val index: UShort) : UndraEnum {
    /** `UNIQUE` or `PRIMARY KEY`. */
    UNIQUE(0u),

    /** `NOT NULL`. */
    NOT_NULL(1u),

    /** `FOREIGN KEY` (enforced: every adapter opens with `foreign_keys = ON`). */
    FOREIGN_KEY(2u),

    /** `CHECK`. */
    CHECK(3u),

    /** Any other constraint (a trigger's `RAISE`, ...). */
    OTHER(4u),
    ;

    /** The wire codec of [DbConstraint] (a `u16` variant index). */
    public companion object : UndraCodec<DbConstraint> {
        override fun encode(w: UndraWriter, v: DbConstraint): Unit = w.writeU16(v.index)

        override fun decode(r: UndraReader): DbConstraint {
            val at = r.position
            val tag = r.readU16()
            return entries.firstOrNull { it.index == tag } ?: throw WireException.InvalidTag(tag.toUInt(), at, "DbConstraint")
        }
    }
}

/**
 * Why a database operation failed (ADR-048):
 * `DbError { Busy, Constraint { kind, message }, Corrupt(String), Full, Unavailable(String), Sql { message }, Migration { version, message } }`.
 * The variant follows SQLite's result code, never the message text (Android, which reports no code, is the documented
 * exception: its `(code NNNN ...)` suffix is parsed).
 */
public sealed class DbError(message: String) : UndraException(message) {
    /** The database stayed locked past the busy timeout (5 s): another connection, or a statement made outside a running transaction on the same database. */
    public data object Busy : DbError("the database is busy")

    /**
     * The statement broke a constraint.
     *
     * @property kind which kind of constraint.
     * @property reason SQLite's message, for example `UNIQUE constraint failed: todos.id` (the Rust field `message`).
     */
    public data class Constraint(val kind: DbConstraint, val reason: String) : DbError("constraint failed: $reason")

    /** The file is not a database or is damaged; [reason] is SQLite's message. */
    public data class Corrupt(val reason: String) : DbError("the database is corrupt: $reason")

    /** The disk or the storage quota is full. */
    public data object Full : DbError("the database is full")

    /** No adapter, a database or transaction that is closed or unknown, an invalid name, a file that cannot be opened. */
    public data class Unavailable(val reason: String) : DbError("the database is unavailable: $reason")

    /**
     * Anything else SQLite refused: a syntax error, a missing table, more than one statement where one is expected.
     *
     * @property reason SQLite's message (or the adapter's); the Rust field `message`.
     */
    public data class Sql(val reason: String) : DbError("SQL error: $reason")

    /**
     * A migration failed (everything it and the migrations before it in the same open did is rolled back), or the
     * database is at a version newer than the newest migration.
     *
     * @property version the version that failed, or the database's version when it is too new.
     * @property reason what went wrong (the Rust field `message`).
     */
    public data class Migration(val version: UInt, val reason: String) : DbError("migration $version failed: $reason")

    /** The wire codec of [DbError]. */
    public companion object : UndraCodec<DbError> {
        override fun encode(w: UndraWriter, v: DbError) {
            when (v) {
                Busy -> w.writeU16(0u)
                is Constraint -> {
                    w.writeU16(1u)
                    DbConstraint.encode(w, v.kind)
                    w.writeStr(v.reason)
                }
                is Corrupt -> {
                    w.writeU16(2u)
                    w.writeStr(v.reason)
                }
                Full -> w.writeU16(3u)
                is Unavailable -> {
                    w.writeU16(4u)
                    w.writeStr(v.reason)
                }
                is Sql -> {
                    w.writeU16(5u)
                    w.writeStr(v.reason)
                }
                is Migration -> {
                    w.writeU16(6u)
                    w.writeU32(v.version)
                    w.writeStr(v.reason)
                }
            }
        }

        override fun decode(r: UndraReader): DbError {
            val at = r.position
            return when (val tag = r.readU16().toInt()) {
                0 -> Busy
                1 -> Constraint(kind = DbConstraint.decode(r), reason = r.readStr())
                2 -> Corrupt(r.readStr())
                3 -> Full
                4 -> Unavailable(r.readStr())
                5 -> Sql(r.readStr())
                6 -> Migration(version = r.readU32(), reason = r.readStr())
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "DbError")
            }
        }
    }
}
