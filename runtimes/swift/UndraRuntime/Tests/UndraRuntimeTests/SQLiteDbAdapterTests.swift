import Foundation
import XCTest
@testable import UndraRuntime

/// The Db failure-injection suite (ADR-048, the ports-v2 brief §5) against the real
/// `SQLiteDbAdapter` in a temporary directory, through the binding the core talks to.
final class SQLiteDbAdapterTests: XCTestCase {
    private var directory: URL!

    override func setUpWithError() throws {
        directory = try temporaryDirectory(self)
    }

    private func binding(busyTimeoutMs: UInt32 = 5_000) -> DbBinding {
        return DbBinding(adapter: SQLiteDbAdapter(directory: directory), busyTimeoutMs: busyTimeoutMs)
    }

    private func expectConstraint(
        _ kind: DbConstraint,
        file: StaticString = #filePath,
        line: UInt = #line,
        _ body: () async throws(DbError) -> DbExecuted
    ) async {
        do {
            _ = try await body()
            XCTFail("expected a \(kind) constraint error", file: file, line: line)
        } catch {
            guard case .constraint(let got, let message) = error else {
                return XCTFail("expected a \(kind) constraint error, got \(error)", file: file, line: line)
            }
            XCTAssertEqual(got, kind, message, file: file, line: line)
            XCTAssertFalse(message.isEmpty, file: file, line: line)
        }
    }

