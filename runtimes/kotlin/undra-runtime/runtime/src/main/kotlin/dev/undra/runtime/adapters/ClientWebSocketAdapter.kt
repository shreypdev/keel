package dev.undra.runtime.adapters

import dev.undra.runtime.WebSocketClient
import dev.undra.runtime.WebSocketHandshakeException
import dev.undra.runtime.WebSocketProtocolException
import dev.undra.runtime.WebSocketUpgradeException
import java.io.IOException
import java.net.URI
import java.net.URISyntaxException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import javax.net.ssl.SSLSocketFactory
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.withContext

/**
 * The default [WebSocketAdapter] on the JVM and on Android (ADR-047): the runtime's own RFC 6455 client over
 * `java.net.Socket`, the one the remote transport uses (`java.net.http.WebSocket` does not exist on Android).
 *
 * * Text messages arrive as [WsMessage.Text] once checked to be UTF-8 (a text message that is not is closed with 1007 and
 *   ends the stream with `Protocol`); binary as [WsMessage.Binary]. Fragments are joined; pings are answered.
 * * [protocols] are offered in `Sec-WebSocket-Protocol` and the server's choice is [WebSocketConnection.protocol]; headers
 *   go into the upgrade request as given, except the ones the client writes itself (`Host`, `Upgrade`, `Connection`,
 *   `Sec-WebSocket-*`) and invalid ones, which are refused (`Refused`), never dropped.
 * * **Backpressure**: the reader thread reads only while the binding's buffer has room (the window of the core's latest
 *   pull), so a core that stops reading leaves the server's messages in TCP's buffers and the server's writes stall.
 * * A refused upgrade is `Refused` with its HTTP status; an unreachable server, a reset or a dead connection (no answer to a
 *   ping for twice [pingAfterMillis]) is `Network`; a peer that breaks RFC 6455 is `Protocol`; the peer's close frame is
 *   `Closed` with its code and reason.
 * * `send` returns once the frame is queued and fewer than 1 MiB wait to be written.
 * * `wss` verifies the certificate and the host name (see [sslSocketFactory]). On Android the client's sockets are not
 *   subject to the network security config's cleartext rule (unlike `HttpURLConnection`): only `ws://` URLs the app passes
 *   are used.
 *
 * @param connectTimeoutMillis how long the TCP connect, and then the handshake, may each take.
 * @param maxMessageBytes the largest message accepted; a larger one closes the connection with 1009 (`Protocol`).
 * @param pingAfterMillis how long the server may be silent before it is pinged.
 * @param sslSocketFactory the TLS sockets for `wss`; `null` for the platform's default.
 */
public class ClientWebSocketAdapter(
    private val connectTimeoutMillis: Int = 30_000,
    private val maxMessageBytes: Int = 64 * 1024 * 1024,
    private val pingAfterMillis: Long = 30_000L,
    private val sslSocketFactory: SSLSocketFactory? = null,
) : WebSocketAdapter {
    override suspend fun connect(url: String, protocols: List<String>, headers: List<Header>): WebSocketConnection {
        val uri = try {
            URI(url)
        } catch (e: URISyntaxException) {
            throw WsError.Refused(null, "invalid URL: $url (${e.reason})")
        }
        val scheme = uri.scheme?.lowercase()
        if (scheme != "ws" && scheme != "wss") throw WsError.Refused(null, "invalid URL: $url (only ws:// and wss:// URLs are supported)")
        if (uri.host.isNullOrEmpty()) throw WsError.Refused(null, "invalid URL: $url (the URL has no host)")
        for (protocol in protocols) {
            WebSocketClient.checkProtocol(protocol)?.let { throw WsError.Refused(null, it) }
        }
        for (header in headers) {
            WebSocketClient.checkHeader(header.name, header.value)?.let { throw WsError.Refused(null, it) }
        }
        val connection = ClientWebSocketConnection()
        // The blocking handshake does not notice a cancelled caller; what it opened anyway is dropped below.
        val opened = AtomicReference<WebSocketClient?>(null)
        val client = try {
            withContext(Dispatchers.IO) {
                runInterruptible {
                    WebSocketClient.connect(
                        uri,
                        connection,
                        connectTimeoutMillis,
                        maxMessageBytes,
                        pingAfterMillis,
                        sslSocketFactory,
                        protocols = protocols,
                        headers = headers.map { it.name to it.value },
                        readGate = connection,
                    ).also { opened.set(it) }
                }
            }
        } catch (e: CancellationException) {
            // Nobody will read or close a connection whose connect was cancelled (the core went away mid-handshake).
            connection.abandon(opened.get())
            throw e
        } catch (e: WebSocketUpgradeException) {
            throw WsError.Refused(if (e.status in 0..65535) e.status.toUShort() else null, describe(e))
        } catch (e: WebSocketHandshakeException) {
            throw WsError.Protocol(describe(e))
        } catch (e: IOException) {
            throw WsError.Network(describe(e))
        } catch (e: IllegalArgumentException) {
            throw WsError.Refused(null, describe(e))
        } catch (e: SecurityException) {
            throw WsError.Network(describe(e))
        }
        connection.attach(client)
        return connection
    }
}

