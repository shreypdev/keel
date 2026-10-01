// The records of the opt-in `Db` port (ADR-048), with their hand-written wire codecs:
// `DbMigration`, `DbOpened`, `DbValue`, `DbExecuted`, `DbRows`, `DbConstraint` and `DbError`.
//
// Public API, like the records of StandardRecords.swift: generated bindings of a core built with
// the `db` feature refer to them (`UndraCallError.mapped(error, domain: DbError.self)`) and
// declare none of them, and an app that implements `DbAdapter` itself builds and reads them. The
// shapes, names and messages are what `undra-bindgen` generates for `undra-ports` (the errors'
// descriptions are the Rust `#[error]` texts).
//
// This file needs no Foundation: `DbError`'s `LocalizedError` conformance is in
// Wire/Foundation+Undra.swift.

/// One schema migration, as the port carries it.
///
/// `DbMigration { version: u32, sql: String }`.
public struct DbMigration: UndraRecord, Sendable, Hashable, Codable {
    /// The version this migration brings the database to; versions strictly increase from 1.
    public var version: UInt32
    /// The SQL, possibly several statements.
    public var sql: String

    /// Creates a migration to `version`.
    public init(version: UInt32, sql: String) {
        self.version = version
        self.sql = sql
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> DbMigration {
        let version = try r.readU32()
        let sql = try r.readString()
        return DbMigration(version: version, sql: sql)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU32(version)
        w.writeString(sql)
    }
}

/// What `Db.open` answers: the database's id and its version after migrating.
///
/// `DbOpened { db: u32, version: u32 }`.
public struct DbOpened: UndraRecord, Sendable, Hashable, Codable {
    /// The database's id, chosen by the binding; never reused by it.
    public var db: UInt32
    /// `PRAGMA user_version` after the migrations ran (0 for a database without any).
    public var version: UInt32

    /// Creates the answer of an open.
    public init(db: UInt32, version: UInt32) {
        self.db = db
        self.version = version
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> DbOpened {
        let db = try r.readU32()
        let version = try r.readU32()
        return DbOpened(db: db, version: version)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU32(db)
        w.writeU32(version)
    }
}

/// One SQLite value: the five storage classes.
///
/// `DbValue { Null = 0, Integer(i64) = 1, Real(f64) = 2, Text(String) = 3, Blob(Bytes) = 4 }`.
/// `Codable` (synthesized, `{"integer":{"_0":7}}`) because generated records that hold a value
/// derive `Codable`.
public enum DbValue: UndraEnum, Sendable, Hashable, Codable {
    /// `NULL`.
    case null
    /// A 64-bit signed integer.
    case integer(Int64)
    /// A 64-bit float. SQLite stores a NaN as `NULL`.
    case real(Double)
    /// UTF-8 text.
    case text(String)
    /// Bytes.
    case blob([UInt8])

    /// SQLite's name of the storage class, as `typeof()` reports it: `"null"`, `"integer"`,
    /// `"real"`, `"text"` or `"blob"`.
    public var storageClass: String {
        switch self {
        case .null:
            return "null"
        case .integer:
            return "integer"
        case .real:
            return "real"
        case .text:
            return "text"
        case .blob:
            return "blob"
        }
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> DbValue {
        let at = r.position
        let tag = try r.readU16()
        switch tag {
        case 0:
            return .null
        case 1:
            return .integer(try r.readI64())
        case 2:
            return .real(try r.readF64())
        case 3:
            return .text(try r.readString())
        case 4:
            return .blob(try r.readBytes())
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "DbValue")
        }
    }

    public func undraEncode(_ w: inout UndraWriter) {
        switch self {
        case .null:
            w.writeU16(0)
        case .integer(let value):
            w.writeU16(1)
            w.writeI64(value)
        case .real(let value):
            w.writeU16(2)
            w.writeF64(value)
        case .text(let value):
            w.writeU16(3)
            w.writeString(value)
        case .blob(let value):
            w.writeU16(4)
            w.writeBytes(value)
        }
    }
}

/// What `Db.execute` answers.
///
/// `DbExecuted { changes: u64, last_insert_id: i64 }`.
public struct DbExecuted: UndraRecord, Sendable, Hashable, Codable {
    /// Rows inserted, updated or deleted by the statement (`sqlite3_changes64`).
    public var changes: UInt64
    /// The rowid of the last insert on the connection (`sqlite3_last_insert_rowid`).
    public var lastInsertId: Int64

    /// Creates the answer of an execute.
    public init(changes: UInt64, lastInsertId: Int64) {
        self.changes = changes
        self.lastInsertId = lastInsertId
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> DbExecuted {
        let changes = try r.readU64()
        let lastInsertId = try r.readI64()
        return DbExecuted(changes: changes, lastInsertId: lastInsertId)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU64(changes)
        w.writeI64(lastInsertId)
    }
}

/// What `Db.query` answers: the column names and the rows, each a cell per column.
///
/// `DbRows { columns: Vec<String>, rows: Vec<Vec<DbValue>> }`.
public struct DbRows: UndraRecord, Sendable, Hashable, Codable {
    /// The result's column names, in order.
    public var columns: [String]
    /// The rows, each with one value per column.
    public var rows: [[DbValue]]

