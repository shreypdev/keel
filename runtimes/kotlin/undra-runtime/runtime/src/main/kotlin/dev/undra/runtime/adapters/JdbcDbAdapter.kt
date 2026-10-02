package dev.undra.runtime.adapters

import java.nio.file.Files
import java.nio.file.Path
import java.sql.Connection
import java.sql.DriverManager
import java.sql.PreparedStatement
import java.sql.SQLException
import java.sql.Types
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.withContext

/**
 * The default [DbAdapter] on a JVM (ADR-048): `java.sql` with the SQLite JDBC driver the app puts on its class path
 * (`org.xerial:sqlite-jdbc`, which bundles SQLite; `:runtime` itself depends on nothing). The driver is found through
 * `DriverManager` with the URL `jdbc:sqlite:<path>`.
 *
 * * Files are `<directory>/<name>.sqlite` (the directory is created); `":memory:"` is a private in-memory database.
 * * Each database has one connection and one thread of its own; every call runs there.
 * * `execute` and `query` take exactly one statement ([SqlText.statements]), bind the values positionally (`Null`,
 *   `Integer` as `long`, `Real` as `double`, `Text`, `Blob`) after checking their number against the statement's
 *   parameters, and read each cell by its storage class. `execute` answers SQLite's `changes()` and `last_insert_rowid()`.
 * * Errors follow the driver's SQLite result code ([fromSqliteCode]): the extended code of `org.sqlite.SQLiteException`
 *   where the driver reports it, else the primary code it puts in `SQLException.getErrorCode()`.
 *
 * Without a driver every `open` is `Unavailable("no SQLite JDBC driver on the class path: add org.xerial:sqlite-jdbc")`.
 *
 * @param directory where the database files live: [JvmAdapters] uses `<dataDir>/db`.
 */
public class JdbcDbAdapter internal constructor(private val directory: Path, private val hasDriver: () -> Boolean) : DbAdapter {
    /** Databases in [directory], through the SQLite JDBC driver on the class path. */
    public constructor(directory: Path) : this(directory, { isDriverAvailable() })

    override suspend fun open(name: String): DbConnection {
        if (!hasDriver()) throw DbError.Unavailable(NO_DRIVER)
        val url = if (name == DbPortAdapter.MEMORY) {
            "$URL_PREFIX:memory:"
        } else {
            try {
                Files.createDirectories(directory)
            } catch (e: Exception) {
                throw DbError.Unavailable("cannot create $directory: ${describe(e)}")
            }
            URL_PREFIX + directory.resolve("$name.sqlite").toAbsolutePath()
        }
        val executor = Executors.newSingleThreadExecutor { task ->
            Thread(task, "undra-db-${threadIds.getAndIncrement()}-$name").also { it.isDaemon = true }
        }
        val dispatcher = executor.asCoroutineDispatcher()
        val connection = try {
            withContext(dispatcher) { DriverManager.getConnection(url) }
        } catch (e: SQLException) {
            executor.shutdown()
            throw mapSqlException(e)
        } catch (e: Exception) {
            executor.shutdown()
            throw DbError.Unavailable(describe(e))
        }
        return JdbcDbConnection(connection, executor, dispatcher)
    }

    /** Facts about the JDBC side. */
    public companion object {
        /** The JDBC URL prefix of SQLite. */
        public const val URL_PREFIX: String = "jdbc:sqlite:"

        /** What every call answers without a driver. */
        public const val NO_DRIVER: String = "no SQLite JDBC driver on the class path: add org.xerial:sqlite-jdbc"

        private val threadIds = AtomicInteger(1)

        /** Whether a JDBC driver for `jdbc:sqlite:` URLs is registered (on the class path). */
        public fun isDriverAvailable(): Boolean =
            try {
                DriverManager.getDriver(URL_PREFIX) != null
            } catch (e: SQLException) {
                false
            }

        /**
         * The [DbError] of a driver's [SQLException]: by the extended result code of `org.sqlite.SQLiteException` (read
         * without a compile-time dependency on the driver), else by the primary code in [SQLException.getErrorCode]; the
         * message is SQLite's own (the driver wraps it as `[SQLITE_X] description (message)`).
         */
        public fun mapSqlException(e: SQLException): DbError {
            val code = extendedCode(e) ?: e.errorCode
            return DbError.fromSqliteCode(code, sqliteMessage(e.message.orEmpty()))
        }

        private fun extendedCode(e: SQLException): Int? =
            try {
                val resultCode = e.javaClass.getMethod("getResultCode").invoke(e) ?: return null
                resultCode.javaClass.getField("code").getInt(resultCode)
            } catch (e: ReflectiveOperationException) {
                null
            } catch (e: RuntimeException) {
                null
            }

        private val DRIVER_MESSAGE = Regex("""^\[SQLITE_[A-Z_]+]\s.*?\((.*)\)\s*$""", RegexOption.DOT_MATCHES_ALL)

        private fun sqliteMessage(message: String): String = DRIVER_MESSAGE.find(message)?.groupValues?.get(1) ?: message
    }
}

