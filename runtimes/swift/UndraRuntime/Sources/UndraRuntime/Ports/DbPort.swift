// The `Db` port (ADR-048): the adapter interface an app implements to replace the default
// (`DbAdapter`, `DbConnection`: they only run SQL) and the binding that implements the port's
// semantics over one (`DbPortAdapter`): ids, name and version checks, the fixed open options,
// migrations in one transaction, one operation at a time per database, transactions that make
// the database's other statements wait (at most the busy timeout), commit, rollback and close.
//
//   open(name, migrations) -> DbOpened     execute(db, sql, params) -> DbExecuted
//   query(db, sql, params) -> DbRows       begin(db) -> u32     commit(tx)     rollback(tx)     close(db)

import Dispatch

/// Opens SQLite databases for the `Db` port (ADR-048).
///
/// Implement it to replace ``SQLiteDbAdapter`` and register it with ``DbPortAdapter``:
///
/// ```swift
/// let adapters = Adapters.platformDefault.replacing(DbPortAdapter(MyDatabases()))
/// ```
///
/// An adapter only runs SQL: the binding checks names and migrations, sets the open options,
/// migrates, orders the operations and keeps the transactions.
public protocol DbAdapter: Sendable {
    /// Opens (creating it if needed) the database `name`: `":memory:"` for a private in-memory
    /// database, else a name the binding has checked (1 to 64 of `A-Z a-z 0-9 . _ -`, not
    /// starting with `.`). A file that cannot be opened is ``DbError/unavailable(_:)``.
    func open(name: String) async throws(DbError) -> any DbConnection
}

/// One open database connection, as an adapter provides it. Every method runs on the adapter's
/// own thread for that database (the binding never calls two at once), and every failure is
/// typed by SQLite's result code, never by its message.
public protocol DbConnection: Sendable {
    /// Runs exactly one statement with `params` bound positionally and returns what it changed.
    /// Rows the statement returns are ignored. Trailing SQL is ``DbError/sql(message:)``, and so
    /// is a parameter count that does not match the statement's.
    func execute(_ sql: String, _ params: [DbValue]) async throws(DbError) -> DbExecuted

    /// Runs exactly one query with `params` bound positionally and returns the column names and
    /// every row, each cell by its storage class.
    func query(_ sql: String, _ params: [DbValue]) async throws(DbError) -> DbRows

    /// Runs several statements (a migration), without parameters.
    func executeScript(_ sql: String) async throws(DbError)

    /// Closes the connection.
    func close() async
}

/// Serves the `Db` port with a ``DbAdapter`` (ADR-048).
///
/// * Ids: databases and transactions share one counter from 1, never reused.
/// * `open(name, migrations)` checks the name (else `unavailable("invalid database name ...")`)
///   and that versions strictly increase from 1 and fit `user_version` (at most 2,147,483,647;
///   else `migration`), opens the database, runs `PRAGMA foreign_keys = ON`, `PRAGMA busy_timeout`
///   and `PRAGMA journal_mode = WAL`, reads `PRAGMA user_version`, refuses a database newer than
///   the newest migration, and runs every pending migration in one `BEGIN IMMEDIATE` transaction
///   that ends by setting `user_version` (any failure rolls all of them back: `migration(version,
///   message)`). The version is read again under that transaction's write lock, so two opens of
///   one file at once migrate it once.
/// * Every database runs one operation at a time, in arrival order. `begin` waits until no
///   transaction is active (at most the busy timeout, then `busy`) and runs `BEGIN IMMEDIATE`;
///   statements on the transaction's id run in it, statements on the database's id wait until it
///   ends (at most the busy timeout, then `busy`). `commit` runs `COMMIT` (a failure rolls back);
///   both end the transaction.
/// * An unknown or closed id is `unavailable("no open database or transaction <id>")`, an ended
///   transaction `unavailable("transaction <id> is over")`. `close` rolls back a running
///   transaction and closes the connection; closing again is not an error.
public final class DbPortAdapter: UndraAdapter, @unchecked Sendable {
    /// The busy timeout of the port: 5 seconds (ADR-048 §6).
    public static let defaultBusyTimeoutMs: UInt32 = 5_000

    private let adapter: any DbAdapter
    private let busyTimeoutMs: UInt32
    private let bindings = BindingSet<DbBinding>()

