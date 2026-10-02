package dev.undra.runtime

import dev.undra.runtime.adapters.DbConstraint
import dev.undra.runtime.adapters.DbError
import dev.undra.runtime.adapters.DbMigration
import dev.undra.runtime.adapters.DbOpened
import dev.undra.runtime.adapters.DbPortAdapter
import dev.undra.runtime.adapters.DbRows
import dev.undra.runtime.adapters.DbValue
import dev.undra.runtime.adapters.JdbcDbAdapter
import dev.undra.runtime.support.TempDir
import dev.undra.runtime.support.eventually
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.fail
import java.nio.file.Files
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.jupiter.api.Test

private suspend inline fun <reified E : Throwable> dbFails(crossinline block: suspend () -> Unit): E {
    try {
        block()
    } catch (e: Throwable) {
        if (e is E) return e
        fail("expected ${E::class.java.simpleName} but got $e")
    }
    fail("expected ${E::class.java.simpleName} but nothing was thrown")
}

private val NOTES = listOf(
    DbMigration(1u, "CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0)"),
    DbMigration(
        2u,
        "CREATE INDEX notes_done ON notes (done); CREATE TABLE tags (note INTEGER NOT NULL REFERENCES notes (id) ON DELETE CASCADE, " +
            "tag TEXT NOT NULL CHECK (length(tag) > 0), UNIQUE (note, tag))",
    ),
)

/**
 * The `Db` port over the real SQLite of [JdbcDbAdapter] (ADR-048's failure-injection list), rooted in a temporary directory.
 * It needs the SQLite JDBC driver (`org.xerial:sqlite-jdbc`) on the class path, which `:runtime` does not depend on: without
 * it every case but the first is skipped with that reason. (The same semantics run on Android over `AndroidDbAdapter`, in
 * `android-adapters`' instrumented tests.)
 */
class JdbcDbAdapterTests : Suite() {
    private fun <T> withDb(busyTimeoutMillis: Long = 5_000, block: suspend CoroutineScope.(DbPortAdapter, TempDir) -> T): T {
        if (!JdbcDbAdapter.isDriverAvailable()) {
            val why = "no SQLite JDBC driver on the class path (org.xerial:sqlite-jdbc; set UNDRA_SQLITE_JDBC)"
            if (System.getenv("UNDRA_REQUIRE_TOOLCHAINS") == "1") fail("UNDRA_REQUIRE_TOOLCHAINS=1: $why")
            skip(why)
        }
        TempDir().use { dir ->
            val port = DbPortAdapter(JdbcDbAdapter(dir.path), busyTimeoutMillis = busyTimeoutMillis)
            try {
                return runBlocking { withTimeout(30_000) { block(port, dir) } }
            } finally {
                // Close every database (asynchronously) and wait, so that SQLite is done with the files before they go.
                port.close()
                eventually("the databases are closed") { port.openDatabases == 0 }
            }
        }
    }

