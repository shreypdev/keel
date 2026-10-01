import Foundation
import XCTest
@testable import UndraRuntime

/// Wraps a `DbAdapter` and records what every connection is asked to run, with optional delays
/// and failures per SQL, so the binding's order of operations can be read back.
final class RecordingDb: DbAdapter, @unchecked Sendable {
    struct Entry: Equatable {
        let connection: Int
        let what: String
    }

    private let inner: any DbAdapter
    private let state = Locked<(log: [Entry], connections: Int, delays: [String: UInt64], failures: [String: DbError])>(([], 0, [:], [:]))

    init(_ inner: any DbAdapter) {
        self.inner = inner
    }

    var log: [Entry] {
        return state.withLock { $0.log }
    }

    /// What connection `connection` ran, in order (`"open <name>"`, the SQL, `"script <sql>"`, `"close"`).
    func ran(_ connection: Int) -> [String] {
        return log.filter { $0.connection == connection }.map(\.what)
    }

    /// Delays every statement whose SQL contains `fragment` by `milliseconds`.
    func delay(_ fragment: String, milliseconds: UInt64) {
        state.withLock { $0.delays[fragment] = milliseconds }
    }

    /// Fails every statement whose SQL is exactly `sql` with `error`.
    func fail(_ sql: String, with error: DbError?) {
        state.withLock { $0.failures[sql] = error }
    }

    fileprivate func note(_ connection: Int, _ what: String) {
        state.withLock { $0.log.append(Entry(connection: connection, what: what)) }
    }

    fileprivate func script(for sql: String) -> (delay: UInt64, failure: DbError?) {
        return state.withLock { current in
            let delay = current.delays.first(where: { sql.contains($0.key) })?.value ?? 0
            return (delay, current.failures[sql])
        }
    }

    func open(name: String) async throws(DbError) -> any DbConnection {
        let number = state.withLock { (current: inout (log: [Entry], connections: Int, delays: [String: UInt64], failures: [String: DbError])) -> Int in
            current.connections += 1
            return current.connections
        }
        note(number, "open \(name)")
        let connection = try await inner.open(name: name)
        return Connection(number: number, inner: connection, recorder: self)
    }

    final class Connection: DbConnection, @unchecked Sendable {
        let number: Int
        let inner: any DbConnection
        let recorder: RecordingDb

        init(number: Int, inner: any DbConnection, recorder: RecordingDb) {
            self.number = number
            self.inner = inner
            self.recorder = recorder
        }

        private func run<Value>(_ sql: String, _ label: String, _ body: () async throws(DbError) -> Value) async throws(DbError) -> Value {
            recorder.note(number, label)
            let (delay, failure) = recorder.script(for: sql)
            if delay > 0 {
                try? await Task.sleep(nanoseconds: delay * 1_000_000)
            }
            if let failure = failure {
                recorder.note(number, "failed \(label)")
                throw failure
            }
            let value = try await body()
            recorder.note(number, "done \(label)")
            return value
        }

        func execute(_ sql: String, _ params: [DbValue]) async throws(DbError) -> DbExecuted {
            return try await run(sql, sql) { () async throws(DbError) -> DbExecuted in try await inner.execute(sql, params) }
        }

        func query(_ sql: String, _ params: [DbValue]) async throws(DbError) -> DbRows {
            return try await run(sql, sql) { () async throws(DbError) -> DbRows in try await inner.query(sql, params) }
        }

        func executeScript(_ sql: String) async throws(DbError) {
            try await run(sql, "script \(sql)") { () async throws(DbError) -> Void in try await inner.executeScript(sql) }
        }

        func close() async {
            recorder.note(number, "close")
            await inner.close()
        }
    }
}

/// A fresh temporary directory, removed by the test's teardown.
func temporaryDirectory(_ test: XCTestCase) throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("undra-db-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    test.addTeardownBlock {
        try? FileManager.default.removeItem(at: url)
    }
    return url
}

let notesMigrations = [
    DbMigration(version: 1, sql: "CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0)"),
    DbMigration(version: 2, sql: "CREATE INDEX notes_done ON notes (done); CREATE TABLE tags (note INTEGER NOT NULL REFERENCES notes (id), tag TEXT NOT NULL)"),
]

// MARK: - The Db binding