    func testEachConstraintKindIsTypedByItsCode() async throws {
        let db = binding()
        let schema = [DbMigration(version: 1, sql: """
            CREATE TABLE parent (id INTEGER PRIMARY KEY, code TEXT UNIQUE);
            CREATE TABLE child (id INTEGER PRIMARY KEY, parent INTEGER NOT NULL REFERENCES parent (id),
                                size INTEGER CHECK (size > 0));
            CREATE TRIGGER no_zero BEFORE INSERT ON parent WHEN NEW.code = 'zero'
                BEGIN SELECT RAISE(ABORT, 'zero is reserved'); END;
            """)]
        let id = try await db.open(name: "constraints", migrations: schema).db
        _ = try await db.execute(id, "INSERT INTO parent (id, code) VALUES (1, 'a')", [])
        await expectConstraint(.unique) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "INSERT INTO parent (id, code) VALUES (2, 'a')", [])
        }
        await expectConstraint(.unique) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "INSERT INTO parent (id, code) VALUES (1, 'b')", [])
        }
        await expectConstraint(.notNull) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "INSERT INTO child (parent, size) VALUES (?, 1)", [.null])
        }
        await expectConstraint(.foreignKey) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "INSERT INTO child (parent, size) VALUES (99, 1)", [])
        }
        await expectConstraint(.check) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "INSERT INTO child (parent, size) VALUES (1, 0)", [])
        }
        await expectConstraint(.other) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "INSERT INTO parent (code) VALUES ('zero')", [])
        }
    }

    func testSqlErrorsTheOneStatementRuleAndTheParameterCount() async throws {
        let db = binding()
        let id = try await db.open(name: ":memory:", migrations: []).db
        await expectThrows(DbError.sql(message: "no such table: missing")) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "INSERT INTO missing VALUES (1)", [])
        }
        do {
            _ = try await db.execute(id, "SELEC 1", [])
            XCTFail("a syntax error")
        } catch {
            guard case .sql(let message) = error else {
                return XCTFail("\(error)")
            }
            XCTAssertTrue(message.contains("syntax error"), message)
        }
        let several = DbError.sql(message: "only one statement per call: use a migration for several")
        await expectThrows(several) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "CREATE TABLE t (x); CREATE TABLE u (y)", [])
        }
        await expectThrows(several) { () async throws(DbError) -> DbRows in
            try await db.query(id, "SELECT 1; SELECT 2", [])
        }
        await expectThrows(several) { () async throws(DbError) -> DbRows in
            try await db.query(id, "SELECT 1; nonsense", [])
        }
        // Whitespace, comments and empty statements after the one are fine.
        let one = try await db.query(id, "SELECT 1 AS one; -- a comment\n /* another */ ;  ", [])
        XCTAssertEqual(one, DbRows(columns: ["one"], rows: [[.integer(1)]]))
        _ = try await db.execute(id, "CREATE TABLE t (a, b);", [])
        await expectThrows(DbError.sql(message: "the statement has 2 parameters, 1 were given")) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "INSERT INTO t VALUES (?, ?)", [.integer(1)])
        }
        await expectThrows(DbError.sql(message: "the statement has 0 parameters, 1 were given")) { () async throws(DbError) -> DbRows in
            try await db.query(id, "SELECT * FROM t", [.integer(1)])
        }
        await expectThrows(DbError.sql(message: "the SQL holds no statement")) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "  -- nothing\n", [])
        }
        // Numbered parameters bind by position too.
        let numbered = try await db.query(id, "SELECT ?2 AS b, ?1 AS a", [.integer(1), .text("two")])
        XCTAssertEqual(numbered.rows, [[.text("two"), .integer(1)]])
    }

    func testEveryStorageClassComesBackTyped() async throws {
        let db = binding()
        let id = try await db.open(name: ":memory:", migrations: []).db
        _ = try await db.execute(id, "CREATE TABLE cells (v)", [])
        let values: [DbValue] = [
            .integer(.min), .integer(.max), .integer(0), .integer(-9_007_199_254_740_993),
            .real(1.5), .real(-0.25), .real(1e300), .real(-1e-300),
            .text(""), .text("é😀"), .text("a\u{0}b"),
            .blob([]), .blob([0, 255, 7]), .blob(Array(repeating: 0xAB, count: 100_000)),
            .null,
        ]
        for value in values {
            let done = try await db.execute(id, "INSERT INTO cells (v) VALUES (?)", [value])
            XCTAssertEqual(done.changes, 1)
        }
        let rows = try await db.query(id, "SELECT v, typeof(v) AS t FROM cells ORDER BY rowid", [])
        XCTAssertEqual(rows.columns, ["v", "t"])
        XCTAssertEqual(rows.rows.map { $0[0] }, values)
        XCTAssertEqual(rows.rows.map { $0[1] }, values.map { DbValue.text($0.storageClass) })
        // A NaN is stored as NULL by SQLite: reals come back NaN-free.
        _ = try await db.execute(id, "DELETE FROM cells", [])
        _ = try await db.execute(id, "INSERT INTO cells (v) VALUES (?)", [.real(.nan)])
        let nan = try await db.query(id, "SELECT v FROM cells", [])
        XCTAssertEqual(nan.rows, [[.null]])
        // changes and last_insert_id.
        _ = try await db.execute(id, "INSERT INTO cells (rowid, v) VALUES (41, 1)", [])
        let inserted = try await db.execute(id, "INSERT INTO cells (v) VALUES (2)", [])
        XCTAssertEqual(inserted.lastInsertId, 42)
        let updated = try await db.execute(id, "UPDATE cells SET v = 0", [])
        XCTAssertEqual(updated.changes, 3)
        let empty = try await db.query(id, "SELECT v FROM cells WHERE 0", [])
        XCTAssertEqual(empty, DbRows(columns: ["v"], rows: []))
    }

    func testABusyDatabaseIsTypedBusy() async throws {
        // Within one binding: a statement on the database while a transaction runs.
        let db = binding(busyTimeoutMs: 150)
        let id = try await db.open(name: "busy", migrations: notesMigrations).db
        let tx = try await db.begin(id)
        await expectThrows(DbError.busy) { () async throws(DbError) -> DbExecuted in
            try await db.execute(id, "INSERT INTO notes (title) VALUES ('x')", [])
        }
        // Across two connections to one file: SQLite's own BUSY, after its busy timeout.
        let other = binding(busyTimeoutMs: 150)
        let second = try await other.open(name: "busy", migrations: notesMigrations).db
        await expectThrows(DbError.busy) { () async throws(DbError) -> DbExecuted in
            try await other.execute(second, "INSERT INTO notes (title) VALUES ('y')", [])
        }
        try await db.finish(tx, commit: true)
        let done = try await other.execute(second, "INSERT INTO notes (title) VALUES ('y')", [])
        XCTAssertEqual(done.changes, 1)
    }

    func testACorruptFileIsTypedCorrupt() async throws {
        let adapter = SQLiteDbAdapter(directory: directory)
        try Data(repeating: 0x5A, count: 8192).write(to: adapter.fileURL(forDatabase: "broken"))
        let db = DbBinding(adapter: adapter, busyTimeoutMs: 5_000)
        do {
            _ = try await db.open(name: "broken", migrations: notesMigrations)
            XCTFail("a file of garbage opened")
        } catch {
            guard case .corrupt(let message) = error else {
                return XCTFail("\(error)")
            }
            XCTAssertTrue(message.contains("not a database"), message)
        }
    }

    func testAFileThatCannotBeOpenedIsUnavailable() async throws {
        // A file where the directory should be.
        let blocked = directory.appendingPathComponent("blocked", isDirectory: false)
        try Data([1]).write(to: blocked)
        let db = DbBinding(adapter: SQLiteDbAdapter(directory: blocked.appendingPathComponent("db")), busyTimeoutMs: 5_000)
        do {
            _ = try await db.open(name: "app", migrations: [])
            XCTFail("opened below a file")
        } catch {
            guard case .unavailable = error else {
                return XCTFail("\(error)")
            }
        }
        await expectThrows(DbError.unavailable(
            "invalid database name \"../escape\": use 1 to 64 of A-Z a-z 0-9 . _ - (not starting with .), or \":memory:\""
        )) { () async throws(DbError) -> DbOpened in
            try await db.open(name: "../escape", migrations: [])
        }
    }

    func testFilesPersistMemoryDatabasesArePrivateAndClosedIdsAreUnavailable() async throws {
        let adapter = SQLiteDbAdapter(directory: directory)
        let db = DbBinding(adapter: adapter, busyTimeoutMs: 5_000)
        let id = try await db.open(name: "keep", migrations: notesMigrations).db
        _ = try await db.execute(id, "INSERT INTO notes (title) VALUES (?)", [.text("milk")])
        try await db.close(id)
        XCTAssertTrue(FileManager.default.fileExists(atPath: adapter.fileURL(forDatabase: "keep").path))
        await expectThrows(DbError.unavailable("no open database or transaction \(id)")) { () async throws(DbError) -> DbRows in
            try await db.query(id, "SELECT 1", [])
        }
        let reopened = try await db.open(name: "keep", migrations: notesMigrations)
        XCTAssertEqual(reopened.version, 2)
        let rows = try await db.query(reopened.db, "SELECT title FROM notes", [])
        XCTAssertEqual(rows.rows, [[.text("milk")]])
        let journal = try await db.query(reopened.db, "PRAGMA journal_mode", [])
        XCTAssertEqual(journal.rows, [[.text("wal")]])
        let keys = try await db.query(reopened.db, "PRAGMA foreign_keys", [])
        XCTAssertEqual(keys.rows, [[.integer(1)]])

        let first = try await db.open(name: ":memory:", migrations: notesMigrations).db
        let second = try await db.open(name: ":memory:", migrations: []).db
        _ = try await db.execute(first, "INSERT INTO notes (title) VALUES ('only here')", [])
        await expectThrows(DbError.sql(message: "no such table: notes")) { () async throws(DbError) -> DbRows in
            try await db.query(second, "SELECT * FROM notes", [])
        }
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: directory.path).filter { $0.contains("memory") }, [])
    }

    func testAMigrationThatBreaksAConstraintRollsBackAndNamesItsVersion() async throws {
        let db = binding()
        let migrations = notesMigrations + [DbMigration(version: 3, sql: "INSERT INTO notes (title) VALUES ('a'); INSERT INTO notes (title) VALUES (NULL)")]
        do {
            _ = try await db.open(name: "m3", migrations: migrations)
            XCTFail("migration 3 should fail")
        } catch {
            guard case .migration(let version, let message) = error else {
                return XCTFail("\(error)")
            }
            XCTAssertEqual(version, 3)
            XCTAssertTrue(message.hasPrefix("constraint failed: NOT NULL"), message)
        }
        // Migrations 1 and 2 ran in the same transaction as 3 and were rolled back with it.
        let bare = try await db.open(name: "m3", migrations: [])
        XCTAssertEqual(bare.version, 0)
        let tables = try await db.query(bare.db, "SELECT name FROM sqlite_master", [])
        XCTAssertEqual(tables.rows, [])
    }
}