    init {
        case("without a driver every open is Unavailable, naming the driver to add") {
            TempDir().use { dir ->
                val port = DbPortAdapter(JdbcDbAdapter(dir.path) { false })
                runBlocking {
                    for (name in listOf("app", ":memory:")) {
                        val e = dbFails<DbError.Unavailable> { port.open(name, NOTES) }
                        assertEq("no SQLite JDBC driver on the class path: add org.xerial:sqlite-jdbc", e.reason)
                    }
                }
                assertEq(0L, Files.list(dir.path).use { it.count() }, "nothing was created")
            }
        }

        case("open migrates once, reopen keeps the rows and the version; files are <dir>/db names") {
            withDb { port, dir ->
                val first = port.open("notes", NOTES)
                assertEq(2u, first.version)
                port.execute(first.db, "INSERT INTO notes (title) VALUES (?)", listOf(DbValue.Text("milk")))
                port.close(first.db)
                assertTrue(Files.isRegularFile(dir.path.resolve("notes.sqlite")))
                val again = port.open("notes", NOTES)
                assertEq(DbOpened(again.db, 2u), again)
                assertEq(DbRows(listOf("title"), listOf(listOf(DbValue.Text("milk")))), port.query(again.db, "SELECT title FROM notes", emptyList()))
            }
        }

        case("every storage class round-trips by its own class: i64 limits, reals, text, empty and binary blobs, null") {
            withDb { port, _ ->
                val db = port.open(":memory:", emptyList()).db
                port.execute(db, "CREATE TABLE c (v)", emptyList())
                val values = listOf(
                    DbValue.Integer(Long.MIN_VALUE), DbValue.Integer(Long.MAX_VALUE), DbValue.Integer(0), DbValue.Real(1.5), DbValue.Real(-0.25),
                    DbValue.Text("é😀"), DbValue.Text(""), DbValue.Blob(ByteArray(0)), DbValue.Blob(byteArrayOf(0, -1, 7)), DbValue.Null,
                )
                for (v in values) port.execute(db, "INSERT INTO c VALUES (?)", listOf(v))
                val rows = port.query(db, "SELECT v, typeof(v) AS t FROM c ORDER BY rowid", emptyList())
                assertEq(listOf("v", "t"), rows.columns)
                assertEq(values, rows.rows.map { it[0] })
                assertEq(listOf("integer", "integer", "integer", "real", "real", "text", "text", "blob", "blob", "null"), rows.rows.map { (it[1] as DbValue.Text).value })
            }
        }

        case("each constraint kind is typed") {
            withDb { port, _ ->
                val db = port.open("c", NOTES).db
                port.execute(db, "INSERT INTO notes (id, title) VALUES (1, 'a')", emptyList())
                port.execute(db, "INSERT INTO tags VALUES (1, 'x')", emptyList())
                val cases = listOf(
                    "INSERT INTO notes (id, title) VALUES (1, 'dup')" to DbConstraint.UNIQUE,
                    "INSERT INTO tags VALUES (1, 'x')" to DbConstraint.UNIQUE,
                    "INSERT INTO notes (title) VALUES (NULL)" to DbConstraint.NOT_NULL,
                    "INSERT INTO tags VALUES (99, 'y')" to DbConstraint.FOREIGN_KEY,
                    "INSERT INTO tags VALUES (1, '')" to DbConstraint.CHECK,
                )
                for ((sql, kind) in cases) {
                    val e = dbFails<DbError.Constraint> { port.execute(db, sql, emptyList()) }
                    assertEq(kind, e.kind, sql)
                    assertTrue(e.reason.contains("constraint failed", ignoreCase = true), e.reason)
                }
            }
        }

        case("SQL errors, one statement per call and the parameter count are Sql") {
            withDb { port, _ ->
                val db = port.open(":memory:", emptyList()).db
                dbFails<DbError.Sql> { port.execute(db, "INSERT INTO missing VALUES (1)", emptyList()) }
                dbFails<DbError.Sql> { port.query(db, "SELEC 1", emptyList()) }
                assertEq(DbError.Sql("only one statement per call: use a migration for several"), dbFails<DbError.Sql> { port.query(db, "SELECT 1; SELECT 2", emptyList()) })
                assertEq(DbError.Sql("the statement has 2 parameters, 1 were given"), dbFails<DbError.Sql> { port.query(db, "SELECT ?, ?", listOf(DbValue.Null)) })
                assertEq(DbRows(listOf("x"), listOf(listOf(DbValue.Integer(1)))), port.query(db, "SELECT 1 AS x; -- a comment", emptyList()))
            }
        }

        case("Busy: a statement on the database during its transaction waits the busy timeout") {
            withDb(busyTimeoutMillis = 300) { port, _ ->
                val db = port.open("busy", NOTES).db
                val tx = port.begin(db)
                port.execute(tx, "INSERT INTO notes (title) VALUES ('in tx')", emptyList())
                assertEq(DbError.Busy, dbFails<DbError.Busy> { port.execute(db, "INSERT INTO notes (title) VALUES ('outside')", emptyList()) })
                port.rollback(tx)
                assertEq(DbRows(listOf("n"), listOf(listOf(DbValue.Integer(0)))), port.query(db, "SELECT COUNT(*) AS n FROM notes", emptyList()))
            }
        }

        case("a corrupt file is Corrupt") {
            withDb { port, dir ->
                Files.write(dir.path.resolve("bad.sqlite"), ByteArray(4096) { 0x41 })
                dbFails<DbError.Corrupt> { port.open("bad", NOTES) }
            }
        }

        case("a failed migration rolls everything back; a downgrade is refused; names are checked") {
            withDb { port, _ ->
                val broken = listOf(DbMigration(1u, "CREATE TABLE a (x INTEGER)"), DbMigration(2u, "CREATE TABLE b (y INTEGER); INSERT INTO nowhere VALUES (1)"))
                assertEq(2u, dbFails<DbError.Migration> { port.open("m", broken) }.version)
                val good = listOf(DbMigration(1u, "CREATE TABLE a (x INTEGER)"), DbMigration(2u, "CREATE TABLE b (y INTEGER); INSERT INTO a VALUES (1)"))
                assertEq(2u, port.open("m", good).version)
                assertEq(2u, dbFails<DbError.Migration> { port.open("m", good.take(1)) }.version)
                dbFails<DbError.Unavailable> { port.open("../escape", emptyList()) }
            }
        }

        case("values travel only as parameters: an injection attempt is stored as text; ? and ; in literals, comments and quoted names are neither") {
            withDb { port, _ ->
                val db = port.open(":memory:", emptyList()).db
                port.execute(db, "CREATE TABLE t (v TEXT)", emptyList())
                val attack = "'; DROP TABLE t; --"
                port.execute(db, "INSERT INTO t (v) VALUES (?)", listOf(DbValue.Text(attack)))
                assertEq(listOf(listOf<DbValue>(DbValue.Text(attack))), port.query(db, "SELECT v FROM t WHERE v = ?", listOf(DbValue.Text(attack))).rows)
                val tricky = "SELECT '?;' AS \"a;?\", [b;?] FROM (SELECT ? AS [b;?]) /* ; ? */ WHERE ?2 = ?2 -- ; ?"
                assertEq(listOf(listOf<DbValue>(DbValue.Text("?;"), DbValue.Integer(5))), port.query(db, tricky, listOf(DbValue.Integer(5), DbValue.Integer(6))).rows)
                assertEq(DbError.Sql("the statement has 2 parameters, 1 were given"), dbFails<DbError.Sql> { port.query(db, "SELECT #a, ?", listOf(DbValue.Integer(1))) })
                assertEq(listOf(listOf<DbValue>(DbValue.Integer(1))), port.query(db, "SELECT COUNT(*) FROM t", emptyList()).rows, "the table is still there")
            }
        }

        case("text with U+0000, blobs with NUL bytes, an empty blob apart from NULL and a row over 2 MB round-trip") {
            withDb { port, _ ->
                val db = port.open(":memory:", emptyList()).db
                port.execute(db, "CREATE TABLE r (v)", emptyList())
                val big = ByteArray(2 * 1024 * 1024 + 17) { (it % 251).toByte() }
                val values = listOf(DbValue.Text("a\u0000b"), DbValue.Blob(byteArrayOf(0, 0, 1, 0)), DbValue.Blob(ByteArray(0)), DbValue.Null, DbValue.Blob(big))
                for (v in values) port.execute(db, "INSERT INTO r (v) VALUES (?)", listOf(v))
                assertEq(values, port.query(db, "SELECT v FROM r ORDER BY rowid", emptyList()).rows.map { it.single() })
            }
        }

        case("a transaction left open when the core detaches is rolled back: a fresh adapter on the file begins at once and sees none of it") {
            withDb { port, dir ->
                val db = port.open("left", NOTES).db
                port.execute(db, "INSERT INTO notes (title) VALUES ('committed')", emptyList())
                val tx = port.begin(db)
                port.execute(tx, "INSERT INTO notes (title) VALUES ('never committed')", emptyList())
                port.portImpl().detach!!.invoke()
                eventually("the detached database is closed") { port.openDatabases == 0 }
                // A short busy timeout: a connection still holding the write lock would make BEGIN IMMEDIATE Busy.
                val fresh = DbPortAdapter(JdbcDbAdapter(dir.path), busyTimeoutMillis = 100)
                try {
                    val again = fresh.open("left", NOTES).db
                    val tx2 = fresh.begin(again)
                    assertEq(listOf(listOf<DbValue>(DbValue.Text("committed"))), fresh.query(tx2, "SELECT title FROM notes", emptyList()).rows)
                    fresh.rollback(tx2)
                } finally {
                    fresh.close()
                    eventually("the fresh databases are closed") { fresh.openDatabases == 0 }
                }
            }
        }

        case("ids after close are Unavailable; transactions commit and roll back") {
            withDb { port, _ ->
                val db = port.open("t", NOTES).db
                val tx = port.begin(db)
                port.execute(tx, "INSERT INTO notes (title) VALUES ('kept')", emptyList())
                port.commit(tx)
                val tx2 = port.begin(db)
                port.execute(tx2, "INSERT INTO notes (title) VALUES ('dropped')", emptyList())
                port.rollback(tx2)
                assertEq(listOf(listOf<DbValue>(DbValue.Text("kept"))), port.query(db, "SELECT title FROM notes", emptyList()).rows)
                port.close(db)
                dbFails<DbError.Unavailable> { port.query(db, "SELECT 1", emptyList()) }
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
