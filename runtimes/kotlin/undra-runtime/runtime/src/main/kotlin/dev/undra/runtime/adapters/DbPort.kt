package dev.undra.runtime.adapters

import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraLog
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Opens SQLite databases for the `Db` port (ADR-048). The adapter only runs SQL; [DbPortAdapter] owns the ids, the
 * migrations, the transactions and the order of operations. Implementations: [JdbcDbAdapter] on a JVM, `AndroidDbAdapter`
 * (module `android-adapters`) on Android.
 */
public interface DbAdapter {
    /**
     * Opens (creating it if needed) the database [name]: `":memory:"` for a private in-memory database, else a file the
     * adapter places (the binding has checked the name: 1 to 64 of `A-Z a-z 0-9 . _ -`, not starting with `.`).
     *
     * @throws DbError `Unavailable` when the file cannot be opened, `Corrupt` when it is not a database.
     */
    public suspend fun open(name: String): DbConnection
}

/**
 * One open database of a [DbAdapter]. Every method runs on the adapter's own thread for that database, one at a time (the
 * binding never overlaps two calls on one connection).
 *
 * Errors follow SQLite's (extended) result code, never the message: `BUSY`/`LOCKED` are `Busy`; `CONSTRAINT` is `Constraint`
 * with the kind of its extended code; `CORRUPT`/`NOTADB` `Corrupt`; `FULL` `Full`; `CANTOPEN`/`PERM`/`READONLY`/`IOERR`
 * `Unavailable`; anything else `Sql` ([fromSqliteCode] maps a code).
 */
public interface DbConnection {
    /**
     * Runs exactly one statement with [params] bound positionally and returns the connection's change count and last
     * inserted rowid after it.
     *
     * @throws DbError `Sql("only one statement per call: use a migration for several")` for trailing SQL,
     *   `Sql("the statement has N parameters, M were given")` for a wrong parameter count, else what SQLite said.
     */
    public suspend fun execute(sql: String, params: List<DbValue>): DbExecuted

    /**
     * Runs exactly one query with [params] bound positionally and returns the column names and every row, each cell by its
     * storage class.
     *
     * @throws DbError as [execute] does.
     */
    public suspend fun query(sql: String, params: List<DbValue>): DbRows

    /**
     * Runs [sql], which may hold several statements (a migration), without parameters.
     *
     * @throws DbError what the failing statement's SQLite said.
     */
    public suspend fun executeScript(sql: String)

    /** Closes the database. Never throws. */
    public suspend fun close()
}

/**
 * The binding of the `Db` port (ADR-048): serves its seven methods over a [DbAdapter].
 *
 * * **Ids**: databases and transactions share one counter from 1, never reused.
 * * `open(name, migrations)` checks the name and that versions strictly increase from 1, opens the connection, sets
 *   `foreign_keys = ON`, `busy_timeout` and (unless [wal] is `false` or the database is in memory) `journal_mode = WAL`,
 *   reads `user_version`, refuses a database newer than the newest migration, and runs every pending migration in one
 *   `BEGIN IMMEDIATE` transaction (a failure rolls them all back: `Migration { version, <the error's text> }`), then sets
 *   `user_version`.
 * * Every database has a **serial queue** (one operation at a time, in arrival order) and a transaction slot. `begin` waits
 *   until no transaction is active (at most [busyTimeoutMillis], then `Busy`) and runs `BEGIN IMMEDIATE`; statements on the
 *   transaction's id run inside it; statements on the database's id wait until it ends (at most [busyTimeoutMillis], then
 *   `Busy`). `commit` (`ROLLBACK` when the `COMMIT` fails) and `rollback` end the transaction.
 * * An unknown or closed id is `Unavailable("no open database or transaction <id>")`, an ended transaction
 *   `Unavailable("transaction <id> is over")`. `close` rolls back an active transaction and closes; again is fine.
 * * When the core closes ([PortImpl.detach]) or [close] is called, every database is closed.
 *
 * @param adapter the platform's SQLite.
 * @param busyTimeoutMillis how long a statement waits for another one's transaction (and SQLite for another connection's lock).
 * @param wal whether to switch file databases to write-ahead logging (`false` where the file system has no shared memory).
 */