    /// Creates a result; without arguments it has no columns and no rows.
    public init(columns: [String] = [], rows: [[DbValue]] = []) {
        self.columns = columns
        self.rows = rows
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> DbRows {
        let columns = try [String].undraDecode(&r)
        let rows = try [[DbValue]].undraDecode(&r)
        return DbRows(columns: columns, rows: rows)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        columns.undraEncode(&w)
        rows.undraEncode(&w)
    }
}

/// Which constraint a statement broke.
///
/// `DbConstraint { Unique = 0, NotNull = 1, ForeignKey = 2, Check = 3, Other = 4 }`.
public enum DbConstraint: UInt16, UndraEnum, CaseIterable, Sendable, Codable {
    /// `UNIQUE` or `PRIMARY KEY`.
    case unique = 0
    /// `NOT NULL`.
    case notNull = 1
    /// `FOREIGN KEY` (enforced: every database is opened with `foreign_keys = ON`).
    case foreignKey = 2
    /// `CHECK`.
    case check = 3
    /// Any other constraint (a trigger's `RAISE`, ...).
    case other = 4

    public static func undraDecode(_ r: inout UndraReader) throws -> DbConstraint {
        let at = r.position
        let tag = try r.readU16()
        guard let value = DbConstraint(rawValue: tag) else {
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "DbConstraint")
        }
        return value
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU16(rawValue)
    }
}

/// Why a database operation failed (ADR-048). The case follows SQLite's result code, never the
/// message text.
///
/// `DbError { Busy = 0, Constraint { kind, message } = 1, Corrupt(String) = 2, Full = 3,
/// Unavailable(String) = 4, Sql { message } = 5, Migration { version, message } = 6 }`.
public enum DbError: UndraError, Error, Sendable, Hashable {
    /// The database stayed locked past the busy timeout (5 s): another connection, or a
    /// statement made outside a running transaction on the same database.
    case busy
    /// The statement broke a constraint; `message` is SQLite's (it names the columns).
    case constraint(kind: DbConstraint, message: String)
    /// The file is not a database or is damaged.
    case corrupt(String)
    /// The disk or the storage quota is full.
    case full
    /// No adapter, a database or transaction that is closed or unknown, an invalid name, a file
    /// that cannot be opened.
    case unavailable(String)
    /// Anything else SQLite refused: a syntax error, a missing table, a wrong cell type, more
    /// than one statement where one is expected.
    case sql(message: String)
    /// A migration failed (everything it and the migrations before it in the same open did is
    /// rolled back), or the database is at a version newer than the newest migration.
    case migration(version: UInt32, message: String)

    public static func undraDecode(_ r: inout UndraReader) throws -> DbError {
        let at = r.position
        let tag = try r.readU16()
        switch tag {
        case 0:
            return .busy
        case 1:
            let kind = try DbConstraint.undraDecode(&r)
            let message = try r.readString()
            return .constraint(kind: kind, message: message)
        case 2:
            return .corrupt(try r.readString())
        case 3:
            return .full
        case 4:
            return .unavailable(try r.readString())
        case 5:
            return .sql(message: try r.readString())
        case 6:
            let version = try r.readU32()
            let message = try r.readString()
            return .migration(version: version, message: message)
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "DbError")
        }
    }

    public func undraEncode(_ w: inout UndraWriter) {
        switch self {
        case .busy:
            w.writeU16(0)
        case .constraint(let kind, let message):
            w.writeU16(1)
            kind.undraEncode(&w)
            w.writeString(message)
        case .corrupt(let message):
            w.writeU16(2)
            w.writeString(message)
        case .full:
            w.writeU16(3)
        case .unavailable(let message):
            w.writeU16(4)
            w.writeString(message)
        case .sql(let message):
            w.writeU16(5)
            w.writeString(message)
        case .migration(let version, let message):
            w.writeU16(6)
            w.writeU32(version)
            w.writeString(message)
        }
    }
}

extension DbError: CustomStringConvertible {
    /// The message of the Rust `#[error]` attribute.
    public var description: String {
        switch self {
        case .busy:
            return "the database is busy"
        case .constraint(_, let message):
            return "constraint failed: \(message)"
        case .corrupt(let message):
            return "the database is corrupt: \(message)"
        case .full:
            return "the database is full"
        case .unavailable(let message):
            return "the database is unavailable: \(message)"
        case .sql(let message):
            return "SQL error: \(message)"
        case .migration(let version, let message):
            return "migration \(version) failed: \(message)"
        }
    }
}
