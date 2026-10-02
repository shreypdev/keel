package dev.undra.runtime.adapters

import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraLog
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.launch

/**
 * Opens WebSocket connections for the `WebSocket` port (ADR-047). Implement it to replace the default
 * ([ClientWebSocketAdapter]) and serve it with [WebSocketPortAdapter], which owns the ids, the pull and the read-ahead.
 *
 * ```kotlin
 * core.registerPort(StandardPorts.WebSocket.PORT_ID, webSocketPort(MyAdapter()))
 * ```
 */
public interface WebSocketAdapter {
    /**
     * Opens a connection to [url] (`ws://` or `wss://`, checked by the binding first), offering [protocols] and sending
     * [headers] with the upgrade request; returns once the handshake succeeded.
     *
     * @throws WsError `Refused` (with the upgrade's HTTP status where the platform reports it) for a refused upgrade, an
     *   unusable URL or a header the platform cannot send; `Network` when the server cannot be reached; `Protocol` for a
     *   handshake that breaks RFC 6455. Anything else thrown is reported as `Network`.
     */
    public suspend fun connect(url: String, protocols: List<String>, headers: List<Header>): WebSocketConnection
}

/** One open connection of a [WebSocketAdapter]. */
public interface WebSocketConnection {
    /** The subprotocol the server chose, `""` for none. */
    public val protocol: String

    /**
     * The inbound messages, collected once, by the binding. It must be lazy: the binding stops collecting while its buffer
     * is full, and a connection should then stop reading its socket so that TCP pushes back on the server. The flow ends by
     * throwing a [WsError] (`Closed` for the peer's close frame, `Network` for a drop, `Protocol` for a broken frame or a
     * text message that is not UTF-8), or by completing after [close].
     */
    public val messages: Flow<WsMessage>

    /**
     * Sends [message]; returns when the platform accepted it and its outbound buffer is small again (ADR-047 §4).
     *
     * @throws WsError the connection's end (`Closed`, `Network`, `Protocol`) when it is over.
     */
    public suspend fun send(message: WsMessage)

    /** Starts the closing handshake with [code] and [reason] and returns once it is done (or gave up). Never throws. */
    public suspend fun close(code: Int, reason: String)
}

/**
 * The binding of the `WebSocket` port (ADR-047): serves the port's four methods over a [WebSocketAdapter].
 *
 * * `connect` asks the adapter (a URL that is not `ws://` or `wss://` is refused first), registers the connection under
 *   the next id (from 1, never reused) and starts a **pump** that collects the adapter's [WebSocketConnection.messages] into
 *   a buffer only while the buffer holds fewer than the window: the `max` of the latest `receive` (16 before the first).
 *   A full buffer stops the collection, so the adapter stops reading its socket.
 * * `receive(conn, max)` answers the buffered messages (up to `max`) at once, else waits for one; after the core's close it
 *   answers `[]`; after the connection's end (the flow's [WsError], or `Network("the connection ended")` when it completed
 *   on its own) it fails with that end, every time. One `receive` per connection at a time.
 * * `send` fails with the core's own close (`Closed` with its code and reason) or the connection's end, else sends.
 * * `close` marks the connection closed (a second close is fine), answers a waiting `receive` with `[]`, drops the buffer,
 *   stops the pump and closes the adapter's connection.
 * * When the core closes ([PortImpl.detach]) or [close] is called, every open connection is closed with 1001 (going away).
 *
 * Errors are the port's typed [WsError]; an unknown id is `Network("no WebSocket connection <id>")`.
 *
 * @param adapter the platform's connections.
 */