public class DbPortAdapter(
    private val adapter: DbAdapter,
    private val busyTimeoutMillis: Long = DEFAULT_BUSY_TIMEOUT_MILLIS,
    private val wal: Boolean = true,
) : AutoCloseable {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + CoroutineName("undra-db"))
    private val ids = AtomicInteger(1)
    private val databases = ConcurrentHashMap<UInt, Database>()
    private val transactions = ConcurrentHashMap<UInt, Database>()
    private val closedDatabases: MutableSet<UInt> = ConcurrentHashMap.newKeySet()
    private val endedTransactions: MutableSet<UInt> = ConcurrentHashMap.newKeySet()

    /** How many times [close] ran: an open that was in flight across one is closed as that [close] would have closed it. */
    private val closings = AtomicInteger(0)

    private class Database(val id: UInt, val connection: DbConnection) {
        /** The serial queue: kotlinx's mutex is fair, so operations run in arrival order. */
        val queue = Mutex()

        /** The running transaction's id, `null` when none runs. */
        val activeTx = MutableStateFlow<UInt?>(null)

        @Volatile
        var closed = false
    }

    /**
     * Opens and migrates database [name].
     *
     * @throws DbError as the port's `open` does.
     */
    public suspend fun open(name: String, migrations: List<DbMigration>): DbOpened {
        validateName(name)
        validateMigrations(migrations)
        val closingsBefore = closings.get()
        val connection = try {
            adapter.open(name)
        } catch (e: DbError) {
            throw e
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            throw DbError.Unavailable(describe(e))
        }
        val version = try {
            prepare(connection, name, migrations)
        } catch (e: Throwable) {
            // Also when the caller was cancelled (the core went away): the file must not stay open.
            withContext(NonCancellable) { closeQuietly(connection) }
            throw when (e) {
                is DbError, is CancellationException -> e
                is Exception -> DbError.Sql(describe(e))
                else -> e
            }
        }
        val id = ids.getAndIncrement().toUInt()
        val database = Database(id, connection)
        databases[id] = database
        // A close() (the core went away) that ran while this opened could not see it: it goes too.
        if (closings.get() != closingsBefore) scope.launch { closeDatabase(database) }
        return DbOpened(id, version)
    }

    /** The pragmas, then the migrations; returns the version reached. */
    private suspend fun prepare(connection: DbConnection, name: String, migrations: List<DbMigration>): UInt {
        connection.query("PRAGMA foreign_keys = ON", emptyList())
        connection.query("PRAGMA busy_timeout = $busyTimeoutMillis", emptyList())
        if (wal && name != MEMORY) connection.query("PRAGMA journal_mode = WAL", emptyList())
        val current = (connection.query("PRAGMA user_version", emptyList()).rows.firstOrNull()?.firstOrNull() as? DbValue.Integer)
            ?.value?.coerceIn(0L, UInt.MAX_VALUE.toLong())?.toUInt() ?: 0u
        val newest = migrations.lastOrNull()?.version ?: return current
        if (current > newest) {
            throw DbError.Migration(current, "the database is at version $current, newer than the newest migration ($newest)")
        }
        val pending = migrations.filter { it.version > current }
        if (pending.isEmpty()) return current
        connection.execute("BEGIN IMMEDIATE", emptyList())
        for (migration in pending) {
            try {
                connection.executeScript(migration.sql)
            } catch (e: Exception) {
                if (e is CancellationException) throw e
                rollbackQuietly(connection)
                throw DbError.Migration(migration.version, e.message ?: describe(e))
            }
        }
        try {
            connection.execute("PRAGMA user_version = ${pending.last().version}", emptyList())
            connection.execute("COMMIT", emptyList())
        } catch (e: Exception) {
            if (e is CancellationException) throw e
            rollbackQuietly(connection)
            throw DbError.Migration(pending.last().version, e.message ?: describe(e))
        }
        return pending.last().version
    }

    /**
     * Runs one statement on database or transaction [db].
     *
     * @throws DbError as the port's `execute` does.
     */
    public suspend fun execute(db: UInt, sql: String, params: List<DbValue>): DbExecuted =
        statement(db) { it.execute(sql, params) }

    /**
     * Runs one query on database or transaction [db].
     *
     * @throws DbError as the port's `query` does.
     */
    public suspend fun query(db: UInt, sql: String, params: List<DbValue>): DbRows =
        statement(db) { it.query(sql, params) }

    /**
     * Starts a transaction on database [db] (waiting for a running one, at most the busy timeout) and returns its id.
     *
     * @throws DbError `Busy`, an unknown id, or what `BEGIN IMMEDIATE` failed with.
     */
    public suspend fun begin(db: UInt): UInt {
        val database = databases[db] ?: if (transactions.containsKey(db)) {
            throw DbError.Sql("a transaction cannot begin inside a transaction")
        } else {
            throw unknown(db)
        }
        return whenNoTransaction(database) { connection ->
            sqlFailures { connection.execute("BEGIN IMMEDIATE", emptyList()) }
            val tx = ids.getAndIncrement().toUInt()
            transactions[tx] = database
            database.activeTx.value = tx
            tx
        }
    }

    /**
     * Commits transaction [tx] and ends it; a failed `COMMIT` is rolled back and ends it too.
     *
     * @throws DbError what `COMMIT` failed with, or an unknown or ended id.
     */
    public suspend fun commit(tx: UInt): Unit = finishTransaction(tx, "COMMIT")

    /**
     * Rolls transaction [tx] back and ends it.
     *
     * @throws DbError what `ROLLBACK` failed with, or an unknown or ended id.
     */
    public suspend fun rollback(tx: UInt): Unit = finishTransaction(tx, "ROLLBACK")

    private suspend fun finishTransaction(tx: UInt, sql: String) {
        val database = transactions[tx] ?: throw unknown(tx)
        database.queue.withLock {
            if (database.activeTx.value != tx || database.closed) throw unknown(tx)
            try {
                sqlFailures { database.connection.execute(sql, emptyList()) }
            } catch (e: DbError) {
                if (sql != "ROLLBACK") rollbackQuietly(database.connection)
                endTransaction(database, tx)
                throw e
            }
            endTransaction(database, tx)
        }
    }

    private fun endTransaction(database: Database, tx: UInt) {
        transactions.remove(tx)
        endedTransactions.add(tx)
        database.activeTx.value = null
    }

    /**
     * Closes database [db] (rolling back a running transaction); closing it again is fine.
     *
     * @throws DbError an unknown id.
     */
    public suspend fun close(db: UInt) {
        val database = databases[db] ?: if (db in closedDatabases) return else throw unknown(db)
        closeDatabase(database)
    }

    private suspend fun closeDatabase(database: Database) {
        database.queue.withLock {
            if (database.closed) return
            database.activeTx.value?.let { tx ->
                rollbackQuietly(database.connection)
                transactions.remove(tx)
                endedTransactions.add(tx)
            }
            database.closed = true
            closeQuietly(database.connection)
            // Counted as closed ([openDatabases]) once its connection is.
            closedDatabases.add(database.id)
            databases.remove(database.id)
        }
        // Wake the statements that waited for the transaction: they see the database closed.
        database.activeTx.value = null
    }

    /** How many databases are open (a database closing is counted until its connection is closed). */
    public val openDatabases: Int get() = databases.size

    /**
     * Closes every open database (rolling back running transactions), without waiting, and so every database whose `open` is
     * in flight once it opens. The core does this when it closes ([PortImpl.detach]).
     */
    override fun close() {
        closings.incrementAndGet()
        for (database in databases.values) scope.launch { closeDatabase(database) }
    }

    /** This binding as the async [PortImpl] of [StandardPorts.Db]; it closes every database when detached. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Db.OPEN] = { args ->
                val (name, migrations) = decodeArgs(args) { it.readStr() to migrationList.decode(it) }
                typed(DbError) { DbOpened.encodeToByteArray(open(name, migrations)) }
            }
            this[StandardPorts.Db.EXECUTE] = { args ->
                val (db, sql, params) = decodeArgs(args) { Triple(it.readU32(), it.readStr(), valueList.decode(it)) }
                typed(DbError) { DbExecuted.encodeToByteArray(execute(db, sql, params)) }
            }
            this[StandardPorts.Db.QUERY] = { args ->
                val (db, sql, params) = decodeArgs(args) { Triple(it.readU32(), it.readStr(), valueList.decode(it)) }
                typed(DbError) { DbRows.encodeToByteArray(query(db, sql, params)) }
            }
            this[StandardPorts.Db.BEGIN] = { args ->
                val db = decodeArgs(args) { it.readU32() }
                typed(DbError) { Codecs.u32.encodeToByteArray(begin(db)) }
            }
            this[StandardPorts.Db.COMMIT] = { args ->
                val tx = decodeArgs(args) { it.readU32() }
                typed(DbError) {
                    commit(tx)
                    EMPTY_REPLY
                }
            }
            this[StandardPorts.Db.ROLLBACK] = { args ->
                val tx = decodeArgs(args) { it.readU32() }
                typed(DbError) {
                    rollback(tx)
                    EMPTY_REPLY
                }
            }
            this[StandardPorts.Db.CLOSE] = { args ->
                val db = decodeArgs(args) { it.readU32() }
                typed(DbError) {
                    close(db)
                    EMPTY_REPLY
                }
            }
        },
        detach = ::close,
    )

    // ---- the queue and the transaction slot ----------------------------------------------------------------------------

    private suspend fun <T> statement(id: UInt, block: suspend (DbConnection) -> T): T {
        databases[id]?.let { database -> return whenNoTransaction(database) { sqlFailures { block(it) } } }
        transactions[id]?.let { database ->
            return database.queue.withLock {
                if (database.closed || database.activeTx.value != id) throw unknown(id)
                sqlFailures { block(database.connection) }
            }
        }
        throw unknown(id)
    }

    /** Runs [block] in [database]'s queue once no transaction runs there, waiting at most the busy timeout. */
    private suspend fun <T> whenNoTransaction(database: Database, block: suspend (DbConnection) -> T): T {
        val deadline = System.nanoTime() + busyTimeoutMillis * 1_000_000L
        while (true) {
            if (database.closed) throw unknown(database.id)
            if (database.activeTx.value == null) {
                database.queue.withLock {
                    if (database.closed) throw unknown(database.id)
                    if (database.activeTx.value == null) return block(database.connection)
                }
            }
            val left = (deadline - System.nanoTime()) / 1_000_000L
            if (left <= 0) throw DbError.Busy
            // (`first` answers the null it waited for, so the block answers `true` to tell an end from a timeout.)
            withTimeoutOrNull(left) {
                database.activeTx.first { it == null }
                true
            } ?: throw DbError.Busy
        }
    }

    private fun unknown(id: UInt): DbError =
        if (id in endedTransactions) DbError.Unavailable("transaction $id is over") else DbError.Unavailable("no open database or transaction $id")

    /** Public companion: the limits and checks of the port. */
    public companion object {
        /** The busy timeout of ADR-048: 5 s. */
        public const val DEFAULT_BUSY_TIMEOUT_MILLIS: Long = 5_000L

        /** The name of a private in-memory database. */
        public const val MEMORY: String = ":memory:"

        /** The longest database name. */
        public const val MAX_NAME_LENGTH: Int = 64

        private val migrationList: UndraCodec<List<DbMigration>> = Codecs.vec(DbMigration)
        private val valueList: UndraCodec<List<DbValue>> = Codecs.vec(DbValue)

        /**
         * Checks a database name: [MEMORY], or 1 to 64 of `A-Z a-z 0-9 . _ -` not starting with `.`.
         *
         * @throws DbError.Unavailable for any other name.
         */
        public fun validateName(name: String) {
            val valid = name == MEMORY || (
                name.isNotEmpty() && name.length <= MAX_NAME_LENGTH && !name.startsWith('.') &&
                    name.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '.' || it == '_' || it == '-' }
                )
            if (!valid) {
                throw DbError.Unavailable(
                    "invalid database name \"$name\": use 1 to 64 of A-Z a-z 0-9 . _ - (not starting with .), or \":memory:\"",
                )
            }
        }

        /**
         * Checks that migration versions strictly increase from 1.
         *
         * @throws DbError.Migration naming the first version out of order.
         */
        public fun validateMigrations(migrations: List<DbMigration>) {
            var last = 0u
            for (migration in migrations) {
                if (migration.version <= last) {
                    throw DbError.Migration(migration.version, "migration versions must strictly increase, starting at 1")
                }
                last = migration.version
            }
        }

        private suspend fun rollbackQuietly(connection: DbConnection) {
            try {
                connection.execute("ROLLBACK", emptyList())
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                UndraLog.debug("ROLLBACK failed: $e")
            }
        }

        private suspend fun closeQuietly(connection: DbConnection) {
            try {
                connection.close()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                UndraLog.debug("closing a database failed: $e")
            }
        }

        /** Runs [block]; an exception that is not a [DbError] becomes `Sql` (an adapter's bug must not look like a dead port). */
        private suspend inline fun <T> sqlFailures(block: () -> T): T =
            try {
                block()
            } catch (e: DbError) {
                throw e
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                throw DbError.Sql(describe(e))
            }
    }
}

