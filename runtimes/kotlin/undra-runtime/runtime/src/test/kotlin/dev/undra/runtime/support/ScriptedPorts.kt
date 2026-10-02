package dev.undra.runtime.support

import dev.undra.runtime.adapters.DbAdapter
import dev.undra.runtime.adapters.DbConnection
import dev.undra.runtime.adapters.DbExecuted
import dev.undra.runtime.adapters.DbRows
import dev.undra.runtime.adapters.DbValue
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.SseAdapter
import dev.undra.runtime.adapters.SseEvent
import dev.undra.runtime.adapters.SseStream
import dev.undra.runtime.adapters.WebSocketAdapter
import dev.undra.runtime.adapters.WebSocketConnection
import dev.undra.runtime.adapters.WsMessage
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow

/** What a scripted stream hands out: an item, an error that ends it, or its quiet end. */
sealed interface Scripted<out T> {
    class Item<T>(val value: T) : Scripted<T>

    class Fail(val error: Throwable) : Scripted<Nothing>

    data object Finish : Scripted<Nothing>
}

/**
 * The inbound side of a scripted connection: the test pushes items; [taken] counts what the binding's pump took from the
 * flow, which is the binding's read-ahead.
 */
class ScriptedInbound<T> {
    private val outbox = Channel<Scripted<T>>(Channel.UNLIMITED)
    private val takenCount = AtomicInteger(0)
    private val collections = AtomicInteger(0)

    /** How many items the binding has taken so far. */
    val taken: Int get() = takenCount.get()

    /** How many times the flow was collected. */
    val collected: Int get() = collections.get()

    /** The flow the binding collects. */
    val flow: Flow<T> = flow {
        collections.incrementAndGet()
        for (item in outbox) {
            when (item) {
                is Scripted.Item -> {
                    takenCount.incrementAndGet()
                    emit(item.value)
                }
                is Scripted.Fail -> throw item.error
                Scripted.Finish -> return@flow
            }
        }
    }

    fun push(value: T) {
        outbox.trySend(Scripted.Item(value))
    }

    fun fail(error: Throwable) {
        outbox.trySend(Scripted.Fail(error))
    }

    fun finish() {
        outbox.trySend(Scripted.Finish)
    }

    fun stop() {
        outbox.close()
    }
}

/** A WebSocket "server" in memory: every connect is accepted (the first offered subprotocol chosen) unless scripted otherwise. */
class ScriptedWebSocket : WebSocketAdapter {
    /** Every connection made, in order. */
    val connections = CopyOnWriteArrayList<Conn>()

    /** What the next connects throw (`null`: accept). */
    @Volatile
    var failConnect: Throwable? = null

    /** When set, every connect waits for it before it answers (a handshake in flight). */
    @Volatile
    var connectGate: CompletableDeferred<Unit>? = null

    /** How many connects began (before the gate). */
    val connectsStarted = AtomicInteger(0)

    inner class Conn(val url: String, val protocols: List<String>, val headers: List<Header>) : WebSocketConnection {
        val inbound = ScriptedInbound<WsMessage>()
        val sent = CopyOnWriteArrayList<WsMessage>()
        val closes = CopyOnWriteArrayList<Pair<Int, String>>()

        @Volatile
        var failSend: Throwable? = null

        override val protocol: String = protocols.firstOrNull().orEmpty()
        override val messages: Flow<WsMessage> get() = inbound.flow

        override suspend fun send(message: WsMessage) {
            failSend?.let { throw it }
            sent.add(message)
        }

        override suspend fun close(code: Int, reason: String) {
            closes.add(code to reason)
            inbound.stop()
        }
    }

    override suspend fun connect(url: String, protocols: List<String>, headers: List<Header>): WebSocketConnection {
        connectsStarted.incrementAndGet()
        connectGate?.await()
        failConnect?.let { throw it }
        return Conn(url, protocols, headers).also { connections.add(it) }
    }
}

/** An event-stream "server" in memory. */
class ScriptedSse : SseAdapter {
    val streams = CopyOnWriteArrayList<Stream>()

    @Volatile
    var failOpen: Throwable? = null