public class WebSocketPortAdapter(private val adapter: WebSocketAdapter) : AutoCloseable {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + CoroutineName("undra-websocket"))
    private val nextId = AtomicInteger(1)
    private val lines = ConcurrentHashMap<UInt, Line>()

    /** Connections the core closed: only how, so that later calls answer as the port says (`[]`, `Closed`, `Ok`). */
    private val closed = ConcurrentHashMap<UInt, WsError.Closed>()

    /** How many times [close] ran: a connect that was in flight across one is closed as that [close] would have closed it. */
    private val closings = AtomicInteger(0)

    private class Line(val id: UInt, val connection: WebSocketConnection, val inbound: PulledStream<WsMessage>)

    /**
     * Opens a connection through the adapter and returns its id and subprotocol.
     *
     * @throws WsError as the port's `connect` does.
     */
    public suspend fun connect(url: String, protocols: List<String>, headers: List<Header>): WsOpened {
        if (!(url.startsWith("ws://", ignoreCase = true) || url.startsWith("wss://", ignoreCase = true))) {
            throw WsError.Refused(null, "invalid URL: $url")
        }
        val closingsBefore = closings.get()
        val connection = try {
            adapter.connect(url, protocols, headers)
        } catch (e: WsError) {
            throw e
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            throw WsError.Network(describe(e))
        }
        val id = nextId.getAndIncrement().toUInt()
        val inbound = PulledStream(
            source = connection.messages,
            scope = scope,
            finishedOnItsOwn = { WsError.Network("the connection ended") },
            typed = { e -> e as? WsError ?: WsError.Network(describe(e)) },
            busy = { WsError.Protocol("a receive is already pending on connection $id") },
            readAhead = connection as? ReadAheadSource,
        )
        val line = Line(id, connection, inbound)
        lines[id] = line
        inbound.start()
        // A close() (the core went away) that ran while the adapter connected could not see this connection: it goes too.
        if (closings.get() != closingsBefore && markClosed(line, GOING_AWAY, "")) scope.launch { closeQuietly(line, GOING_AWAY, "") }
        return WsOpened(id, connection.protocol)
    }

    /**
     * The next messages of connection [conn], at least one and at most [max], once there are any; `[]` after the core's
     * close.
     *
     * @throws WsError the connection's end, an unknown id, or a second `receive` while one waits.
     */
    public suspend fun receive(conn: UInt, max: UInt): List<WsMessage> {
        val line = lines[conn] ?: if (closed.containsKey(conn)) return emptyList() else throw unknown(conn)
        return line.inbound.pull(max)
    }

    /**
     * Sends [message] on connection [conn].
     *
     * @throws WsError the core's close (`Closed`), the connection's end, or an unknown id.
     */
    public suspend fun send(conn: UInt, message: WsMessage) {
        val line = lines[conn] ?: throw (closed[conn] ?: unknown(conn))
        line.inbound.end?.let { throw it }
        try {
            line.connection.send(message)
        } catch (e: WsError) {
            throw closed[conn] ?: e
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            throw WsError.Network(describe(e))
        }
    }

    /**
     * Closes connection [conn] with [code] and [reason]; closing it again is fine.
     *
     * @throws WsError `Network` for an unknown id.
     */
    public suspend fun close(conn: UInt, code: UShort, reason: String) {
        val line = lines[conn] ?: if (closed.containsKey(conn)) return else throw unknown(conn)
        if (!markClosed(line, code, reason)) return
        closeQuietly(line, code, reason)
    }

    /** Marks [line] closed by the core and leaves a tombstone in its place; `false` if it already was closed. */
    private fun markClosed(line: Line, code: UShort, reason: String): Boolean {
        if (!line.inbound.markClosed()) return false
        closed[line.id] = WsError.Closed(code, reason)
        lines.remove(line.id)
        return true
    }

    private suspend fun closeQuietly(line: Line, code: UShort, reason: String) {
        try {
            line.connection.close(code.toInt(), reason)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            UndraLog.debug("closing WebSocket connection ${line.id} failed: $e")
        }
    }

    /** How many connections are open (not closed by the core). */
    public val openConnections: Int get() = lines.size

    /**
     * Closes every open connection with 1001 (going away), without waiting for the handshakes, and so every connection whose
     * `connect` is in flight once it opens. The binding stays usable. The core does this when it closes ([PortImpl.detach]).
     */
    override fun close() {
        closings.incrementAndGet()
        for (line in lines.values) {
            if (markClosed(line, GOING_AWAY, "")) scope.launch { closeQuietly(line, GOING_AWAY, "") }
        }
    }

    /** This binding as the async [PortImpl] of [StandardPorts.WebSocket]; it closes every connection when detached. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.WebSocket.CONNECT] = { args ->
                val (url, protocols, headers) = decodeArgs(args) { Triple(it.readStr(), stringList.decode(it), headerList.decode(it)) }
                typed(WsError) { WsOpened.encodeToByteArray(connect(url, protocols, headers)) }
            }
            this[StandardPorts.WebSocket.SEND] = { args ->
                val (conn, message) = decodeArgs(args) { it.readU32() to WsMessage.decode(it) }
                typed(WsError) {
                    send(conn, message)
                    EMPTY_REPLY
                }
            }
            this[StandardPorts.WebSocket.RECEIVE] = { args ->
                val (conn, max) = decodeArgs(args) { it.readU32() to it.readU32() }
                typed(WsError) { messageList.encodeToByteArray(receive(conn, max)) }
            }
            this[StandardPorts.WebSocket.CLOSE] = { args ->
                val (conn, code, reason) = decodeArgs(args) { Triple(it.readU32(), it.readU16(), it.readStr()) }
                typed(WsError) {
                    close(conn, code, reason)
                    EMPTY_REPLY
                }
            }
        },
        detach = ::close,
    )

    private companion object {
        /** The close code of a connection the core left open when it went away. */
        val GOING_AWAY: UShort = 1001u

        val stringList: UndraCodec<List<String>> = Codecs.vec(Codecs.string)
        val headerList: UndraCodec<List<Header>> = Codecs.vec(Header)
        val messageList: UndraCodec<List<WsMessage>> = Codecs.vec(WsMessage)

        fun unknown(conn: UInt): WsError = WsError.Network("no WebSocket connection $conn")
    }
}

/** [adapter] served as the `WebSocket` port: `core.registerPort(StandardPorts.WebSocket.PORT_ID, webSocketPort(adapter))`. */
public fun webSocketPort(adapter: WebSocketAdapter): PortImpl = WebSocketPortAdapter(adapter).portImpl()

// ---- shared by the bindings of the opt-in ports ---------------------------------------------------------------------

internal val EMPTY_REPLY: ByteArray = ByteArray(0)

/** Decodes the arguments of a port method with [read] and requires that nothing is left over. */
internal inline fun <T> decodeArgs(args: ByteArray, read: (dev.undra.runtime.wire.UndraReader) -> T): T {
    val reader = dev.undra.runtime.wire.UndraReader(args)
    val value = read(reader)
    reader.finish()
    return value
}

/** Runs [body]; a typed error [E] becomes the port's error reply (an [UndraPortException] carrying its encoding). */
internal suspend inline fun <reified E : Throwable> typed(codec: UndraCodec<E>, body: () -> ByteArray): ByteArray =
    try {
        body()
    } catch (e: Throwable) {
        if (e is E) throw UndraPortException(codec.encodeToByteArray(e)) else throw e
    }

/** A short description of [e] for a typed error's text: its message, or its class name when it has none. */
internal fun describe(e: Throwable): String = e.message?.takeIf { it.isNotBlank() } ?: e.javaClass.simpleName