    /// Serves the port with `adapter`. `busyTimeoutMs` is how long a statement or a `begin` waits
    /// for a running transaction (and SQLite's own busy timeout); tests shorten it.
    public init(_ adapter: any DbAdapter, busyTimeoutMs: UInt32 = DbPortAdapter.defaultBusyTimeoutMs) {
        self.adapter = adapter
        self.busyTimeoutMs = busyTimeoutMs
    }

    public var portId: UInt32 {
        return StandardPorts.Db.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return bindings.add(DbBinding(adapter: adapter, busyTimeoutMs: busyTimeoutMs)).portImpl()
    }

    public func detach() {
        bindings.detachAll()
    }
}

// MARK: - Checks

/// The name that opens a private in-memory database.
let memoryDatabaseName = ":memory:"

/// Checks a database name: `":memory:"`, or 1 to 64 of `A-Z a-z 0-9 . _ -` not starting with `.`.
func validateDatabaseName(_ name: String) throws(DbError) {
    if name == memoryDatabaseName {
        return
    }
    let bytes = Array(name.utf8)
    let valid = !bytes.isEmpty && bytes.count <= 64 && bytes[0] != UInt8(ascii: ".")
        && bytes.allSatisfy { (byte: UInt8) -> Bool in
            switch byte {
            case UInt8(ascii: "a") ... UInt8(ascii: "z"), UInt8(ascii: "A") ... UInt8(ascii: "Z"),
                 UInt8(ascii: "0") ... UInt8(ascii: "9"), UInt8(ascii: "."), UInt8(ascii: "_"), UInt8(ascii: "-"):
                return true
            default:
                return false
            }
        }
    if !valid {
        throw DbError.unavailable(
            "invalid database name \(name.debugDescription): use 1 to 64 of A-Z a-z 0-9 . _ - (not starting with .), or \":memory:\""
        )
    }
}

/// The largest migration version: SQLite keeps the version in `PRAGMA user_version`, a signed
/// 32-bit integer, and records a larger value as 0.
let maximumMigrationVersion = UInt32(Int32.max)

/// Checks that migration versions strictly increase from 1 and fit `PRAGMA user_version`.
func validateMigrations(_ migrations: [DbMigration]) throws(DbError) {
    var last: UInt32 = 0
    for migration in migrations {
        if migration.version <= last {
            throw DbError.migration(
                version: migration.version,
                message: "migration versions must strictly increase, starting at 1"
            )
        }
        if migration.version > maximumMigrationVersion {
            throw DbError.migration(
                version: migration.version,
                message: "migration versions must be at most \(maximumMigrationVersion): SQLite keeps the version in a signed 32-bit integer (PRAGMA user_version)"
            )
        }
        last = migration.version
    }
}

// MARK: - The binding

/// The state of the `Db` port of one core.
final class DbBinding: DetachableBinding, @unchecked Sendable {
    /// One open database: its connection, its serial queue, its transaction and who waits for it.
    /// The mutable fields are only touched under the binding's lock.
    final class Database: @unchecked Sendable {
        let id: UInt32
        let connection: any DbConnection
        let runner = SerialRunner()
        /// The running transaction, until its `COMMIT` / `ROLLBACK` finished.
        var transaction: UInt32?
        /// Statements on the database's id and `begin`s waiting for the transaction to end, in
        /// arrival order.
        var waiters: [Waiter] = []

        init(id: UInt32, connection: any DbConnection) {
            self.id = id
            self.connection = connection
        }
    }

    /// Something waiting for a database's transaction to end.
    struct Waiter {
        let ticket: UInt64
        /// A `begin`: it takes the transaction slot when it goes.
        let isBegin: Bool
        /// Starts the operation (called under the lock with the transaction id a `begin` got).
        let go: @Sendable (UInt32) -> Void
        /// Fails it (called outside the lock).
        let fail: @Sendable (DbError) -> Void
        /// Its busy timeout.
        let timeout: Task<Void, Never>?
    }

    private struct State {
        var lastId: UInt32 = 0
        var lastTicket: UInt64 = 0
        var databases: [UInt32: Database] = [:]
        /// Running transactions (by id) and their database; a transaction leaves when its
        /// `commit` or `rollback` is accepted.
        var transactions: [UInt32: UInt32] = [:]
        var endedTransactions: Set<UInt32> = []
        var closedDatabases: Set<UInt32> = []
        var detached = false
    }