final class DbBindingTests: XCTestCase {
    private func makeBinding(busyTimeoutMs: UInt32 = 5_000) throws -> (DbBinding, RecordingDb) {
        let recorder = RecordingDb(SQLiteDbAdapter(directory: try temporaryDirectory(self)))
        return (DbBinding(adapter: recorder, busyTimeoutMs: busyTimeoutMs), recorder)
    }

    private func count(_ binding: DbBinding, _ id: UInt32) async throws -> Int64 {
        let rows = try await binding.query(id, "SELECT COUNT(*) FROM notes", [])
        guard case .integer(let value)? = rows.rows.first?.first else {
            XCTFail("no count in \(rows)")
            return -1
        }
        return value
    }

    func testOpenSetsTheOptionsAndRunsThePendingMigrationsInOneTransaction() async throws {
        let (binding, recorder) = try makeBinding()
        let opened = try await binding.open(name: "app", migrations: notesMigrations)
        XCTAssertEqual(opened, DbOpened(db: 1, version: 2))
        XCTAssertEqual(recorder.ran(1).filter { !$0.hasPrefix("done ") }, [
            "open app",
            "PRAGMA foreign_keys = ON",
            "PRAGMA busy_timeout = 5000",
            "PRAGMA journal_mode = WAL",
            "PRAGMA user_version",
            "BEGIN IMMEDIATE",
            "script \(notesMigrations[0].sql)",
            "script \(notesMigrations[1].sql)",
            "PRAGMA user_version = 2",
            "COMMIT",
        ])
        try await binding.close(1)
        // Opened again: nothing to migrate; with a third migration, only that one runs.
        let again = try await binding.open(name: "app", migrations: notesMigrations)
        XCTAssertEqual(again, DbOpened(db: 2, version: 2))
        XCTAssertFalse(recorder.ran(2).contains("BEGIN IMMEDIATE"))
        let third = notesMigrations + [DbMigration(version: 3, sql: "CREATE TABLE extra (x)")]
        let upgraded = try await binding.open(name: "app", migrations: third)
        XCTAssertEqual(upgraded, DbOpened(db: 3, version: 3))
        XCTAssertEqual(recorder.ran(3).filter { $0.hasPrefix("script") }, ["script CREATE TABLE extra (x)"])
        // No migrations: the version it has.
        let plain = try await binding.open(name: "app", migrations: [])
        XCTAssertEqual(plain, DbOpened(db: 4, version: 3))
    }

    func testANewerDatabaseIsRefusedAndClosedNeverDowngraded() async throws {
        let (binding, recorder) = try makeBinding()
        _ = try await binding.open(name: "app", migrations: notesMigrations)
        await expectThrows(DbError.migration(version: 2, message: "the database is at version 2, newer than the newest migration (1)")) { () async throws(DbError) -> DbOpened in
            try await binding.open(name: "app", migrations: [notesMigrations[0]])
        }
        XCTAssertEqual(recorder.ran(2).last, "close")
    }

    func testAFailingMigrationRollsBackEveryMigrationOfTheOpen() async throws {
        let (binding, recorder) = try makeBinding()
        let broken = [
            DbMigration(version: 1, sql: "CREATE TABLE a (x INTEGER)"),
            DbMigration(version: 2, sql: "CREATE TABLE b (y INTEGER); INSERT INTO nowhere VALUES (1)"),
        ]
        await expectThrows(DbError.migration(version: 2, message: "SQL error: no such table: nowhere")) { () async throws(DbError) -> DbOpened in
            try await binding.open(name: "m", migrations: broken)
        }
        XCTAssertEqual(Array(recorder.ran(1).filter { !$0.hasPrefix("done ") }.suffix(3)), ["script \(broken[1].sql)", "ROLLBACK", "close"])
        // Migration 1 did not survive: its CREATE TABLE a runs again without a conflict.
        let good = [broken[0], DbMigration(version: 2, sql: "CREATE TABLE b (y INTEGER); INSERT INTO a VALUES (1)")]
        let opened = try await binding.open(name: "m", migrations: good)
        XCTAssertEqual(opened.version, 2)
    }

