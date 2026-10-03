package dev.undra.okhttp

import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.ReadAheadSource
import dev.undra.runtime.adapters.WebSocketAdapter
import dev.undra.runtime.adapters.WebSocketConnection
import dev.undra.runtime.adapters.WsError
import dev.undra.runtime.adapters.WsMessage
import java.net.ProtocolException
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString

/**
 * The `WebSocket` port over the app's own [OkHttpClient] (ADR-060, ADR-047): every connection is opened through [client], so
 * its application interceptors, `Authenticator`, certificate pinner, `Dns`, proxy and connection pool apply to the upgrade request
 * with nothing Undra-specific configured, and its dispatcher runs the connection's threads. Serve it with `WebSocketPortAdapter`,
 * which owns the ids, the pull and the read-ahead.
 *
 * It keeps the contract of `ClientWebSocketAdapter` (the default), and the shared suite that checks it
 * (`RealtimeAdapterContract`) runs on this one, with the differences below, which are OkHttp's:
 *
 *  - [protocols] are offered in `Sec-WebSocket-Protocol` and the server's choice is [WebSocketConnection.protocol]; headers go into
 *    the upgrade request as given, except the ones the client writes itself (`Host`, `Upgrade`, `Connection`, `Sec-WebSocket-*`) and
 *    invalid ones, which are refused (`Refused`), never dropped.
 *  - A refused upgrade is `Refused` with its HTTP status; an unreachable server, a reset or a dead connection is `Network`; a
 *    handshake or a frame that breaks RFC 6455 as OkHttp checks it is `Protocol`; the peer's close frame is `Closed` with its code and
 *    reason, after the messages before it, and is answered with the same code.
 *  - **Backpressure**: OkHttp reads frames on a thread of its own and has no way to pause it, but it waits for the listener to
 *    return before it reads the next frame, and the listener does not return while the binding's buffer is full. A core that stops
 *    reading leaves the server's messages in TCP's buffers and the server's writes stall, with one frame read beyond the window.
 *    (The reader cannot answer a ping meanwhile: with a ping interval, a core that stops reading for longer than it loses the
 *    connection to OkHttp's missing-pong timeout.)
 *  - `send` returns once OkHttp queued the frame and fewer than 1 MiB wait to be written. OkHttp's queue holds 16 MiB: a message
 *    that does not fit closes the connection with 1001 (going away), which is OkHttp's rule.
 *  - **Text is not checked for UTF-8**: OkHttp decodes a text frame itself and replaces a malformed sequence with U+FFFD, where the
 *    default adapter closes with 1007 and ends the stream with `Protocol` (RFC 6455 section 8.1). It has no limit on the size of an
 *    inbound message either. An app that needs either keeps `ClientWebSocketAdapter` for this port.
 *  - OkHttp opens a WebSocket on a client of its own derived from the app's: HTTP/1.1 only, with **no event listener**
 *    (`EventListener.NONE`), and runs the upgrade **without the client's network interceptors**: the app's tracing sees it only as
 *    an application interceptor. Everything else of the app's client is used.
 *  - **Liveness**: a connection the network silently dropped is only noticed by sending something, so unless the client has a ping
 *    interval of its own the adapter gives it one ([pingIntervalMillis]), as the default adapter pings a silent server.
 *
 * @param client the app's client, asked for on every connect.
 * @param pingIntervalMillis how often OkHttp pings the server (and gives up on one that does not answer the next ping); `null` for
 *   the client's own `pingInterval` and [DEFAULT_PING_INTERVAL_MILLIS] when it has none (OkHttp's default is never), `0` for never.
 */
