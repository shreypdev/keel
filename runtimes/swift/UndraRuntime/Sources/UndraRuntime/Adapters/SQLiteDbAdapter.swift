// Db (ADR-048) on the system's SQLite: the SQLite3 C API from Swift, one serial DispatchQueue per
// database connection.

import Foundation
import SQLite3

/// `Db` on the SQLite library every Apple platform ships (`import SQLite3`).
///
/// Each database is a file `<directory>/<name>.sqlite`, the directory being
/// `Application Support/<bundle id>/Undra/<namespace>/db` (next to the `Kv` adapter's directories,
/// the namespace being that of the core the adapter is registered with: ADR-044 amendment A, so two
/// cores of one app never share a database file) unless one is given (the directories are
/// created); `":memory:"` is a private in-memory database. Every connection runs on a serial
/// `DispatchQueue` of its own, so the core's thread never waits for the disk.
///
/// `execute` and `query` prepare exactly one statement (trailing SQL other than whitespace and
/// comments is ``DbError/sql(message:)``), check the parameter count, and bind the values
/// positionally; `query` reads every cell by its storage class. Failures are typed by SQLite's
/// extended result code, never by its message: `BUSY`/`LOCKED` → `busy`, `CONSTRAINT_*` →
/// `constraint` of that kind, `CORRUPT`/`NOTADB` → `corrupt`, `FULL` → `full`,
/// `CANTOPEN`/`PERM`/`READONLY`/`IOERR` → `unavailable`, anything else → `sql`.
///
/// It is also the `Db` adapter of ``Adapters/platformDefault`` (served by the binding of
/// ``DbPortAdapter`` with the 5-second busy timeout).
public final class SQLiteDbAdapter: NamespaceScopedDb, UndraAdapter, @unchecked Sendable {
    /// The directory the database files are in; `nil` for the default adapter, whose directory is
    /// that of the namespace of each core it serves (``defaultDirectory(namespace:)``).
    public let directory: URL?
    private let bindings = BindingSet<DbBinding>()

    /// Keeps the databases in `Application Support/<bundle id>/Undra/<namespace>/db`, the namespace
    /// being that of the core the adapter is registered with.
    public convenience init() {
        self.init(optionalDirectory: nil)
    }

    /// Keeps the databases in `directory` (created when the first database is opened), for every
    /// core it serves.
    public convenience init(directory: URL) {
        self.init(optionalDirectory: directory)
    }

    private init(optionalDirectory: URL?) {
        self.directory = optionalDirectory
    }

    /// `<Application Support>/<bundle id>/Undra/<namespace>/db`: next to the `Kv` adapter's
    /// directory of the same namespace, so two apps on a Mac never share a database, nor two cores
    /// of one app.
    public static func defaultDirectory(namespace: String) -> URL {
        return KvAdapter.defaultDirectory(namespace: namespace, named: "db")
    }

    /// The file of database `name` of the core with the namespace given (the default adapter's
    /// directory for it, or the one this adapter was given).
    public func fileURL(forDatabase name: String, namespace: String = UndraCore.unnamedNamespace) -> URL {
        let directory = self.directory ?? SQLiteDbAdapter.defaultDirectory(namespace: namespace)
        return directory.appendingPathComponent("\(name).sqlite", isDirectory: false)
    }

    /// Opens `<directory>/<name>.sqlite` (or a private in-memory database for `":memory:"`) on a
    /// serial queue of its own, with extended result codes on.
    public func open(name: String) async throws(DbError) -> any DbConnection {
        let path: String
        if name == memoryDatabaseName {
            path = memoryDatabaseName
        } else {
            let directory = self.directory ?? SQLiteDbAdapter.defaultDirectory(namespace: UndraCore.unnamedNamespace)
            do {
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            } catch {
                throw DbError.unavailable("cannot create \(directory.path): \(error.localizedDescription)")
            }
            path = fileURL(forDatabase: name).path
        }
        let connection = SQLiteConnection(label: "dev.undra.db.\(name)")
        try await connection.open(path: path)
        return connection
    }

    // MARK: UndraAdapter

    public var portId: UInt32 {
        return StandardPorts.Db.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        let served = scoped(toNamespace: core.namespace)
        return bindings.add(DbBinding(adapter: served, busyTimeoutMs: DbPortAdapter.defaultBusyTimeoutMs)).portImpl()
    }

    /// This adapter when it has a directory, else one over the default directory of `namespace`.
    func scoped(toNamespace namespace: String) -> any DbAdapter {
        if directory != nil {
            return self
        }
        return SQLiteDbAdapter(directory: SQLiteDbAdapter.defaultDirectory(namespace: namespace))
    }

    public func detach() {
        bindings.detachAll()
    }
}

/// One SQLite connection on a serial queue. `handle` is only touched on `queue`.
final class SQLiteConnection: DbConnection, @unchecked Sendable {
    private let queue: DispatchQueue
    private var handle: OpaquePointer?

    init(label: String) {
        self.queue = DispatchQueue(label: label, qos: .userInitiated)
    }