    private let adapter: any DbAdapter
    private let busyTimeoutMs: UInt32
    private let state = Guarded(State())

    init(adapter: any DbAdapter, busyTimeoutMs: UInt32) {
        self.adapter = adapter
        self.busyTimeoutMs = busyTimeoutMs
    }

    /// The method table of the port.
    func portImpl() -> PortImpl {
        return .async([
            StandardPorts.Db.open: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let name = try reader.readString()
                let migrations = try [DbMigration].undraDecode(&reader)
                try reader.finish()
                return try await answer { () async throws(DbError) -> DbOpened in
                    try await self.open(name: name, migrations: migrations)
                }
            },
            StandardPorts.Db.execute: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let id = try reader.readU32()
                let sql = try reader.readString()
                let params = try [DbValue].undraDecode(&reader)
                try reader.finish()
                return try await answer { () async throws(DbError) -> DbExecuted in
                    try await self.execute(id, sql, params)
                }
            },
            StandardPorts.Db.query: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let id = try reader.readU32()
                let sql = try reader.readString()
                let params = try [DbValue].undraDecode(&reader)
                try reader.finish()
                return try await answer { () async throws(DbError) -> DbRows in
                    try await self.query(id, sql, params)
                }
            },
            StandardPorts.Db.begin: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let id = try reader.readU32()
                try reader.finish()
                return try await answer { () async throws(DbError) -> UInt32 in
                    try await self.begin(id)
                }
            },
            StandardPorts.Db.commit: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let tx = try reader.readU32()
                try reader.finish()
                return try await answerUnit { () async throws(DbError) -> Void in
                    try await self.finish(tx, commit: true)
                }
            },
            StandardPorts.Db.rollback: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let tx = try reader.readU32()
                try reader.finish()
                return try await answerUnit { () async throws(DbError) -> Void in
                    try await self.finish(tx, commit: false)
                }
            },
            StandardPorts.Db.close: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let db = try reader.readU32()
                try reader.finish()
                return try await answerUnit { () async throws(DbError) -> Void in
                    try await self.close(db)
                }
            },
        ])
    }

    // MARK: open

    func open(name: String, migrations: [DbMigration]) async throws(DbError) -> DbOpened {
        try validateDatabaseName(name)
        try validateMigrations(migrations)
        let connection = try await adapter.open(name: name)
        let version: UInt32
        do throws(DbError) {
            version = try await prepare(connection, migrations: migrations)
        } catch {
            await connection.close()
            throw error
        }
        let id = state.withLock { (current: inout State) -> UInt32? in
            if current.detached {
                return nil
            }
            current.lastId += 1
            current.databases[current.lastId] = Database(id: current.lastId, connection: connection)
            return current.lastId
        }
        guard let id = id else {
            await connection.close()
            throw DbError.unavailable("the core shut down")
        }
        return DbOpened(db: id, version: version)
    }

    /// The fixed open options, then the migrations; returns the version reached.
    private func prepare(_ connection: any DbConnection, migrations: [DbMigration]) async throws(DbError) -> UInt32 {
        _ = try await connection.execute("PRAGMA foreign_keys = ON", [])
        _ = try await connection.execute("PRAGMA busy_timeout = \(busyTimeoutMs)", [])
        try await enableWal(connection)
        let current = try await DbBinding.userVersion(of: connection)
        guard let newest = migrations.last?.version else {
            return current
        }
        try DbBinding.refuseNewer(current, than: newest)
        guard current < newest else {
            return current
        }
        _ = try await connection.execute("BEGIN IMMEDIATE", [])
        // Another connection to the file (a second open of it, another process) may have migrated
        // it since the version was read: what is pending is decided again under the write lock.
        let pending: [DbMigration]
        do throws(DbError) {
            let locked = try await DbBinding.userVersion(of: connection)
            try DbBinding.refuseNewer(locked, than: newest)
            pending = migrations.filter { (migration: DbMigration) -> Bool in migration.version > locked }
        } catch {
            _ = try? await connection.execute("ROLLBACK", [])
            throw error
        }
        guard let first = pending.first else {
            _ = try? await connection.execute("ROLLBACK", [])
            return newest
        }
        var step = first.version
        do throws(DbError) {
            for migration in pending {
                step = migration.version
                try await connection.executeScript(migration.sql)
            }
            step = newest
            _ = try await connection.execute("PRAGMA user_version = \(newest)", [])
            _ = try await connection.execute("COMMIT", [])
        } catch {
            _ = try? await connection.execute("ROLLBACK", [])
            throw DbError.migration(version: step, message: error.description)
        }
        return newest
    }

    /// `PRAGMA journal_mode = WAL`. While another connection switches the same new file to WAL,
    /// SQLite answers `BUSY` at once instead of calling its busy handler, so the switch is retried
    /// here until the busy timeout, as any other lock is waited for.
    private func enableWal(_ connection: any DbConnection) async throws(DbError) {
        let deadline = DispatchTime.now().uptimeNanoseconds + UInt64(busyTimeoutMs) * 1_000_000
        while true {
            do throws(DbError) {
                _ = try await connection.execute("PRAGMA journal_mode = WAL", [])
                return
            } catch {
                guard error == .busy, DispatchTime.now().uptimeNanoseconds < deadline else {
                    throw error
                }
                try? await Task.sleep(nanoseconds: 2_000_000)
            }
        }
    }

    /// `PRAGMA user_version` of `connection`.
    private static func userVersion(of connection: any DbConnection) async throws(DbError) -> UInt32 {
        let rows = try await connection.query("PRAGMA user_version", [])
        if case .integer(let value)? = rows.rows.first?.first {
            return UInt32(clamping: value)
        }
        return 0
    }

    /// A database newer than the newest migration is refused, never downgraded.
    private static func refuseNewer(_ current: UInt32, than newest: UInt32) throws(DbError) {
        if current > newest {
            throw DbError.migration(
                version: current,
                message: "the database is at version \(current), newer than the newest migration (\(newest))"
            )
        }
    }

    // MARK: Statements

    func execute(_ id: UInt32, _ sql: String, _ params: [DbValue]) async throws(DbError) -> DbExecuted {
        return try await statement(id) { (connection: any DbConnection) async throws(DbError) -> DbExecuted in
            try await connection.execute(sql, params)
        }
    }

    func query(_ id: UInt32, _ sql: String, _ params: [DbValue]) async throws(DbError) -> DbRows {
        return try await statement(id) { (connection: any DbConnection) async throws(DbError) -> DbRows in
            try await connection.query(sql, params)
        }
    }

    /// Runs `body` on the connection `id` names, in its database's queue: at once on a
    /// transaction's id; on a database's id once no transaction is running (at most the busy
    /// timeout, then `busy`).
    private func statement<Value: Sendable>(
        _ id: UInt32,
        _ body: @escaping @Sendable (any DbConnection) async throws(DbError) -> Value
    ) async throws(DbError) -> Value {
        let result = await withCheckedContinuation { (continuation: CheckedContinuation<Result<Value, DbError>, Never>) in
            let run: @Sendable (Database) -> Void = { (database: Database) -> Void in
                database.runner.enqueue {
                    do throws(DbError) {
                        continuation.resume(returning: .success(try await body(database.connection)))
                    } catch {
                        continuation.resume(returning: .failure(error))
                    }
                }
            }
            let fail: @Sendable (DbError) -> Void = { (error: DbError) -> Void in
                continuation.resume(returning: .failure(error))
            }
            let refused = state.withLock { (current: inout State) -> DbError? in
                if let owner = current.transactions[id], let database = current.databases[owner] {
                    run(database)
                    return nil
                }
                if current.endedTransactions.contains(id) {
                    return DbError.unavailable("transaction \(id) is over")
                }
                guard let database = current.databases[id] else {
                    return DbError.unavailable("no open database or transaction \(id)")
                }
                if database.transaction == nil {
                    run(database)
                    return nil
                }
                self.wait(on: database, in: &current, isBegin: false, go: { (_: UInt32) -> Void in run(database) }, fail: fail)
                return nil
            }
            if let refused = refused {
                fail(refused)
            }
        }
        return try result.get()
    }

    // MARK: Transactions

    func begin(_ id: UInt32) async throws(DbError) -> UInt32 {
        let result = await withCheckedContinuation { (continuation: CheckedContinuation<Result<UInt32, DbError>, Never>) in
            let fail: @Sendable (DbError) -> Void = { (error: DbError) -> Void in
                continuation.resume(returning: .failure(error))
            }
            let start: @Sendable (Database, UInt32) -> Void = { [self] (database: Database, tx: UInt32) -> Void in
                database.runner.enqueue {
                    do throws(DbError) {
                        _ = try await database.connection.execute("BEGIN IMMEDIATE", [])
                        continuation.resume(returning: .success(tx))
                    } catch {
                        self.end(tx, of: database)
                        continuation.resume(returning: .failure(error))
                    }
                }
            }
            let refused = state.withLock { (current: inout State) -> DbError? in
                if current.transactions[id] != nil {
                    return DbError.sql(message: "a transaction cannot begin inside a transaction")
                }
                if current.endedTransactions.contains(id) {
                    return DbError.unavailable("transaction \(id) is over")
                }
                guard let database = current.databases[id] else {
                    return DbError.unavailable("no open database or transaction \(id)")
                }
                if database.transaction == nil {
                    let tx = DbBinding.claim(database, in: &current)
                    start(database, tx)
                    return nil
                }
                self.wait(on: database, in: &current, isBegin: true, go: { (tx: UInt32) -> Void in start(database, tx) }, fail: fail)
                return nil
            }
            if let refused = refused {
                fail(refused)
            }
        }
        return try result.get()
    }

    /// `commit` (`COMMIT`, rolled back when it fails) or `rollback` (`ROLLBACK`) of `tx`; either
    /// ends the transaction and wakes what waits for it.
    func finish(_ tx: UInt32, commit: Bool) async throws(DbError) {
        let result = await withCheckedContinuation { (continuation: CheckedContinuation<Result<Void, DbError>, Never>) in
            let database = state.withLock { (current: inout State) -> Result<Database, DbError> in
                guard let owner = current.transactions.removeValue(forKey: tx), let database = current.databases[owner] else {
                    if current.endedTransactions.contains(tx) {
                        return .failure(DbError.unavailable("transaction \(tx) is over"))
                    }
                    return .failure(DbError.unavailable("no open database or transaction \(tx)"))
                }
                current.endedTransactions.insert(tx)
                return .success(database)
            }
            switch database {
            case .failure(let error):
                continuation.resume(returning: .failure(error))
            case .success(let database):
                database.runner.enqueue { [self] in
                    var outcome: Result<Void, DbError> = .success(())
                    do throws(DbError) {
                        _ = try await database.connection.execute(commit ? "COMMIT" : "ROLLBACK", [])
                    } catch {
                        if commit {
                            _ = try? await database.connection.execute("ROLLBACK", [])
                        }
                        outcome = .failure(error)
                    }
                    self.end(tx, of: database)
                    continuation.resume(returning: outcome)
                }
            }
        }
        try result.get()
    }

    /// Takes the transaction slot of `database` for a new transaction id.
    private static func claim(_ database: Database, in current: inout State) -> UInt32 {
        current.lastId += 1
        let tx = current.lastId
        current.transactions[tx] = database.id
        database.transaction = tx
        return tx
    }

    /// `tx` is over: free the slot and let the waiters go, in arrival order, up to the first
    /// `begin` (which takes the slot).
    private func end(_ tx: UInt32, of database: Database) {
        state.withLock { (current: inout State) -> Void in
            current.transactions[tx] = nil
            current.endedTransactions.insert(tx)
            guard database.transaction == tx else {
                return
            }
            database.transaction = nil
            while !database.waiters.isEmpty, database.transaction == nil {
                let waiter = database.waiters.removeFirst()
                waiter.timeout?.cancel()
                if waiter.isBegin {
                    let next = DbBinding.claim(database, in: &current)
                    waiter.go(next)
                } else {
                    waiter.go(0)
                }
            }
        }
    }

    /// Queues `go` until the transaction of `database` ends; after the busy timeout it fails
    /// with `busy` instead.
    private func wait(
        on database: Database,
        in current: inout State,
        isBegin: Bool,
        go: @escaping @Sendable (UInt32) -> Void,
        fail: @escaping @Sendable (DbError) -> Void
    ) {
        current.lastTicket += 1
        let ticket = current.lastTicket
        let delay = UInt64(busyTimeoutMs) * 1_000_000
        let timeout = Task { [weak self, weak database] in
            try? await Task.sleep(nanoseconds: delay)
            guard !Task.isCancelled, let self = self, let database = database else {
                return
            }
            self.expire(ticket, of: database)
        }
        database.waiters.append(Waiter(ticket: ticket, isBegin: isBegin, go: go, fail: fail, timeout: timeout))
    }

    /// The busy timeout of waiter `ticket` passed.
    private func expire(_ ticket: UInt64, of database: Database) {
        let waiter = state.withLock { (current: inout State) -> Waiter? in
            guard let index = database.waiters.firstIndex(where: { $0.ticket == ticket }) else {
                return nil
            }
            return database.waiters.remove(at: index)
        }
        waiter?.fail(.busy)
    }

    // MARK: close

    func close(_ id: UInt32) async throws(DbError) {
        let found = state.withLock { (current: inout State) -> Result<(Database, [Waiter], Bool)?, DbError> in
            if current.closedDatabases.contains(id) {
                return .success(nil)
            }
            guard let database = current.databases.removeValue(forKey: id) else {
                return .failure(DbError.unavailable("no database \(id)"))
            }
            current.closedDatabases.insert(id)
            return .success(DbBinding.release(database, in: &current))
        }
        switch found {
        case .failure(let error):
            throw error
        case .success(nil):
            return
        case .success(let (database, waiters, rollBack)?):
            for waiter in waiters {
                waiter.fail(.unavailable("database \(id) was closed"))
            }
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                database.runner.enqueue {
                    if rollBack {
                        _ = try? await database.connection.execute("ROLLBACK", [])
                    }
                    await database.connection.close()
                    continuation.resume()
                }
            }
        }
    }

    /// Takes a closing database's waiters and its running transaction (which ends); returns them
    /// and whether a transaction has to be rolled back.
    private static func release(_ database: Database, in current: inout State) -> (Database, [Waiter], Bool) {
        let waiters = database.waiters
        database.waiters = []
        for waiter in waiters {
            waiter.timeout?.cancel()
        }
        var rollBack = false
        if let tx = database.transaction {
            rollBack = current.transactions.removeValue(forKey: tx) != nil
            current.endedTransactions.insert(tx)
            database.transaction = nil
        }
        return (database, waiters, rollBack)
    }

    /// The core shut down: every database still open is closed (a running transaction rolled
    /// back).
    func detach() {
        let released = state.withLock { (current: inout State) -> [(Database, [Waiter], Bool)] in
            current.detached = true
            let open = Array(current.databases.values)
            current.databases = [:]
            return open.map { (database: Database) -> (Database, [Waiter], Bool) in
                current.closedDatabases.insert(database.id)
                return DbBinding.release(database, in: &current)
            }
        }
        for (database, waiters, rollBack) in released {
            for waiter in waiters {
                waiter.fail(.unavailable("the core shut down"))
            }
            database.runner.enqueue {
                if rollBack {
                    _ = try? await database.connection.execute("ROLLBACK", [])
                }
                await database.connection.close()
            }
        }
    }
}

// MARK: - SerialRunner

/// Runs asynchronous jobs one at a time, in the order they were queued.
final class SerialRunner: @unchecked Sendable {
    private struct State {
        var jobs: [@Sendable () async -> Void] = []
        var running = false
    }

    private let state = Guarded(State())

    /// Queues `job`; it starts once every job queued before it finished.
    func enqueue(_ job: @escaping @Sendable () async -> Void) {
        let start = state.withLock { (current: inout State) -> Bool in
            current.jobs.append(job)
            if current.running {
                return false
            }
            current.running = true
            return true
        }
        if start {
            Task {
                await self.drain()
            }
        }
    }

    private func drain() async {
        while let job = next() {
            await job()
        }
    }

    private func next() -> (@Sendable () async -> Void)? {
        return state.withLock { (current: inout State) -> (@Sendable () async -> Void)? in
            if current.jobs.isEmpty {
                current.running = false
                return nil
            }
            return current.jobs.removeFirst()
        }
    }
}
