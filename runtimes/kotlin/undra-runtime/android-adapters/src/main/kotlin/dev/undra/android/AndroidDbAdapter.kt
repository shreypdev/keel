package dev.undra.android

import android.content.Context
import android.database.Cursor
import android.database.DatabaseUtils
import android.database.sqlite.SQLiteAccessPermException
import android.database.sqlite.SQLiteCantOpenDatabaseException
import android.database.sqlite.SQLiteConstraintException
import android.database.sqlite.SQLiteCursor
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteDatabaseCorruptException
import android.database.sqlite.SQLiteDatabaseLockedException
import android.database.sqlite.SQLiteDiskIOException
import android.database.sqlite.SQLiteException
import android.database.sqlite.SQLiteFullException
import android.database.sqlite.SQLiteProgram
import android.database.sqlite.SQLiteReadOnlyDatabaseException
import dev.undra.runtime.adapters.DbAdapter
import dev.undra.runtime.adapters.DbConnection
import dev.undra.runtime.adapters.DbConstraint
import dev.undra.runtime.adapters.DbError
import dev.undra.runtime.adapters.DbExecuted
import dev.undra.runtime.adapters.DbPortAdapter
import dev.undra.runtime.adapters.DbRows
import dev.undra.runtime.adapters.DbValue
import dev.undra.runtime.adapters.SqlText
import dev.undra.runtime.adapters.fromSqliteCode
import java.io.File
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.withContext

/**
 * The `Db` port's SQLite on Android (ADR-048): `android.database.sqlite`, the platform's own SQLite, no dependency.
 * Serve it with [DbPortAdapter] (`AndroidPlatformDefaults.install` does).
 *
 * * Files are `context.getDatabasePath("undra-<namespace>-<name>.sqlite")`, the namespace being the core's (a namespace has
 *   no `-`, so the name is what follows the second one; two cores of one app never share a file: ADR-044 amendment A). The
 *   React Native module uses the same file, so either shell reads the other's database; `":memory:"` is a private in-memory
 *   database. The directory constructor roots `undra-<name>.sqlite` elsewhere, for every core (tests, and apps that choose).
 * * Each database has one connection and one thread of its own; every call runs there, so Android's thread-bound
 *   transactions are the binding's. Android's own write-ahead-logging pool is turned off (one connection: the pragmas the
 *   binding sets and `last_insert_rowid()` are that connection's); the binding switches the file to WAL itself.
 * * `execute` and `query` take exactly one statement ([SqlText]) and bind typed values (`bindNull`, `bindLong`,
 *   `bindDouble`, `bindString`, `bindBlob`) after checking their number against the statement's parameters; cells are read
 *   by `Cursor.getType`. `BEGIN`, `COMMIT` and `ROLLBACK` go through `beginTransactionNonExclusive` (`BEGIN IMMEDIATE`),
 *   `setTransactionSuccessful` + `endTransaction`, and `endTransaction`.
 * * Errors: Android reports no result code, so the `(code NNNN ...)` suffix of its message is parsed (the one documented
 *   exception to "codes, never text") and mapped by [fromSqliteCode]; without one, the exception's class decides. A file
 *   that is not a database is `Corrupt` and is left alone (Android's default handler would delete it).
 * * A cursor window holds about 2 MB: a row larger than that (a big blob) fails with `Sql`.
 */
public class AndroidDbAdapter private constructor(private val fileOf: (String) -> File) : DbAdapter {
    /**
     * Databases under the app's database directory: `context.getDatabasePath("undra-<namespace>-<name>.sqlite")`,
     * [namespace] being the core's (`UndraCore.namespace`).
     */
    public constructor(context: Context, namespace: String) :
        this({ name -> context.applicationContext.getDatabasePath(fileNameOf(namespace, name)) })

    /** Databases in [directory] (`<directory>/undra-<name>.sqlite`), for tests. */
    public constructor(directory: File) : this({ name -> File(directory, "undra-$name.sqlite") })