/**
 * One connection of [ClientWebSocketAdapter]: the client's listener (what the reader thread hears goes into [inbox]) and its
 * read gate (the reader waits while a message is waiting for the binding).
 */
private class ClientWebSocketConnection : WebSocketConnection, WebSocketClient.Listener, WebSocketClient.ReadGate, ReadAheadSource {
    private sealed interface Item {
        class Message(val message: WsMessage) : Item

        class End(val error: WsError) : Item
    }

    private val inbox = Channel<Item>(Channel.UNLIMITED)
    private val gate = ReentrantLock()
    private val roomChanged = gate.newCondition()

    /** Messages the reader put into [inbox] that the flow has not taken yet. Guarded by [gate]. */
    private var waiting = 0

    /** How many messages the reader may have waiting: the room of the binding's buffer (one without a binding). Guarded by [gate]. */
    private var room = 1

    @Volatile
    private var stopping = false

    @Volatile
    private var end: WsError? = null

    @Volatile
    private var client: WebSocketClient? = null
    private val collected = AtomicBoolean(false)

    fun attach(client: WebSocketClient) {
        this.client = client
    }

    override val protocol: String get() = client?.protocol.orEmpty()

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

    // ---- the reader thread ----------------------------------------------------------------------------------------

    override fun awaitOpen() {
        gate.withLock {
            while (waiting >= room && !stopping) roomChanged.awaitUninterruptibly()
        }
    }

    override fun setRoom(room: Int) {
        gate.withLock {
            this.room = room
            roomChanged.signalAll()
        }
    }

    private fun deliver(message: WsMessage) {
        gate.withLock { waiting++ }
        inbox.trySend(Item.Message(message))
    }

    private fun finish(error: WsError) {
        end = error
        inbox.trySend(Item.End(error))
        inbox.close()
    }

    override fun onBinary(message: ByteArray) = deliver(WsMessage.Binary(message))

    override fun onText(message: String) = deliver(WsMessage.Text(message))

    // Only the text overload above is ever called: the client hands every text message to it.
    override fun onText() = Unit

    override fun onClose(code: Int, reason: String) = finish(WsError.Closed(code.toUShort(), reason))

    override fun onError(cause: Throwable) =
        finish(if (cause is WebSocketProtocolException) WsError.Protocol(describe(cause)) else WsError.Network(describe(cause)))

    // ---- the binding ----------------------------------------------------------------------------------------------

    override suspend fun send(message: WsMessage) {
        end?.let { throw it }
        val c = client ?: throw WsError.Network("the WebSocket is not connected")
        try {
            when (message) {
                is WsMessage.Text -> c.sendText(message.value)
                is WsMessage.Binary -> c.sendBinary(message.value)
            }
        } catch (e: IOException) {
            throw end ?: WsError.Network(describe(e))
        }
        // ADR-047 §4: a sender that awaits its sends cannot outrun the network.
        while (c.bufferedAmount > MAX_OUTBOUND_BYTES && c.isOpen) delay(SEND_POLL_MILLIS)
        if (!c.isOpen) end?.let { throw it }
    }

    /** Drops a connection nobody will use (its connect was cancelled): the reader stops waiting and the socket goes. */
    fun abandon(opened: WebSocketClient?) {
        stopping = true
        gate.withLock { roomChanged.signalAll() }
        inbox.close()
        opened?.abort()
    }

    override suspend fun close(code: Int, reason: String) {
        stopping = true
        gate.withLock { roomChanged.signalAll() }
        inbox.close()
        val c = client ?: return
        c.close(code, reason)
        // Returns once the server echoed the close (it has seen ours), or after the client's two-second grace.
        withContext(Dispatchers.IO) { runInterruptible { c.awaitFinished(CLOSE_WAIT_MILLIS) } }
    }

    private companion object {
        const val MAX_OUTBOUND_BYTES = 1L shl 20
        const val SEND_POLL_MILLIS = 2L
        const val CLOSE_WAIT_MILLIS = 3_000L
    }
}