    func testNamesAndVersionsAreCheckedBeforeTheAdapterIsAsked() async throws {
        let (binding, recorder) = try makeBinding()
        for bad in ["", ".hidden", "a/b", "../escape", "a b", String(repeating: "x", count: 65), "é"] {
            do {
                _ = try await binding.open(name: bad, migrations: [])
                XCTFail("\(bad) was accepted")
            } catch {
                guard case .unavailable(let message) = error else {
                    return XCTFail("\(bad): \(error)")
                }
                XCTAssertTrue(message.hasPrefix("invalid database name"), message)
            }
        }
        for versions in [[0], [2, 2], [1, 3, 2]] {
            let migrations = versions.map { DbMigration(version: UInt32($0), sql: "") }
            do {
                _ = try await binding.open(name: "app", migrations: migrations)
                XCTFail("\(versions) was accepted")
            } catch {
                guard case .migration(_, let message) = error else {
                    return XCTFail("\(versions): \(error)")
                }
                XCTAssertEqual(message, "migration versions must strictly increase, starting at 1")
            }
        }
        XCTAssertEqual(recorder.log, [])
        for good in ["app", "a.b_c-1", ":memory:", String(repeating: "x", count: 64)] {
            _ = try await binding.open(name: good, migrations: [])
        }
    }

    func testDatabasesAndTransactionsShareOneCounter() async throws {
        let (binding, _) = try makeBinding()
        let first = try await binding.open(name: ":memory:", migrations: notesMigrations)
        let tx = try await binding.begin(first.db)
        let second = try await binding.open(name: ":memory:", migrations: [])
        XCTAssertEqual([first.db, tx, second.db], [1, 2, 3])
        try await binding.finish(tx, commit: true)
        let next = try await binding.begin(first.db)
        XCTAssertEqual(next, 4)
    }

    func testStatementsOnTheTransactionRunInItAndTheDatabaseWaitsForItsEnd() async throws {
        let (binding, recorder) = try makeBinding()
        let db = try await binding.open(name: "app", migrations: notesMigrations).db
        let tx = try await binding.begin(db)
        _ = try await binding.execute(tx, "INSERT INTO notes (title) VALUES (?)", [.text("in tx")])
        let outside = Task { () async throws(DbError) -> DbExecuted in
            try await binding.execute(db, "INSERT INTO notes (title) VALUES (?)", [.text("outside")])
        }
        try await Task.sleep(nanoseconds: 100_000_000)
        let inserts = recorder.ran(1).filter { $0 == "INSERT INTO notes (title) VALUES (?)" }
        XCTAssertEqual(inserts.count, 1, "the outer statement waits for the transaction")
        let inTx = try await count(binding, tx)
        XCTAssertEqual(inTx, 1, "the transaction's statements are not blocked by the waiting one")
        try await binding.finish(tx, commit: true)
        let executed = try await outside.value
        XCTAssertEqual(executed.lastInsertId, 2, "the waiting statement ran after the commit")
        let total = try await count(binding, db)
        XCTAssertEqual(total, 2)
        let commits = recorder.ran(1)
        let commitAt = try XCTUnwrap(commits.firstIndex(of: "COMMIT"))
        let outsideAt = try XCTUnwrap(commits.lastIndex(of: "INSERT INTO notes (title) VALUES (?)"))
        XCTAssertGreaterThan(outsideAt, commitAt)
    }

    func testAStatementOnTheDatabaseDuringATransactionIsBusyAfterTheTimeout() async throws {
        let (binding, _) = try makeBinding(busyTimeoutMs: 150)
        let db = try await binding.open(name: "app", migrations: notesMigrations).db
        let tx = try await binding.begin(db)
        let started = Date()
        await expectThrows(DbError.busy) { () async throws(DbError) -> DbExecuted in
            try await binding.execute(db, "INSERT INTO notes (title) VALUES ('x')", [])
        }
        XCTAssertGreaterThanOrEqual(Date().timeIntervalSince(started), 0.14)
        await expectThrows(DbError.busy) { () async throws(DbError) -> UInt32 in
            try await binding.begin(db)
        }
        try await binding.finish(tx, commit: false)
        let count = try await count(binding, db)
        XCTAssertEqual(count, 0)
    }