/** One database of [JdbcDbAdapter]: a connection used only on [dispatcher]'s single thread. */
private class JdbcDbConnection(
    private val connection: Connection,
    private val executor: ExecutorService,
    private val dispatcher: CoroutineDispatcher,
) : DbConnection {
    override suspend fun execute(sql: String, params: List<DbValue>): DbExecuted = onThread {
        prepare(sql, params).use { it.execute() }
        connection.createStatement().use { statement ->
            statement.executeQuery("SELECT changes(), last_insert_rowid()").use { rows ->
                rows.next()
                DbExecuted(rows.getLong(1).coerceAtLeast(0).toULong(), rows.getLong(2))
            }
        }
    }

    override suspend fun query(sql: String, params: List<DbValue>): DbRows = onThread {
        prepare(sql, params).use { statement ->
            // `executeQuery` refuses a statement without a result (`PRAGMA foreign_keys = ON`): that is no rows here.
            if (!statement.execute()) {
                DbRows(emptyList(), emptyList())
            } else {
                statement.resultSet.use { rows ->
                    val meta = rows.metaData
                    val columns = List(meta.columnCount) { meta.getColumnLabel(it + 1) ?: meta.getColumnName(it + 1) }
                    val out = ArrayList<List<DbValue>>()
                    while (rows.next()) out.add(List(columns.size) { cell(rows.getObject(it + 1)) })
                    DbRows(columns, out)
                }
            }
        }
    }

    override suspend fun executeScript(sql: String): Unit = onThread {
        for (statement in SqlText.statements(sql)) {
            connection.createStatement().use { it.execute(statement) }
        }
    }

    override suspend fun close() {
        try {
            withContext(dispatcher) { connection.close() }
        } catch (e: SQLException) {
            // nothing more to release
        } finally {
            executor.shutdown()
        }
    }

    private fun prepare(sql: String, params: List<DbValue>): PreparedStatement {
        val statements = SqlText.statements(sql)
        if (statements.size != 1) {
            throw DbError.Sql(if (statements.isEmpty()) "the SQL holds no statement" else "only one statement per call: use a migration for several")
        }
        val prepared = connection.prepareStatement(statements[0])
        try {
            val expected = try {
                prepared.parameterMetaData.parameterCount
            } catch (e: SQLException) {
                SqlText.parameterCount(statements[0])
            }
            if (expected != params.size) throw DbError.Sql("the statement has $expected parameters, ${params.size} were given")
            params.forEachIndexed { i, value ->
                val at = i + 1
                when (value) {
                    DbValue.Null -> prepared.setNull(at, Types.NULL)
                    is DbValue.Integer -> prepared.setLong(at, value.value)
                    is DbValue.Real -> prepared.setDouble(at, value.value)
                    is DbValue.Text -> prepared.setString(at, value.value)
                    is DbValue.Blob -> prepared.setBytes(at, value.value)
                }
            }
            return prepared
        } catch (e: Throwable) {
            prepared.close()
            throw e
        }
    }

    private fun cell(value: Any?): DbValue = when (value) {
        null -> DbValue.Null
        is Long -> DbValue.Integer(value)
        is Int -> DbValue.Integer(value.toLong())
        is Short -> DbValue.Integer(value.toLong())
        is Byte -> DbValue.Integer(value.toLong())
        is Double -> DbValue.Real(value)
        is Float -> DbValue.Real(value.toDouble())
        is ByteArray -> DbValue.Blob(value)
        is String -> DbValue.Text(value)
        else -> DbValue.Text(value.toString())
    }

    private suspend fun <T> onThread(block: () -> T): T =
        try {
            withContext(dispatcher) { block() }
        } catch (e: SQLException) {
            throw JdbcDbAdapter.mapSqlException(e)
        } catch (e: java.util.concurrent.RejectedExecutionException) {
            throw DbError.Unavailable("the database is closed")
        }
}