    deinit {
        if let handle = handle {
            sqlite3_close_v2(handle)
        }
    }

    // MARK: DbConnection

    func open(path: String) async throws(DbError) {
        try await onQueue { () throws(DbError) -> Void in
            var opened: OpaquePointer?
            let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_NOMUTEX
            let code = sqlite3_open_v2(path, &opened, flags, nil)
            guard code == SQLITE_OK, let opened = opened else {
                let message = opened.map { String(cString: sqlite3_errmsg($0)) } ?? String(cString: sqlite3_errstr(code))
                if let opened = opened {
                    sqlite3_close_v2(opened)
                }
                throw SQLiteConnection.error(code: code, message: "\(message): \(path)")
            }
            sqlite3_extended_result_codes(opened, 1)
            self.handle = opened
        }
    }

    func execute(_ sql: String, _ params: [DbValue]) async throws(DbError) -> DbExecuted {
        return try await onQueue { () throws(DbError) -> DbExecuted in
            let db = try self.connection()
            let statement = try SQLiteConnection.prepareOne(db, sql)
            defer { sqlite3_finalize(statement) }
            try SQLiteConnection.bind(db, statement, params)
            while true {
                let code = sqlite3_step(statement)
                if code == SQLITE_ROW {
                    continue
                }
                if code == SQLITE_DONE {
                    break
                }
                throw SQLiteConnection.lastError(db, code)
            }
            // `sqlite3_changes`, not `sqlite3_changes64`: that one is SQLite 3.37, iOS 15.4 / macOS 12.3, above the
            // runtime's floor (ADR-045). A statement that changes more than 2^31 rows is not one an app's database runs.
            return DbExecuted(
                changes: UInt64(clamping: sqlite3_changes(db)),
                lastInsertId: sqlite3_last_insert_rowid(db)
            )
        }
    }

    func query(_ sql: String, _ params: [DbValue]) async throws(DbError) -> DbRows {
        return try await onQueue { () throws(DbError) -> DbRows in
            let db = try self.connection()
            let statement = try SQLiteConnection.prepareOne(db, sql)
            defer { sqlite3_finalize(statement) }
            try SQLiteConnection.bind(db, statement, params)
            let count = sqlite3_column_count(statement)
            var columns: [String] = []
            columns.reserveCapacity(Int(count))
            for column in 0 ..< count {
                columns.append(sqlite3_column_name(statement, column).map { String(cString: $0) } ?? "")
            }
            var rows: [[DbValue]] = []
            while true {
                let code = sqlite3_step(statement)
                if code == SQLITE_DONE {
                    break
                }
                guard code == SQLITE_ROW else {
                    throw SQLiteConnection.lastError(db, code)
                }
                var row: [DbValue] = []
                row.reserveCapacity(Int(count))
                for column in 0 ..< count {
                    row.append(SQLiteConnection.cell(statement, column))
                }
                rows.append(row)
            }
            return DbRows(columns: columns, rows: rows)
        }
    }

    func executeScript(_ sql: String) async throws(DbError) {
        try await onQueue { () throws(DbError) -> Void in
            let db = try self.connection()
            var message: UnsafeMutablePointer<CChar>?
            let code = sqlite3_exec(db, sql, nil, nil, &message)
            if code != SQLITE_OK {
                let text = message.map { String(cString: $0) } ?? String(cString: sqlite3_errmsg(db))
                sqlite3_free(message)
                throw SQLiteConnection.error(code: sqlite3_extended_errcode(db), message: text)
            }
        }
    }