/** [adapter] served as the `Db` port: `core.registerPort(StandardPorts.Db.PORT_ID, dbPort(adapter))`. */
public fun dbPort(adapter: DbAdapter): PortImpl = DbPortAdapter(adapter).portImpl()

/**
 * The [DbError] of an SQLite failure, chosen by its (extended) [resultCode] as ADR-048 says, with [message] SQLite's text:
 * `BUSY` (5) and `LOCKED` (6) are `Busy`; `CONSTRAINT` (19) is `Constraint` with the kind of its extended code (`UNIQUE` 2067
 * and `PRIMARYKEY` 1555 `UNIQUE`, `NOTNULL` 1299, `FOREIGNKEY` 787, `CHECK` 275, else `OTHER`); `CORRUPT` (11) and `NOTADB`
 * (26) `Corrupt`; `FULL` (13) `Full`; `CANTOPEN` (14), `PERM` (3), `READONLY` (8) and `IOERR` (10) `Unavailable`; anything
 * else `Sql`.
 */
public fun DbError.Companion.fromSqliteCode(resultCode: Int, message: String): DbError =
    when (resultCode and 0xff) {
        5, 6 -> DbError.Busy
        19 -> DbError.Constraint(
            when (resultCode) {
                2067, 1555 -> DbConstraint.UNIQUE
                1299 -> DbConstraint.NOT_NULL
                787 -> DbConstraint.FOREIGN_KEY
                275 -> DbConstraint.CHECK
                else -> DbConstraint.OTHER
            },
            message,
        )
        11, 26 -> DbError.Corrupt(message)
        13 -> DbError.Full
        14, 3, 8, 10 -> DbError.Unavailable(message)
        else -> DbError.Sql(message)
    }