    override suspend fun open(name: String): DbConnection {
        val executor = Executors.newSingleThreadExecutor { task ->
            Thread(task, "undra-db-${threadIds.getAndIncrement()}-$name").also { it.isDaemon = true }
        }
        val dispatcher = executor.asCoroutineDispatcher()
        try {
            val database = withContext(dispatcher) {
                mapped {
                    val path = if (name == DbPortAdapter.MEMORY) {
                        DbPortAdapter.MEMORY
                    } else {
                        val file = fileOf(name)
                        file.parentFile?.let { parent ->
                            if (!parent.isDirectory && !parent.mkdirs()) throw DbError.Unavailable("cannot create $parent")
                        }
                        file.path
                    }
                    SQLiteDatabase.openDatabase(
                        path,
                        null,
                        SQLiteDatabase.CREATE_IF_NECESSARY or SQLiteDatabase.NO_LOCALIZED_COLLATORS,
                        // Keep a damaged file: the default handler deletes it. The open then fails, as `Corrupt`.
                        { _ -> },
                    ).also { db ->
                        // One connection (no WAL pool, no "compatibility WAL"): the binding's pragmas apply to it.
                        db.disableWriteAheadLogging()
                    }
                }
            }
            return AndroidDbConnection(database, executor, dispatcher)
        } catch (e: Throwable) {
            executor.shutdown()
            throw e
        }
    }

    /** Where the default adapter keeps a core's databases. */
    public companion object {
        /** The file name of database [name] of the core [namespace] in the app's database directory: `undra-<namespace>-<name>.sqlite`. */
        public fun fileNameOf(namespace: String, name: String): String = "undra-$namespace-$name.sqlite"

        private val threadIds = AtomicInteger(1)
    }
}

/** One database of [AndroidDbAdapter], used only on its own thread. */
private class AndroidDbConnection(
    private val database: SQLiteDatabase,
    private val executor: ExecutorService,
    private val dispatcher: CoroutineDispatcher,
) : DbConnection {
    override suspend fun execute(sql: String, params: List<DbValue>): DbExecuted = onThread {
        val statement = single(sql, params)
        if (transactionControl(statement)) {
            DbExecuted(changes(), lastInsertId())
        } else {
            database.compileStatement(statement).use { compiled ->
                bind(compiled, params)
                compiled.executeUpdateDelete()
            }
            DbExecuted(changes(), lastInsertId())
        }
    }

    override suspend fun query(sql: String, params: List<DbValue>): DbRows = onThread {
        val statement = single(sql, params)
        val factory = SQLiteDatabase.CursorFactory { _, driver, editTable, query ->
            bind(query, params)
            SQLiteCursor(driver, editTable, query)
        }
        database.rawQueryWithFactory(factory, statement, null, "").use { cursor ->
            val columns = cursor.columnNames.toList()
            val rows = ArrayList<List<DbValue>>(cursor.count)
            while (cursor.moveToNext()) rows.add(List(columns.size) { cell(cursor, it) })
            DbRows(columns, rows)
        }
    }

    override suspend fun executeScript(sql: String): Unit = onThread {
        for (statement in SqlText.statements(sql)) {
            if (!transactionControl(statement)) database.execSQL(statement)
        }
    }

    override suspend fun close() {
        try {
            withContext(dispatcher) {
                // A transaction left open (a core that went away mid-way) is rolled back by closing.
                while (database.inTransaction()) database.endTransaction()
                database.close()
            }
        } catch (e: Exception) {
            // nothing more to release
        } finally {
            executor.shutdown()
        }
    }

    /** The one statement of [sql], with its parameter count checked against [params]. */
    private fun single(sql: String, params: List<DbValue>): String {
        val statements = SqlText.statements(sql)
        if (statements.size != 1) {
            throw DbError.Sql(if (statements.isEmpty()) "the SQL holds no statement" else "only one statement per call: use a migration for several")
        }
        val expected = SqlText.parameterCount(statements[0])
        if (expected != params.size) throw DbError.Sql("the statement has $expected parameters, ${params.size} were given")
        return statements[0]
    }

    /** Runs `BEGIN`, `COMMIT`/`END` and `ROLLBACK` through Android's transaction API; `false` for any other statement. */
    private fun transactionControl(statement: String): Boolean {
        when (DatabaseUtils.getSqlStatementType(statement)) {
            DatabaseUtils.STATEMENT_BEGIN, DatabaseUtils.STATEMENT_COMMIT, DatabaseUtils.STATEMENT_ABORT -> {
                // Run by SQLite itself, not through Android's per-thread transaction stack: when SQLite refuses a
                // COMMIT (a deferred foreign key), Android has already popped its transaction while SQLite keeps it
                // open, so the binding's ROLLBACK would be refused ("no current transaction") and every later `begin`
                // would fail. A leading `;` keeps the statement out of that path (DatabaseUtils reads it as OTHER);
                // the database has one connection (no WAL pool), so the transaction spans the statements that follow.
                database.execSQL(";$statement")
            }
            else -> return false
        }
        return true
    }

    private fun changes(): ULong = DatabaseUtils.longForQuery(database, "SELECT changes()", null).coerceAtLeast(0).toULong()

    private fun lastInsertId(): Long = DatabaseUtils.longForQuery(database, "SELECT last_insert_rowid()", null)

    private fun bind(program: SQLiteProgram, params: List<DbValue>) {
        params.forEachIndexed { i, value ->
            val at = i + 1
            when (value) {
                DbValue.Null -> program.bindNull(at)
                is DbValue.Integer -> program.bindLong(at, value.value)
                is DbValue.Real -> program.bindDouble(at, value.value)
                is DbValue.Text -> program.bindString(at, value.value)
                is DbValue.Blob -> program.bindBlob(at, value.value)
            }
        }
    }

    private fun cell(cursor: Cursor, column: Int): DbValue = when (cursor.getType(column)) {
        Cursor.FIELD_TYPE_NULL -> DbValue.Null
        Cursor.FIELD_TYPE_INTEGER -> DbValue.Integer(cursor.getLong(column))
        Cursor.FIELD_TYPE_FLOAT -> DbValue.Real(cursor.getDouble(column))
        Cursor.FIELD_TYPE_BLOB -> DbValue.Blob(cursor.getBlob(column))
        else -> DbValue.Text(cursor.getString(column))
    }

    private suspend fun <T> onThread(block: () -> T): T =
        try {
            withContext(dispatcher) { mapped(block) }
        } catch (e: RejectedExecutionException) {
            throw DbError.Unavailable("the database is closed")
        }
}