    func testABeginWaitsForTheRunningTransaction() async throws {
        let (binding, _) = try makeBinding()
        let db = try await binding.open(name: "app", migrations: notesMigrations).db
        let first = try await binding.begin(db)
        let second = Task { () async throws(DbError) -> UInt32 in try await binding.begin(db) }
        try await Task.sleep(nanoseconds: 50_000_000)
        _ = try await binding.execute(first, "INSERT INTO notes (title) VALUES ('a')", [])
        try await binding.finish(first, commit: true)
        let next = try await second.value
        XCTAssertEqual(next, first + 1)
        _ = try await binding.execute(next, "INSERT INTO notes (title) VALUES ('b')", [])
        try await binding.finish(next, commit: false)
        let total = try await count(binding, db)
        XCTAssertEqual(total, 1, "the second transaction rolled back")
    }

    func testEndedUnknownAndNestedIdsAreTyped() async throws {
        let (binding, _) = try makeBinding()
        let db = try await binding.open(name: ":memory:", migrations: notesMigrations).db
        let tx = try await binding.begin(db)
        await expectThrows(DbError.sql(message: "a transaction cannot begin inside a transaction")) { () async throws(DbError) -> UInt32 in
            try await binding.begin(tx)
        }
        try await binding.finish(tx, commit: true)
        await expectThrows(DbError.unavailable("transaction \(tx) is over")) { () async throws(DbError) -> Void in
            try await binding.finish(tx, commit: true)
        }
        await expectThrows(DbError.unavailable("transaction \(tx) is over")) { () async throws(DbError) -> DbRows in
            try await binding.query(tx, "SELECT 1", [])
        }
        await expectThrows(DbError.unavailable("no open database or transaction 99")) { () async throws(DbError) -> DbExecuted in
            try await binding.execute(99, "SELECT 1", [])
        }
        await expectThrows(DbError.unavailable("no open database or transaction 99")) { () async throws(DbError) -> Void in
            try await binding.finish(99, commit: false)
        }
        await expectThrows(DbError.unavailable("no database 99")) { () async throws(DbError) -> Void in
            try await binding.close(99)
        }
    }

    func testAFailedCommitRollsBackAndEndsTheTransaction() async throws {
        let (binding, recorder) = try makeBinding()
        let db = try await binding.open(name: "app", migrations: notesMigrations).db
        let tx = try await binding.begin(db)
        _ = try await binding.execute(tx, "INSERT INTO notes (title) VALUES ('a')", [])
        recorder.fail("COMMIT", with: .full)
        await expectThrows(DbError.full) { () async throws(DbError) -> Void in
            try await binding.finish(tx, commit: true)
        }
        recorder.fail("COMMIT", with: nil)
        let ran = recorder.ran(1)
        XCTAssertEqual(Array(ran.suffix(3)), ["failed COMMIT", "ROLLBACK", "done ROLLBACK"])
        let total = try await count(binding, db)
        XCTAssertEqual(total, 0, "the database is usable again and the insert is gone")
    }

    func testCloseRollsBackFailsTheWaitersAndIsIdempotent() async throws {
        let (binding, recorder) = try makeBinding()
        let db = try await binding.open(name: "app", migrations: notesMigrations).db
        let tx = try await binding.begin(db)
        _ = try await binding.execute(tx, "INSERT INTO notes (title) VALUES ('a')", [])
        let waiting = Task { await capture { () async throws(DbError) -> DbRows in try await binding.query(db, "SELECT * FROM notes", []) } }
        try await Task.sleep(nanoseconds: 50_000_000)
        try await binding.close(db)
        await expectThrows(DbError.unavailable("database \(db) was closed")) { () async throws(DbError) -> DbRows in try await waiting.value.get() }
        XCTAssertEqual(Array(recorder.ran(1).suffix(3)), ["ROLLBACK", "done ROLLBACK", "close"])
        try await binding.close(db)
        await expectThrows(DbError.unavailable("no open database or transaction \(db)")) { () async throws(DbError) -> DbRows in
            try await binding.query(db, "SELECT 1", [])
        }
        await expectThrows(DbError.unavailable("transaction \(tx) is over")) { () async throws(DbError) -> DbRows in
            try await binding.query(tx, "SELECT 1", [])
        }
        let reopened = try await binding.open(name: "app", migrations: notesMigrations).db
        let total = try await count(binding, reopened)
        XCTAssertEqual(total, 0, "the transaction was rolled back")
    }