public class OkHttpWebSocketAdapter(
    private val client: () -> OkHttpClient,
    private val pingIntervalMillis: Long? = null,
) : WebSocketAdapter {
    /** An adapter over [client]; see the class documentation. */
    public constructor(client: OkHttpClient, pingIntervalMillis: Long? = null) : this({ client }, pingIntervalMillis)

    init {
        require(pingIntervalMillis == null || pingIntervalMillis >= 0) { "pingIntervalMillis must not be negative, got $pingIntervalMillis" }
    }

    override suspend fun connect(url: String, protocols: List<String>, headers: List<Header>): WebSocketConnection {
        val scheme = url.substringBefore("://", "").lowercase()
        if (scheme != "ws" && scheme != "wss") throw WsError.Refused(null, "invalid URL: $url (only ws:// and wss:// URLs are supported)")
        for (protocol in protocols) {
            OkHttpRules.subprotocolProblem(protocol)?.let { throw WsError.Refused(null, it) }
        }
        val builder = try {
            Request.Builder().url(url)
        } catch (e: IllegalArgumentException) {
            throw WsError.Refused(null, "invalid URL: $url (${OkHttpRules.describe(e)})")
        }
        for (header in headers) {
            OkHttpRules.webSocketHeaderProblem(header.name, header.value)?.let { throw WsError.Refused(null, it) }
            try {
                builder.addHeader(header.name, header.value)
            } catch (e: IllegalArgumentException) {
                throw WsError.Refused(null, "header '${header.name}' is not allowed: ${OkHttpRules.describe(e)}")
            }
        }
        if (protocols.isNotEmpty()) builder.header("Sec-WebSocket-Protocol", protocols.joinToString(", "))

        val connection = OkHttpWebSocketConnection()
        val socket = try {
            withPing(client()).newWebSocket(builder.build(), connection)
        } catch (e: IllegalArgumentException) {
            throw WsError.Refused(null, OkHttpRules.describe(e))
        }
        connection.attach(socket)
        // The failure travels as a value and is thrown here: an exception resumed through a coroutine may be rebuilt by the
        // debugger-friendly stack trace recovery, which would put its own text into the typed error's reason.
        val failure = try {
            connection.opened.await()
        } catch (e: CancellationException) {
            // Nobody will read or close a connection whose connect was cancelled (the core went away mid-handshake).
            connection.abandon()
            throw e
        }
        if (failure != null) throw failure
        return connection
    }

    /** [base], with the ping interval this adapter wants (the client itself when it already has it). */
    internal fun withPing(base: OkHttpClient): OkHttpClient {
        val wanted = pingIntervalMillis ?: if (base.pingIntervalMillis == 0) DEFAULT_PING_INTERVAL_MILLIS else return base
        if (wanted == base.pingIntervalMillis.toLong()) return base
        return base.newBuilder().pingInterval(wanted, TimeUnit.MILLISECONDS).build()
    }

    /** Defaults. */
    public companion object {
        /** How often a connection is pinged when neither the adapter nor the client says: 30 s, the default adapter's silence limit. */
        public const val DEFAULT_PING_INTERVAL_MILLIS: Long = 30_000L
    }
}

/**
 * One connection of [OkHttpWebSocketAdapter]: OkHttp's listener (what its reader thread hears goes into [inbox]) and the read gate
 * (the reader thread waits in the listener while a message is waiting for the binding).
 */
private class OkHttpWebSocketConnection : WebSocketListener(), WebSocketConnection, ReadAheadSource {
    private sealed interface Item {
        class Message(val message: WsMessage) : Item

        class End(val error: WsError) : Item
    }

    /** Completes with `null` when the upgrade succeeded, or with the typed [WsError] of a connect that did not. */
    val opened = CompletableDeferred<WsError?>()

    /** Completes when OkHttp is done with the connection (its `onClosed` or `onFailure`). */
    private val finished = CompletableDeferred<Unit>()

    private val inbox = Channel<Item>(Channel.UNLIMITED)
    private val gate = ReentrantLock()
    private val roomChanged = gate.newCondition()

    /** Messages the reader put into [inbox] that the flow has not taken yet. Guarded by [gate]. */
    private var waiting = 0

    /** How many messages the reader may leave waiting: the room of the binding's buffer (one without a binding). Guarded by [gate]. */
    private var room = 1

    @Volatile
    private var stopping = false

    @Volatile
    private var end: WsError? = null

    @Volatile
    private var socket: WebSocket? = null

    @Volatile
    private var chosen = ""
    private val collected = AtomicBoolean(false)

    fun attach(socket: WebSocket) {
        this.socket = socket
    }

    override val protocol: String get() = chosen

    override val messages: Flow<WsMessage> = flow {
        if (!collected.compareAndSet(false, true)) throw WsError.Protocol("the messages of this connection are already being read")
        for (item in inbox) {
            gate.withLock {
                waiting--
                roomChanged.signalAll()
            }
            when (item) {
                is Item.Message -> emit(item.message)
                is Item.End -> throw item.error
            }
        }
    }

    // ---- OkHttp's reader thread ---------------------------------------------------------------------------------