    /** When set, every open waits for it before it answers (a request in flight). */
    @Volatile
    var openGate: CompletableDeferred<Unit>? = null

    /** How many opens began (before the gate). */
    val opensStarted = AtomicInteger(0)

    inner class Stream(val url: String, val headers: List<Header>, val lastEventId: String?) : SseStream {
        val inbound = ScriptedInbound<SseEvent>()

        @Volatile
        var closed = 0

        override val events: Flow<SseEvent> get() = inbound.flow

        override suspend fun close() {
            closed++
            inbound.stop()
        }
    }

    override suspend fun open(url: String, headers: List<Header>, lastEventId: String?): SseStream {
        opensStarted.incrementAndGet()
        openGate?.await()
        failOpen?.let { throw it }
        return Stream(url, headers, lastEventId).also { streams.add(it) }
    }
}

/**
 * A scripted SQLite: no SQL engine. It records every call per database (in order), answers the pragmas the binding sends,
 * keeps `user_version` per name across opens, fails the statements a rule names, and checks that the binding never runs
 * two calls on one connection at once.
 */
class ScriptedDb : DbAdapter {
    /** One call as the adapter saw it. */
    data class Call(val db: String, val kind: String, val sql: String, val params: List<DbValue>)

    val calls = CopyOnWriteArrayList<Call>()
    val versions = ConcurrentHashMap<String, Long>()
    private val failures = CopyOnWriteArrayList<Pair<String, Throwable>>()
    val opened = CopyOnWriteArrayList<Conn>()

    /** How long every statement takes (to make overlaps visible). */
    @Volatile
    var statementMillis = 0L

    @Volatile
    var failOpen: Throwable? = null

    /** When set, every open waits for it before it answers (a file being opened). */
    @Volatile
    var openGate: CompletableDeferred<Unit>? = null

    /** How many opens began (before the gate). */
    val opensStarted = AtomicInteger(0)

    /** Statements (and scripts) whose SQL contains [fragment] fail with [error]. */
    fun failOn(fragment: String, error: Throwable) {
        failures.add(fragment to error)
    }

    /** The calls of kind [kind] (`execute`, `query`, `script`, `close`). */
    fun calls(kind: String): List<Call> = calls.filter { it.kind == kind }

    inner class Conn(val name: String) : DbConnection {
        private val inFlight = AtomicInteger(0)

        @Volatile
        var closed = false

        @Volatile
        var overlapped = false

        private suspend fun <T> step(kind: String, sql: String, params: List<DbValue>, answer: () -> T): T {
            if (inFlight.incrementAndGet() > 1) overlapped = true
            try {
                calls.add(Call(name, kind, sql, params))
                if (statementMillis > 0) delay(statementMillis)
                failures.firstOrNull { sql.contains(it.first) }?.let { throw it.second }
                return answer()
            } finally {
                inFlight.decrementAndGet()
            }
        }

        override suspend fun execute(sql: String, params: List<DbValue>): DbExecuted = step("execute", sql, params) {
            Regex("PRAGMA user_version = (\\d+)").find(sql)?.let { versions[name] = it.groupValues[1].toLong() }
            DbExecuted(if (sql.startsWith("INSERT")) 1uL else 0uL, if (sql.startsWith("INSERT")) 42L else 0L)
        }

        override suspend fun query(sql: String, params: List<DbValue>): DbRows = step("query", sql, params) {
            when (sql) {
                "PRAGMA user_version" -> DbRows(listOf("user_version"), listOf(listOf(DbValue.Integer(versions[name] ?: 0L))))
                "PRAGMA journal_mode = WAL" -> DbRows(listOf("journal_mode"), listOf(listOf(DbValue.Text("wal"))))
                else -> DbRows(listOf("n"), listOf(listOf(DbValue.Integer(params.size.toLong()))))
            }
        }

        override suspend fun executeScript(sql: String): Unit = step("script", sql, emptyList()) {}

        override suspend fun close() {
            calls.add(Call(name, "close", "", emptyList()))
            closed = true
        }
    }

    override suspend fun open(name: String): DbConnection {
        opensStarted.incrementAndGet()
        openGate?.await()
        failOpen?.let { throw it }
        return Conn(name).also { opened.add(it) }
    }
}
