package dev.keel.runtime

import dev.keel.runtime.wire.Envelope
import dev.keel.runtime.wire.Payloads
import dev.keel.runtime.wire.Payloads.PortStatus
import dev.keel.runtime.wire.WireException
import java.io.ByteArrayOutputStream
import java.net.URI
import java.net.http.HttpClient
import java.net.http.WebSocket
import java.nio.ByteBuffer
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CompletionStage
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.time.Duration

/**
 * A core in another process (`keel dev`), reached over a WebSocket with the envelope framing of SPEC
 * section 3.2 (`java.net.http.WebSocket`, so JDK 11 or later; not available on Android).
 *
 * **Development only.** Every operation is a message; [KeelCore] turns `callSync` and `construct` into a
 * blocking wait for the reply, which is fine against a server on `localhost` and wrong for an app you
 * ship. Snapshots and statistics have no message in the protocol and are unavailable.
 *
 * Attaching sends a `Hello` with the expected schema hash and waits for the server's; from then on every
 * envelope must carry that hash, and a server that restarts with another schema ends the connection with
 * a [KeelSchemaMismatchException]. Outgoing messages are sent one at a time in order (the JDK's WebSocket
 * allows a single send in flight).
 */
internal class RemoteTransport(
    private val uri: URI,
    private val timeout: Duration,
) : Transport {
    override val mode: Mode get() = Mode.REMOTE
    override val isSynchronous: Boolean get() = false

    @Volatile
    private var events: TransportEvents? = null

    @Volatile
    private var webSocket: WebSocket? = null

    @Volatile
    private var schemaHash: ULong = 0uL

    @Volatile
    private var handshakeDone = false
    private val hello = CompletableFuture<Payloads.Hello>()
    private val closed = AtomicBoolean(false)
    private val sendLock = Any()
    private var sendTail: CompletableFuture<*> = CompletableFuture.completedFuture(null)
    private var nextSeq = 0

    override fun connect(events: TransportEvents, expectedSchemaHash: ULong): ULong {
        this.events = events
        this.schemaHash = expectedSchemaHash
        val millis = timeout.inWholeMilliseconds
        val ws = try {
            HttpClient.newBuilder()
                .version(HttpClient.Version.HTTP_1_1)
                .connectTimeout(java.time.Duration.ofMillis(millis))
                .build()
                .newWebSocketBuilder()
                .connectTimeout(java.time.Duration.ofMillis(millis))
                .buildAsync(uri, Listener())
                .get(millis, TimeUnit.MILLISECONDS)
        } catch (e: ExecutionException) {
            throw KeelException("could not connect to the Keel dev server at $uri: ${e.cause?.message ?: e.message}", e.cause ?: e)
        } catch (e: TimeoutException) {
            throw KeelException("timed out connecting to the Keel dev server at $uri", e)
        } catch (e: InterruptedException) {
            Thread.currentThread().interrupt()
            throw KeelException("interrupted while connecting to the Keel dev server at $uri", e)
        }
        webSocket = ws
        val theirs = try {
            send(Envelope.Kind.HELLO, Payloads.Hello(KEEL_RUNTIME_VERSION, expectedSchemaHash, Platform.name, "dev").toByteArray())
            hello.get(millis, TimeUnit.MILLISECONDS)
        } catch (e: ExecutionException) {
            close()
            throw (e.cause as? KeelException) ?: KeelException("the Keel dev server at $uri did not complete the handshake: ${e.cause?.message}", e.cause)
        } catch (e: TimeoutException) {
            close()
            throw KeelException("the Keel dev server at $uri did not answer the handshake within $timeout", e)
        } catch (e: InterruptedException) {
            close()
            Thread.currentThread().interrupt()
            throw KeelException("interrupted during the handshake with $uri", e)
        }
        handshakeDone = true
        return theirs.schemaHash
    }

    override fun call(payload: ByteArray): Int {
        send(Envelope.Kind.CALL, payload)
        return 0
    }

    override fun callSync(payload: ByteArray): ByteArray =
        throw KeelModeException("a remote core has no synchronous call; KeelCore waits for the reply instead")

    override fun cancel(callId: UInt) = send(Envelope.Kind.CANCEL, Payloads.Cancel(callId).toByteArray())

    override fun streamCredit(callId: UInt, credit: UInt) =
        send(Envelope.Kind.STREAM_CREDIT, Payloads.StreamCredit(callId, credit).toByteArray())

    override fun observe(handle: Long, signalId: UInt, on: Boolean) =
        send(Envelope.Kind.OBSERVE, Payloads.Observe(dev.keel.runtime.wire.Handle(handle), signalId, on).toByteArray())

    override fun release(handle: Long) = send(Envelope.Kind.RELEASE, Payloads.Release(dev.keel.runtime.wire.Handle(handle)).toByteArray())

    override fun portReply(payload: ByteArray) = send(Envelope.Kind.PORT_REPLY, payload)

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) =
        send(Envelope.Kind.EVENT, Payloads.Event(portId, methodId, payload).toByteArray())

    override fun timerFired(timerId: UInt) = send(Envelope.Kind.TIMER_FIRED, Payloads.TimerFired(timerId).toByteArray())

    override fun snapshot(): ByteArray = throw KeelModeException("snapshots are only available for an in-process core (Mode.INPROC)")

    override fun restore(snapshot: ByteArray): Int = throw KeelModeException("snapshots are only available for an in-process core (Mode.INPROC)")

    override fun statsJson(): String? = null

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        abortConnection()
        hello.completeExceptionally(KeelException("the connection to $uri was closed"))
    }

    // ---- sending -------------------------------------------------------------------------------------

    private fun send(kind: Envelope.Kind, payload: ByteArray) {
        val ws = webSocket ?: throw KeelException("not connected to the Keel dev server")
        if (closed.get() || ws.isOutputClosed) throw KeelException("the connection to the Keel dev server at $uri is closed")
        synchronized(sendLock) {
            val frame = ByteBuffer.wrap(Envelope.encode(kind, (nextSeq++).toUInt(), schemaHash, payload))
            val previous = sendTail
            val sent: CompletableFuture<*> = if (previous.isDone) {
                ws.sendBinary(frame, true)
            } else {
                previous.handle { _, _ -> null }.thenCompose { ws.sendBinary(frame, true) }
            }
            sent.whenComplete { _, failure -> if (failure != null) connectionLost(failure.cause ?: failure) }
            sendTail = sent
        }
    }

    // ---- receiving -----------------------------------------------------------------------------------

    private fun connectionLost(cause: Throwable?) {
        val target = events
        if (closed.compareAndSet(false, true)) {
            abortConnection()
            hello.completeExceptionally(cause ?: KeelException("the connection to $uri was closed"))
            target?.onClosed(cause?.let { it as? KeelException ?: KeelException("the connection to the Keel dev server at $uri failed: ${it.message}", it) })
        }
    }

    private fun abortConnection() {
        val ws = webSocket ?: return
        try {
            ws.sendClose(WebSocket.NORMAL_CLOSURE, "").whenComplete { _, _ -> ws.abort() }
        } catch (e: Exception) {
            ws.abort()
        }
    }

    private fun receive(message: ByteArray) {
        val envelope = try {
            Envelope.decode(message, if (handshakeDone) schemaHash else null)
        } catch (e: WireException.SchemaMismatch) {
            connectionLost(KeelSchemaMismatchException(e.expected, e.got))
            return
        } catch (e: WireException) {
            connectionLost(KeelException("protocol error: the dev server sent a malformed envelope: ${e.message}", e))
            return
        }
        try {
            dispatch(envelope)
        } catch (e: WireException) {
            connectionLost(KeelException("protocol error: malformed ${envelope.kind} payload: ${e.message}", e))
        } catch (e: KeelException) {
            connectionLost(e)
        } catch (e: RuntimeException) {
            KeelLog.warn("handling a ${envelope.kind} message from the dev server failed", e)
        }
    }

    private fun dispatch(envelope: Envelope) {
        if (envelope.kind == Envelope.Kind.HELLO) {
            hello.complete(Payloads.Hello.decode(envelope.payload))
            return
        }
        val target = events ?: return
        when (envelope.kind) {
            Envelope.Kind.REPLY -> Payloads.Reply.decode(envelope.payload).let { target.onReply(it.callId, it.status, it.body) }
            Envelope.Kind.STREAM_ITEM -> Payloads.StreamItem.decode(envelope.payload).let { target.onStreamItem(it.callId, it.flag, it.body) }
            Envelope.Kind.CHANGE_SET -> target.onChangeSet(envelope.payload)
            Envelope.Kind.PORT_CALL -> {
                val call = Payloads.PortCall.decode(envelope.payload)
                when (val outcome = target.onPortCall(call.portId, call.methodId, call.portCallId, call.args)) {
                    is PortOutcome.Sync -> send(Envelope.Kind.PORT_REPLY, outcome.reply)
                    PortOutcome.Async -> Unit
                    PortOutcome.Unavailable ->
                        send(Envelope.Kind.PORT_REPLY, Payloads.PortReply(call.portCallId, PortStatus.UNAVAILABLE, ByteArray(0)).toByteArray())
                }
            }
            Envelope.Kind.LOG -> Payloads.Log.decode(envelope.payload).let { target.onLog(it.level, it.target, it.message) }
            Envelope.Kind.SNAPSHOT -> KeelLog.debug("ignoring an unsolicited snapshot from the dev server")
            else -> KeelLog.warn("the dev server sent a ${envelope.kind} message, which only a host may send; ignoring it")
        }
    }

    private inner class Listener : WebSocket.Listener {
        private val partial = ByteArrayOutputStream()

        override fun onOpen(webSocket: WebSocket) {
            webSocket.request(1)
        }

        override fun onBinary(webSocket: WebSocket, data: ByteBuffer, last: Boolean): CompletionStage<*>? {
            val chunk = ByteArray(data.remaining())
            data.get(chunk)
            if (partial.size() + chunk.size > MAX_MESSAGE_BYTES) {
                partial.reset()
                connectionLost(KeelException("protocol error: the dev server sent a message larger than $MAX_MESSAGE_BYTES bytes"))
                return null
            }
            partial.write(chunk, 0, chunk.size)
            if (last) {
                val message = partial.toByteArray()
                partial.reset()
                receive(message)
            }
            webSocket.request(1)
            return null
        }

        override fun onText(webSocket: WebSocket, data: CharSequence, last: Boolean): CompletionStage<*>? {
            connectionLost(KeelException("protocol error: the dev server sent a text frame; Keel speaks binary envelopes"))
            return null
        }

        override fun onClose(webSocket: WebSocket, statusCode: Int, reason: String): CompletionStage<*>? {
            connectionLost(KeelException("the Keel dev server closed the connection ($statusCode ${reason.ifEmpty { "no reason" }})"))
            return null
        }

        override fun onError(webSocket: WebSocket, error: Throwable) {
            connectionLost(error)
        }
    }

    private companion object {
        /** Largest message accepted from the server; a change-set or reply beyond this is a protocol violation. */
        const val MAX_MESSAGE_BYTES: Int = 64 * 1024 * 1024
    }
}