    override fun onOpen(webSocket: WebSocket, response: Response) {
        chosen = response.header("Sec-WebSocket-Protocol").orEmpty()
        opened.complete(null)
    }

    override fun onMessage(webSocket: WebSocket, text: String) = deliver(WsMessage.Text(text))

    override fun onMessage(webSocket: WebSocket, bytes: ByteString) = deliver(WsMessage.Binary(bytes.toByteArray()))

    override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
        finish(WsError.Closed(code.toUShort(), reason))
        // OkHttp leaves the answer to the listener: echo the peer's code, or a plain 1000 for one that cannot be sent (1005: none).
        val answered = try {
            webSocket.close(code, reason)
        } catch (e: IllegalArgumentException) {
            webSocket.close(NORMAL_CLOSURE, "")
        }
        if (!answered) webSocket.cancel()
    }

    override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
        finish(WsError.Closed(code.toUShort(), reason))
        finished.complete(Unit)
    }

    override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
        if (!opened.isCompleted) {
            // The upgrade failed: refused with the server's status, or broke the protocol, or never got an answer.
            val code = response?.code
            val error = when {
                code != null && code != 101 -> WsError.Refused(code.toUShort(), "the server answered HTTP $code")
                response != null || t is ProtocolException -> WsError.Protocol(OkHttpRules.describe(t))
                else -> WsError.Network(OkHttpRules.describe(t))
            }
            opened.complete(error)
        }
        finish(if (t is ProtocolException) WsError.Protocol(OkHttpRules.describe(t)) else WsError.Network(OkHttpRules.describe(t)))
        finished.complete(Unit)
    }

    /** Queues [message] for the binding, then holds OkHttp's reader thread while the binding's buffer is full. */
    private fun deliver(message: WsMessage) {
        if (stopping) return
        gate.withLock { waiting++ }
        inbox.trySend(Item.Message(message))
        gate.withLock {
            while (waiting >= room && !stopping) roomChanged.awaitUninterruptibly()
        }
    }

    /** Ends the stream with [error] unless it already ended. */
    private fun finish(error: WsError) {
        if (end != null) return
        end = error
        inbox.trySend(Item.End(error))
        inbox.close()
    }

    override fun setRoom(room: Int) {
        gate.withLock {
            this.room = room
            roomChanged.signalAll()
        }
    }

    // ---- the binding --------------------------------------------------------------------------------------------

    override suspend fun send(message: WsMessage) {
        end?.let { throw it }
        val s = socket ?: throw WsError.Network("the WebSocket is not connected")
        val queued = when (message) {
            is WsMessage.Text -> s.send(message.value)
            is WsMessage.Binary -> s.send(message.value.toByteString())
        }
        if (!queued) throw end ?: WsError.Network("the WebSocket is closing, or the message does not fit OkHttp's 16 MiB outgoing queue")
        // ADR-047 §4: a sender that awaits its sends cannot outrun the network.
        while (s.queueSize() > MAX_OUTBOUND_BYTES && end == null) delay(SEND_POLL_MILLIS)
        end?.let { throw it }
    }

    /** Drops a connection nobody will use (its connect was cancelled): the reader stops waiting and the socket goes. */
    fun abandon() {
        stopping = true
        gate.withLock { roomChanged.signalAll() }
        inbox.close()
        socket?.cancel()
    }

    override suspend fun close(code: Int, reason: String) {
        stopping = true
        gate.withLock { roomChanged.signalAll() }
        inbox.close()
        val s = socket ?: return
        val started = try {
            s.close(code, reason)
        } catch (e: IllegalArgumentException) {
            s.close(NORMAL_CLOSURE, "") // a code or a reason the protocol does not allow: the handshake still ends
        }
        // Returns once the server echoed the close (it has seen ours), or after the grace period. A close that was already under way
        // (the peer's, which onClosing answered) is waited for in the same way; one that cannot start is not.
        if (started || !finished.isCompleted) withTimeoutOrNull(CLOSE_WAIT_MILLIS) { finished.await() }
        s.cancel() // what is still open goes now; a closed connection ignores it
    }

    private companion object {
        const val MAX_OUTBOUND_BYTES = 1L shl 20
        const val SEND_POLL_MILLIS = 2L
        const val CLOSE_WAIT_MILLIS = 3_000L
        const val NORMAL_CLOSURE = 1000
    }
}