/** Runs [block]; Android's SQLite failures become [DbError]s. */
private inline fun <T> mapped(block: () -> T): T =
    try {
        block()
    } catch (e: SQLiteException) {
        throw androidDbError(e)
    } catch (e: IllegalStateException) {
        throw DbError.Sql(e.message ?: "the statement is not valid here")
    } catch (e: IllegalArgumentException) {
        throw DbError.Sql(e.message ?: "invalid argument")
    }

/** `... (code 2067 SQLITE_CONSTRAINT_UNIQUE[2067]) ...`: the extended code, and the message before it. */
private val CODE_SUFFIX = Regex("""^(.*?)\s*\(code (\d+)(?:\s[A-Z_]+(?:\[\d+])?)?\)(.*)$""", RegexOption.DOT_MATCHES_ALL)

/**
 * The [DbError] of an Android SQLite exception: by the `(code NNNN ...)` suffix of its message when it has one (ADR-048's
 * documented exception), else by its class.
 */
internal fun androidDbError(e: SQLiteException): DbError {
    val text = e.message.orEmpty()
    val match = CODE_SUFFIX.find(text)
    if (match != null) {
        val code = match.groupValues[2].toIntOrNull()
        if (code != null) return DbError.fromSqliteCode(code, match.groupValues[1].trim())
    }
    return when (e) {
        is SQLiteDatabaseLockedException -> DbError.Busy
        is SQLiteConstraintException -> DbError.Constraint(DbConstraint.OTHER, text)
        is SQLiteDatabaseCorruptException -> DbError.Corrupt(text)
        is SQLiteFullException -> DbError.Full
        is SQLiteCantOpenDatabaseException, is SQLiteReadOnlyDatabaseException, is SQLiteAccessPermException, is SQLiteDiskIOException ->
            DbError.Unavailable(text)
        else -> DbError.Sql(text)
    }
}