    func close() async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            queue.async {
                if let handle = self.handle {
                    sqlite3_close_v2(handle)
                    self.handle = nil
                }
                continuation.resume()
            }
        }
    }

    // MARK: On the queue

    /// Runs `body` on the connection's queue.
    private func onQueue<Value: Sendable>(_ body: @escaping @Sendable () throws(DbError) -> Value) async throws(DbError) -> Value {
        let result = await withCheckedContinuation { (continuation: CheckedContinuation<Result<Value, DbError>, Never>) in
            queue.async {
                do throws(DbError) {
                    continuation.resume(returning: .success(try body()))
                } catch {
                    continuation.resume(returning: .failure(error))
                }
            }
        }
        return try result.get()
    }

    private func connection() throws(DbError) -> OpaquePointer {
        guard let handle = handle else {
            throw DbError.unavailable("the database is closed")
        }
        return handle
    }

    /// Prepares the one statement of `sql`; anything but whitespace and comments after it is
    /// `sql("only one statement per call: use a migration for several")`.
    static func prepareOne(_ db: OpaquePointer, _ sql: String) throws(DbError) -> OpaquePointer {
        let outcome = sql.withCString { (text: UnsafePointer<CChar>) -> Result<OpaquePointer, DbError> in
            var statement: OpaquePointer?
            var tail: UnsafePointer<CChar>?
            let code = sqlite3_prepare_v2(db, text, -1, &statement, &tail)
            guard code == SQLITE_OK else {
                return .failure(lastError(db, code))
            }
            guard let statement = statement else {
                return .failure(.sql(message: "the SQL holds no statement"))
            }
            // Whatever follows must prepare to nothing: whitespace, comments, empty statements.
            var rest = tail
            while let current = rest, current.pointee != 0 {
                var extra: OpaquePointer?
                var next: UnsafePointer<CChar>?
                let more = sqlite3_prepare_v2(db, current, -1, &extra, &next)
                if more != SQLITE_OK || extra != nil {
                    sqlite3_finalize(extra)
                    sqlite3_finalize(statement)
                    return .failure(.sql(message: "only one statement per call: use a migration for several"))
                }
                if next == current {
                    break
                }
                rest = next
            }
            return .success(statement)
        }
        return try outcome.get()
    }

    /// Binds `params` positionally after checking their number.
    static func bind(_ db: OpaquePointer, _ statement: OpaquePointer, _ params: [DbValue]) throws(DbError) {
        let expected = Int(sqlite3_bind_parameter_count(statement))
        guard expected == params.count else {
            throw DbError.sql(message: "the statement has \(expected) parameters, \(params.count) were given")
        }
        let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
        for (offset, value) in params.enumerated() {
            let index = Int32(offset + 1)
            let code: Int32
            switch value {
            case .null:
                code = sqlite3_bind_null(statement, index)
            case .integer(let integer):
                code = sqlite3_bind_int64(statement, index, integer)
            case .real(let real):
                code = sqlite3_bind_double(statement, index, real)
            case .text(let text):
                let length = UInt64(text.utf8.count)
                code = text.withCString { (pointer: UnsafePointer<CChar>) -> Int32 in
                    return sqlite3_bind_text64(statement, index, pointer, length, transient, UInt8(SQLITE_UTF8))
                }
            case .blob(let bytes):
                if bytes.isEmpty {
                    code = sqlite3_bind_zeroblob(statement, index, 0)
                } else {
                    code = bytes.withUnsafeBytes { (raw: UnsafeRawBufferPointer) -> Int32 in
                        return sqlite3_bind_blob64(statement, index, raw.baseAddress, UInt64(raw.count), transient)
                    }
                }
            }
            if code != SQLITE_OK {
                throw lastError(db, code)
            }
        }
    }

    /// The cell of `column` in the current row, by its storage class.
    static func cell(_ statement: OpaquePointer, _ column: Int32) -> DbValue {
        switch sqlite3_column_type(statement, column) {
        case SQLITE_INTEGER:
            return .integer(sqlite3_column_int64(statement, column))
        case SQLITE_FLOAT:
            return .real(sqlite3_column_double(statement, column))
        case SQLITE_TEXT:
            let count = Int(sqlite3_column_bytes(statement, column))
            guard let text = sqlite3_column_text(statement, column), count > 0 else {
                return .text("")
            }
            return .text(String(decoding: UnsafeBufferPointer(start: text, count: count), as: UTF8.self))
        case SQLITE_BLOB:
            let count = Int(sqlite3_column_bytes(statement, column))
            guard let blob = sqlite3_column_blob(statement, column), count > 0 else {
                return .blob([])
            }
            return .blob(Array(UnsafeRawBufferPointer(start: blob, count: count)))
        default:
            return .null
        }
    }

    /// The error of the last failed call on `db` (`code` is what that call returned).
    static func lastError(_ db: OpaquePointer, _ code: Int32) -> DbError {
        let extended = sqlite3_extended_errcode(db)
        // The extended code refines the primary one the call returned; a stale one does not.
        let chosen = (extended & 0xff) == (code & 0xff) ? extended : code
        return error(code: chosen, message: String(cString: sqlite3_errmsg(db)))
    }

    /// The `DbError` of an (extended) SQLite result code.
    static func error(code: Int32, message: String) -> DbError {
        switch code & 0xff {
        case SQLITE_BUSY, SQLITE_LOCKED:
            return .busy
        case SQLITE_CONSTRAINT:
            switch code {
            case 2067, 1555: // SQLITE_CONSTRAINT_UNIQUE, SQLITE_CONSTRAINT_PRIMARYKEY
                return .constraint(kind: .unique, message: message)
            case 1299: // SQLITE_CONSTRAINT_NOTNULL
                return .constraint(kind: .notNull, message: message)
            case 787: // SQLITE_CONSTRAINT_FOREIGNKEY
                return .constraint(kind: .foreignKey, message: message)
            case 275: // SQLITE_CONSTRAINT_CHECK
                return .constraint(kind: .check, message: message)
            default:
                return .constraint(kind: .other, message: message)
            }
        case SQLITE_CORRUPT, SQLITE_NOTADB:
            return .corrupt(message)
        case SQLITE_FULL:
            return .full
        case SQLITE_CANTOPEN, SQLITE_PERM, SQLITE_READONLY, SQLITE_IOERR:
            return .unavailable(message)
        default:
            return .sql(message: message)
        }
    }
}