    func testOperationsOnOneDatabaseRunOneAtATimeInArrivalOrder() async throws {
        let (binding, recorder) = try makeBinding()
        let db = try await binding.open(name: "app", migrations: notesMigrations).db
        recorder.delay("'slow'", milliseconds: 100)
        let slow = Task { () async throws(DbError) -> DbExecuted in
            try await binding.execute(db, "INSERT INTO notes (title) VALUES ('slow')", [])
        }
        try await Task.sleep(nanoseconds: 20_000_000)
        let fast = try await binding.execute(db, "INSERT INTO notes (title) VALUES ('fast')", [])
        let first = try await slow.value
        XCTAssertEqual(first.lastInsertId, 1)
        XCTAssertEqual(fast.lastInsertId, 2, "the second statement waited for the first")
        let ran = recorder.ran(1)
        let slowDone = try XCTUnwrap(ran.firstIndex(of: "done INSERT INTO notes (title) VALUES ('slow')"))
        let fastStart = try XCTUnwrap(ran.firstIndex(of: "INSERT INTO notes (title) VALUES ('fast')"))
        XCTAssertLessThan(slowDone, fastStart)
    }

    func testDetachClosesEveryDatabase() async throws {
        let (binding, recorder) = try makeBinding()
        let db = try await binding.open(name: "app", migrations: notesMigrations).db
        _ = try await binding.begin(db)
        binding.detach()
        await eventually("the close") { recorder.ran(1).last == "close" }
        XCTAssertTrue(recorder.ran(1).contains("ROLLBACK"))
        await expectThrows(DbError.unavailable("the core shut down")) { () async throws(DbError) -> DbOpened in
            try await binding.open(name: "other", migrations: [])
        }
    }

    func testThePortImplDecodesArgumentsAndAnswersTypedErrors() async throws {
        let port = DbPortAdapter(SQLiteDbAdapter(directory: try temporaryDirectory(self)), busyTimeoutMs: 100)
        XCTAssertEqual(port.portId, 0x559e_da82)
        let core = try makeCore(FakeTransport())
        let impl = port.makePortImpl(core: core)
        let openArgs = encodeArgs { (w: inout UndraWriter) -> Void in
            w.writeString("app")
            notesMigrations.undraEncode(&w)
        }
        let opened = try DbOpened.undraDecoded(from: await PortCaller.callAsync(impl, StandardPorts.Db.open, openArgs))
        XCTAssertEqual(opened, DbOpened(db: 1, version: 2))
        let insert = encodeArgs { (w: inout UndraWriter) -> Void in
            w.writeU32(1)
            w.writeString("INSERT INTO notes (id, title) VALUES (?, ?)")
            [DbValue.integer(7), .text("milk")].undraEncode(&w)
        }
        let executed = try DbExecuted.undraDecoded(from: await PortCaller.callAsync(impl, StandardPorts.Db.execute, insert))
        XCTAssertEqual(executed, DbExecuted(changes: 1, lastInsertId: 7))
        do {
            _ = try await PortCaller.callAsync(impl, StandardPorts.Db.execute, insert)
            XCTFail("a duplicate id is a constraint error")
        } catch let error as UndraPortError {
            let decoded = try DbError.undraDecoded(from: error.body)
            XCTAssertEqual(decoded, .constraint(kind: .unique, message: "UNIQUE constraint failed: notes.id"))
        }
        let begin = encodeArgs { (w: inout UndraWriter) -> Void in w.writeU32(1) }
        let tx = try UInt32.undraDecoded(from: await PortCaller.callAsync(impl, StandardPorts.Db.begin, begin))
        let select = encodeArgs { (w: inout UndraWriter) -> Void in
            w.writeU32(tx)
            w.writeString("SELECT id, title FROM notes")
            [DbValue]().undraEncode(&w)
        }
        let rows = try DbRows.undraDecoded(from: await PortCaller.callAsync(impl, StandardPorts.Db.query, select))
        XCTAssertEqual(rows, DbRows(columns: ["id", "title"], rows: [[.integer(7), .text("milk")]]))
        let txArgs = encodeArgs { (w: inout UndraWriter) -> Void in w.writeU32(tx) }
        let rolledBack = try await PortCaller.callAsync(impl, StandardPorts.Db.rollback, txArgs)
        XCTAssertEqual(rolledBack, [])
        let closed = try await PortCaller.callAsync(impl, StandardPorts.Db.close, begin)
        XCTAssertEqual(closed, [])
        port.detach()
        core.shutdown()
    }
}
