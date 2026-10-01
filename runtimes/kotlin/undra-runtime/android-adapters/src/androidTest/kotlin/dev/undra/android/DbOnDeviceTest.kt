package dev.undra.android

import android.content.Context
import android.database.sqlite.SQLiteConstraintException
import android.database.sqlite.SQLiteDatabaseLockedException
import android.database.sqlite.SQLiteException
import androidx.test.core.app.ApplicationProvider
import dev.undra.runtime.adapters.DbConstraint
import dev.undra.runtime.adapters.DbError
import dev.undra.runtime.adapters.DbMigration
import dev.undra.runtime.adapters.DbOpened
import dev.undra.runtime.adapters.DbPortAdapter
import dev.undra.runtime.adapters.DbRows
import dev.undra.runtime.adapters.DbValue
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

private val NOTES = listOf(
    DbMigration(1u, "CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0)"),
    DbMigration(
        2u,
        "CREATE INDEX notes_done ON notes (done); CREATE TABLE tags (note INTEGER NOT NULL REFERENCES notes (id) ON DELETE CASCADE, " +
            "tag TEXT NOT NULL CHECK (length(tag) > 0), UNIQUE (note, tag))",
    ),
)

private suspend inline fun <reified E : Throwable> fails(crossinline block: suspend () -> Unit): E {
    try {
        block()
    } catch (e: Throwable) {
        if (e is E) return e
        fail("expected ${E::class.java.simpleName} but got $e")
    }
    fail("expected ${E::class.java.simpleName} but nothing was thrown")
    throw AssertionError()
}

/**
 * The `Db` port over Android's own SQLite ([AndroidDbAdapter] through [DbPortAdapter], ADR-048): the failure-injection list of
 * the brief (each constraint kind, busy, a corrupt file, unknown ids, names, migrations, typed cells, one statement, the
 * parameter count, `:memory:`) and the file the default constructor uses.
 */
class DbOnDeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private lateinit var dir: File
    private lateinit var port: DbPortAdapter

    @Before
    fun setUp() {
        dir = File(context.cacheDir, "undra-db-test-${System.nanoTime()}").also { it.mkdirs() }
        port = DbPortAdapter(AndroidDbAdapter(dir), busyTimeoutMillis = 300)
    }

    @After
    fun tearDown() {
        port.close()
        Thread.sleep(100)
        dir.deleteRecursively()
    }

    /** Runs a test body (JUnit 4 wants test methods that return nothing). */
    private fun run(block: suspend CoroutineScope.() -> Unit) {
        runBlocking { withTimeout(30_000) { block() } }
    }

    @Test
    fun open_migrates_once_and_a_reopen_keeps_rows_and_version() = run {
        val first = port.open("notes", NOTES)
        assertEquals(2u, first.version)
        assertTrue(File(dir, "undra-notes.sqlite").isFile)
        assertEquals(1uL, port.execute(first.db, "INSERT INTO notes (title) VALUES (?)", listOf(DbValue.Text("milk"))).changes)
        port.close(first.db)
        val again = port.open("notes", NOTES)
        assertEquals(DbOpened(again.db, 2u), again)
        assertEquals(DbRows(listOf("title"), listOf(listOf(DbValue.Text("milk")))), port.query(again.db, "SELECT title FROM notes", emptyList()))
        assertEquals(
            "the binding switched the file to write-ahead logging",
            listOf(listOf<DbValue>(DbValue.Text("wal"))),
            port.query(again.db, "PRAGMA journal_mode", emptyList()).rows,
        )
        assertEquals(listOf(listOf<DbValue>(DbValue.Integer(1))), port.query(again.db, "PRAGMA foreign_keys", emptyList()).rows)
    }

    @Test
    fun every_storage_class_round_trips_by_its_own_class() = run {
        val db = port.open(":memory:", emptyList()).db
        port.execute(db, "CREATE TABLE c (v)", emptyList())
        val values = listOf(
            DbValue.Integer(Long.MIN_VALUE), DbValue.Integer(Long.MAX_VALUE), DbValue.Integer(0), DbValue.Real(1.5), DbValue.Real(-0.25),
            DbValue.Text("é😀"), DbValue.Text(""), DbValue.Blob(ByteArray(0)), DbValue.Blob(byteArrayOf(0, -1, 7)), DbValue.Null,
        )
        for (v in values) port.execute(db, "INSERT INTO c VALUES (?)", listOf(v))
        val rows = port.query(db, "SELECT v, typeof(v) AS t FROM c ORDER BY rowid", emptyList())
        assertEquals(listOf("v", "t"), rows.columns)
        assertEquals(values, rows.rows.map { it[0] })
        assertEquals(
            listOf("integer", "integer", "integer", "real", "real", "text", "text", "blob", "blob", "null"),
            rows.rows.map { (it[1] as DbValue.Text).value },
        )
        val inserted = port.execute(db, "INSERT INTO c VALUES (?)", listOf(DbValue.Integer(9)))
        assertEquals(1uL, inserted.changes)
        assertEquals(11L, inserted.lastInsertId)
    }

    @Test
    fun each_constraint_kind_is_typed_from_androids_code_suffix() = run {
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
            val e = fails<DbError.Constraint> { port.execute(db, sql, emptyList()) }
            assertEquals(sql, kind, e.kind)
            assertTrue(e.reason, e.reason.contains("constraint failed", ignoreCase = true))
            assertTrue("the (code ...) suffix is not part of the message: ${e.reason}", !e.reason.contains("(code"))
        }
    }

    @Test
    fun sql_errors_one_statement_and_the_parameter_count_are_sql() = run {
        val db = port.open(":memory:", emptyList()).db
        fails<DbError.Sql> { port.execute(db, "INSERT INTO missing VALUES (1)", emptyList()) }
        fails<DbError.Sql> { port.query(db, "SELEC 1", emptyList()) }
        assertEquals(
            DbError.Sql("only one statement per call: use a migration for several"),
            fails<DbError.Sql> { port.query(db, "SELECT 1; SELECT 2", emptyList()) },
        )
        assertEquals(DbError.Sql("the statement has 2 parameters, 1 were given"), fails<DbError.Sql> { port.query(db, "SELECT ?, ?", listOf(DbValue.Null)) })
        assertEquals(DbRows(listOf("x"), listOf(listOf(DbValue.Integer(1)))), port.query(db, "SELECT 1 AS x; -- a comment", emptyList()))
        assertEquals(DbRows(listOf("a", "b"), listOf(listOf(DbValue.Integer(7), DbValue.Text("t")))), port.query(db, "SELECT ? AS a, ? AS b", listOf(DbValue.Integer(7), DbValue.Text("t"))))
    }

    @Test
    fun a_statement_outside_a_running_transaction_is_busy_after_the_busy_timeout() = run {
        val db = port.open("busy", NOTES).db
        val tx = port.begin(db)
        port.execute(tx, "INSERT INTO notes (title) VALUES ('in tx')", emptyList())
        assertEquals(DbError.Busy, fails<DbError.Busy> { port.execute(db, "INSERT INTO notes (title) VALUES ('outside')", emptyList()) })
        val waiting = async { port.query(db, "SELECT COUNT(*) AS n FROM notes", emptyList()) }
        delay(50)
        port.rollback(tx)
        assertEquals(listOf(listOf<DbValue>(DbValue.Integer(0))), waiting.await().rows)
        val kept = port.begin(db)
        port.execute(kept, "INSERT INTO notes (title) VALUES ('kept')", emptyList())
        port.commit(kept)
        assertEquals(listOf(listOf<DbValue>(DbValue.Text("kept"))), port.query(db, "SELECT title FROM notes", emptyList()).rows)
        assertEquals(DbError.Unavailable("transaction $kept is over"), fails<DbError.Unavailable> { port.commit(kept) })
    }

    @Test
    fun a_corrupt_file_is_corrupt_and_is_not_deleted() = run {
        val file = File(dir, "undra-bad.sqlite")
        file.writeBytes(ByteArray(4096) { 0x41 })
        fails<DbError.Corrupt> { port.open("bad", NOTES) }
        assertTrue("Android's default handler would have deleted it", file.isFile && file.length() == 4096L)
    }

    @Test
    fun a_failed_migration_rolls_everything_back_and_a_downgrade_is_refused() = run {
        val broken = listOf(DbMigration(1u, "CREATE TABLE a (x INTEGER)"), DbMigration(2u, "CREATE TABLE b (y INTEGER); INSERT INTO nowhere VALUES (1)"))
        assertEquals(2u, fails<DbError.Migration> { port.open("m", broken) }.version)
        val good = listOf(DbMigration(1u, "CREATE TABLE a (x INTEGER)"), DbMigration(2u, "CREATE TABLE b (y INTEGER); INSERT INTO a VALUES (1)"))
        assertEquals(2u, port.open("m", good).version)
        assertEquals("nothing pending: the same migrations open it again", 2u, port.open("m", good).version)
        val newer = fails<DbError.Migration> { port.open("m", good.take(1)) }
        assertEquals(2u, newer.version)
        assertEquals("the database is at version 2, newer than the newest migration (1)", newer.reason)
    }

    @Test
    fun names_ids_after_close_and_memory() = run {
        fails<DbError.Unavailable> { port.open("../escape", emptyList()) }
        fails<DbError.Unavailable> { port.open(".hidden", emptyList()) }
        val db = port.open(":memory:", emptyList()).db
        port.close(db)
        port.close(db)
        assertEquals(DbError.Unavailable("no open database or transaction $db"), fails<DbError.Unavailable> { port.query(db, "SELECT 1", emptyList()) })
        val a = port.open(":memory:", emptyList()).db
        val b = port.open(":memory:", emptyList()).db
        port.execute(a, "CREATE TABLE only_in_a (x)", emptyList())
        fails<DbError.Sql> { port.query(b, "SELECT * FROM only_in_a", emptyList()) }
    }

    @Test
    fun the_default_constructor_uses_the_apps_database_path() = run {
        val name = "device-test-${System.nanoTime()}"
        val defaults = DbPortAdapter(AndroidDbAdapter(context))
        try {
            val db = defaults.open(name, NOTES).db
            defaults.close(db)
            val file = context.getDatabasePath("undra-$name.sqlite")
            assertTrue("$file exists", file.isFile)
        } finally {
            defaults.close()
            Thread.sleep(100)
            context.deleteDatabase("undra-$name.sqlite")
        }
    }

    @Test
    fun android_messages_map_by_their_code_suffix_then_by_class() {
        assertEquals(DbError.Constraint(DbConstraint.UNIQUE, "UNIQUE constraint failed: t.id"), androidDbError(SQLiteConstraintException("UNIQUE constraint failed: t.id (code 2067 SQLITE_CONSTRAINT_UNIQUE[2067])")))
        assertEquals(DbError.Constraint(DbConstraint.UNIQUE, "UNIQUE constraint failed: t.id"), androidDbError(SQLiteConstraintException("UNIQUE constraint failed: t.id (code 1555)")))
        assertEquals(DbError.Constraint(DbConstraint.NOT_NULL, "NOT NULL constraint failed: t.x"), androidDbError(SQLiteConstraintException("NOT NULL constraint failed: t.x (code 1299 SQLITE_CONSTRAINT_NOTNULL)")))
        assertEquals(DbError.Busy, androidDbError(SQLiteDatabaseLockedException("database is locked (code 5 SQLITE_BUSY)")))
        assertEquals(DbError.Busy, androidDbError(SQLiteDatabaseLockedException("database is locked")))
        assertEquals(DbError.Sql("no such table: missing"), androidDbError(SQLiteException("no such table: missing (code 1 SQLITE_ERROR): , while compiling: INSERT INTO missing VALUES (1)")))
        assertEquals(DbError.Constraint(DbConstraint.OTHER, "something"), androidDbError(SQLiteConstraintException("something")))
        assertEquals(DbError.Corrupt("file is not a database"), androidDbError(SQLiteException("file is not a database (code 26 SQLITE_NOTADB)")))
        assertEquals(DbError.Full, androidDbError(SQLiteException("database or disk is full (code 13 SQLITE_FULL)")))
        assertEquals(DbError.Unavailable("unable to open database file"), androidDbError(SQLiteException("unable to open database file (code 14 SQLITE_CANTOPEN)")))
    }
}
