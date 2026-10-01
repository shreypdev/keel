package dev.undra.runtime

import dev.undra.runtime.wire.Envelope
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.PortStatus
import dev.undra.runtime.wire.WireException
import java.io.IOException
import java.net.URI
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.time.Duration

/** Waits between reconnect attempts; tests replace it with one that records the waits and does not wait. */
internal fun interface Sleeper {
    /** Waits [millis] milliseconds. @throws InterruptedException when the transport is closed meanwhile. */
    fun sleep(millis: Long)
}

/** The wall clock. */
internal val RealSleeper: Sleeper = Sleeper { Thread.sleep(it) }

/**
 * A core in another process (`undra dev`), reached over a WebSocket with the envelope framing of SPEC
 * section 3.2, on the runtime's own client ([WebSocketClient]: plain `java.net.Socket`, so it runs on Android
 * as well as on a JDK).
 *
 * **Development only.** Every operation is a message; [UndraCore] turns `callSync` and `construct` into a
 * blocking wait for the reply, which is fine against a server on `localhost` and wrong for an app you
 * ship. Snapshots and statistics have no message in the protocol and are unavailable.
 *
 * Attaching sends a `Hello` with the expected schema hash and waits for the server's; from then on every
 * envelope must carry that hash, and a server that restarts with another schema ends the connection with
 * an [UndraSchemaMismatchException].
 *
 * **Reconnecting** (ADR-034). With a [ReconnectPolicy], a connection that drops is not the end: the transport
 * tells its core ([TransportEvents.onReconnecting], and what was in flight fails), waits the policy's backoff
 * on a thread of its own, connects again and tells the core ([TransportEvents.onReconnected]), which observes
 * its stores again. The app closing the transport, a server with another schema, a session the server lost and
 * a protocol error are final ([TransportEvents.onClosed]). Every connection carries the same [session] token in
 * its URL (and `undra_resume=1` when the core holds objects), so `undra dev` can keep the objects of a client
 * that dropped and give them back to it.
 *
 * No thread that calls into this class does network I/O: connecting, reading and writing happen on threads of
 * the transport and the socket client, so an Android app may call in from its main thread.
 */
internal class RemoteTransport(
    private val uri: URI,
    private val timeout: Duration,
    private val reconnect: ReconnectPolicy? = null,
    private val session: String? = null,
    private val sleeper: Sleeper = RealSleeper,
    private val pingAfterMillis: Long = 10_000L,
) : Transport {
    override val mode: Mode get() = Mode.REMOTE
    override val isSynchronous: Boolean get() = false

    @Volatile
    private var events: TransportEvents? = null

    @Volatile
    private var expected: ULong = 0uL

    /** The connection that is up (handshake done); `null` while connecting, reconnecting or closed. */
    @Volatile
    private var current: Connection? = null

    /** The connection being set up, so that [close] can drop it. */
    @Volatile
    private var opening: Connection? = null

    @Volatile
    private var reconnectThread: Thread? = null
    private val closed = AtomicBoolean(false)

    /** Orders the decisions about which connection is current and what the core is told. */
    private val stateLock = Any()

    /** One WebSocket connection with its own handshake state, so that a late event of an old one cannot confuse a new one. */
    private inner class Connection : WebSocketClient.Listener {
        val hello = CompletableFuture<Payloads.Hello>()

        @Volatile
        var ws: WebSocketClient? = null

        @Volatile
        var handshakeDone = false

        /** Why the connection ended, once it did; guarded by [stateLock]. */
        var failure: Throwable? = null

        /** Guards [nextSeq] and the order of the queue: wire order equals sequence order. */
        val sendLock = Any()
        var nextSeq = 0

        override fun onBinary(message: ByteArray) = receive(this, message)

        override fun onText() = lost(this, ProtocolError("protocol error: the dev server sent a text frame; Undra speaks binary envelopes"))

        override fun onClose(code: Int, reason: String) = lost(this, closeCause(code, reason))

        override fun onError(cause: Throwable) = lost(this, wrap(cause))
    }

    /** A malformed or forbidden message from the server: retrying would meet the same bug. */
    private class ProtocolError(message: String, cause: Throwable? = null) : UndraException(message, cause)

    override fun connect(events: TransportEvents, expectedSchemaHash: ULong): ULong {
        this.events = events
        this.expected = expectedSchemaHash
        val millis = timeout.inWholeMilliseconds
        // The connect and the handshake run on a thread of ours: the caller may be an Android main thread.
        val outcome = CompletableFuture<Payloads.Hello>()
        Thread({
            try {
                outcome.complete(establish(resume = false, millis))
            } catch (e: Throwable) {
                outcome.completeExceptionally(e)
            }
        }, "undra-connect").also { it.isDaemon = true }.start()
        return try {
            outcome.get().schemaHash
        } catch (e: ExecutionException) {
            close()
            throw (e.cause as? UndraException) ?: UndraException("could not connect to the Undra dev server at $uri: ${e.cause?.message}", e.cause)
        } catch (e: InterruptedException) {
            close()
            Thread.currentThread().interrupt()
            throw UndraException("interrupted while connecting to the Undra dev server at $uri", e)
        }
    }

    /**
     * Opens a connection, exchanges `Hello` and makes it the current one; returns the server's `Hello`. Blocks for
     * about [millis]; run it on a thread of the transport's. A failure is an [UndraException] and leaves nothing open.
     * [checkSchema] refuses a server whose schema hash is not the expected one (a reconnect; `UndraCore.attach` does
     * it for the first connection).
     */
    private fun establish(resume: Boolean, millis: Long, checkSchema: Boolean = false): Payloads.Hello {
        val connection = Connection()
        opening = connection
        try {
            val ws = try {
                WebSocketClient.connect(urlFor(resume), connection, millis.toInt().coerceAtLeast(1), MAX_MESSAGE_BYTES, pingAfterMillis)
            } catch (e: IOException) {
                throw UndraException("could not connect to the Undra dev server at $uri: ${e.message}", e)
            }
            connection.ws = ws
            if (closed.get()) {
                ws.abort()
                throw UndraException("the connection to the Undra dev server at $uri was closed")
            }
            try {
                val hello = Payloads.Hello(UNDRA_RUNTIME_VERSION, expected, Platform.name, "dev").toByteArray()
                send(connection, Envelope.Kind.HELLO, hello)
                val theirs = connection.hello.get(millis, TimeUnit.MILLISECONDS)
                connection.handshakeDone = true
                synchronized(stateLock) {
                    // A close that arrived right behind the Hello (a refused session) is this attempt's failure,
                    // not a connection that came up and dropped.
                    connection.failure?.let { throw it as? UndraException ?: wrap(it) }
                    if (closed.get()) throw UndraException("the connection to the Undra dev server at $uri was closed")
                    if (checkSchema && theirs.schemaHash != expected) throw UndraSchemaMismatchException(expected, theirs.schemaHash)
                    current = connection
                }
                return theirs
            } catch (e: ExecutionException) {
                ws.abort()
                throw (e.cause as? UndraException) ?: UndraException("the Undra dev server at $uri did not complete the handshake: ${e.cause?.message}", e.cause)
            } catch (e: TimeoutException) {
                ws.abort()
                throw UndraException("the Undra dev server at $uri did not answer the handshake within $millis ms", e)
            } catch (e: InterruptedException) {
                ws.abort()
                Thread.currentThread().interrupt()
                throw UndraException("interrupted during the handshake with $uri", e)
            } catch (e: UndraException) {
                ws.abort()
                throw e
            }
        } finally {
            opening = null
        }
    }

    /** The URL of one connection: the server's, with the session token and, when objects are to be found again, the resume flag. */
    private fun urlFor(resume: Boolean): URI {
        val token = session ?: return uri
        val query = listOfNotNull(uri.rawQuery, "undra_session=$token", if (resume) "undra_resume=1" else null).joinToString("&")
        val path = uri.rawPath?.takeIf { it.isNotEmpty() } ?: "/"
        return URI("${uri.scheme}://${uri.rawAuthority}$path?$query")
    }

    override fun call(payload: ByteArray): Int {
        send(Envelope.Kind.CALL, payload)
        return 0
    }

    override fun callSync(payload: ByteArray): ByteArray =
        throw UndraModeException("a remote core has no synchronous call; UndraCore waits for the reply instead")

    override fun cancel(callId: UInt) = send(Envelope.Kind.CANCEL, Payloads.Cancel(callId).toByteArray())

    override fun streamCredit(callId: UInt, credit: UInt) =
        send(Envelope.Kind.STREAM_CREDIT, Payloads.StreamCredit(callId, credit).toByteArray())

    override fun observe(handle: Long, signalId: UInt, on: Boolean) =
        send(Envelope.Kind.OBSERVE, Payloads.Observe(dev.undra.runtime.wire.Handle(handle), signalId, on).toByteArray())

    override fun release(handle: Long) = send(Envelope.Kind.RELEASE, Payloads.Release(dev.undra.runtime.wire.Handle(handle)).toByteArray())

    override fun portReply(payload: ByteArray) = send(Envelope.Kind.PORT_REPLY, payload)

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) =
        send(Envelope.Kind.EVENT, Payloads.Event(portId, methodId, payload).toByteArray())

    override fun timerFired(timerId: UInt) = send(Envelope.Kind.TIMER_FIRED, Payloads.TimerFired(timerId).toByteArray())

    override fun snapshot(): ByteArray = throw UndraModeException("snapshots are only available for an in-process core (Mode.INPROC)")

    override fun restore(snapshot: ByteArray): Int = throw UndraModeException("snapshots are only available for an in-process core (Mode.INPROC)")

    override fun statsJson(): String? = null

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        reconnectThread?.interrupt()
        val up = synchronized(stateLock) { current.also { current = null } }
        up?.ws?.close()
        up?.hello?.completeExceptionally(UndraException("the connection to $uri was closed"))
        opening?.let {
            it.ws?.abort()
            it.hello.completeExceptionally(UndraException("the connection to $uri was closed"))
        }
    }

    // ---- sending -------------------------------------------------------------------------------------

    private fun send(kind: Envelope.Kind, payload: ByteArray) {
        val connection = current
            ?: throw UndraException(
                when {
                    closed.get() -> "the connection to the Undra dev server at $uri is closed"
                    reconnect != null -> "not connected to the Undra dev server at $uri: reconnecting"
                    else -> "not connected to the Undra dev server"
                },
            )
        send(connection, kind, payload)
    }

    private fun send(connection: Connection, kind: Envelope.Kind, payload: ByteArray) {
        val ws = connection.ws ?: throw UndraException("not connected to the Undra dev server")
        if (closed.get() || !ws.isOpen) throw UndraException("the connection to the Undra dev server at $uri is closed")
        synchronized(connection.sendLock) {
            try {
                ws.sendBinary(Envelope.encode(kind, (connection.nextSeq++).toUInt(), expected, payload))
            } catch (e: IOException) {
                throw UndraException("the connection to the Undra dev server at $uri is closed", e)
            }
        }
    }

    // ---- receiving -----------------------------------------------------------------------------------

    private fun receive(connection: Connection, message: ByteArray) {
        val envelope = try {
            Envelope.decode(message, if (connection.handshakeDone) expected else null)
        } catch (e: WireException.SchemaMismatch) {
            lost(connection, UndraSchemaMismatchException(e.expected, e.got))
            return
        } catch (e: WireException) {
            lost(connection, ProtocolError("protocol error: the dev server sent a malformed envelope: ${e.message}", e))
            return
        }
        try {
            dispatch(connection, envelope)
        } catch (e: WireException) {
            lost(connection, ProtocolError("protocol error: malformed ${envelope.kind} payload: ${e.message}", e))
        } catch (e: UndraException) {
            lost(connection, e)
        } catch (e: RuntimeException) {
            UndraLog.warn("handling a ${envelope.kind} message from the dev server failed", e)
        }
    }

    private fun dispatch(connection: Connection, envelope: Envelope) {
        if (envelope.kind == Envelope.Kind.HELLO) {
            connection.hello.complete(Payloads.Hello.decode(envelope.payload))
            return
        }
        if (!connection.handshakeDone || connection !== current) return
        val target = events ?: return
        when (envelope.kind) {
            Envelope.Kind.REPLY -> Payloads.Reply.decode(envelope.payload).let { target.onReply(it.callId, it.status, it.body) }
            Envelope.Kind.STREAM_ITEM -> Payloads.StreamItem.decode(envelope.payload).let { target.onStreamItem(it.callId, it.flag, it.body) }
            Envelope.Kind.CHANGE_SET -> target.onChangeSet(envelope.payload)
            Envelope.Kind.PORT_CALL -> {
                val call = Payloads.PortCall.decode(envelope.payload)
                when (val outcome = target.onPortCall(call.portId, call.methodId, call.portCallId, call.args)) {
                    is PortOutcome.Sync -> send(connection, Envelope.Kind.PORT_REPLY, outcome.reply)
                    PortOutcome.Async -> Unit
                    PortOutcome.Unavailable ->
                        send(connection, Envelope.Kind.PORT_REPLY, Payloads.PortReply(call.portCallId, PortStatus.UNAVAILABLE, ByteArray(0)).toByteArray())
                }
            }
            Envelope.Kind.LOG -> Payloads.Log.decode(envelope.payload).let { target.onLog(it.level, it.target, it.message) }
            Envelope.Kind.SNAPSHOT -> UndraLog.debug("ignoring an unsolicited snapshot from the dev server")
            else -> UndraLog.warn("the dev server sent a ${envelope.kind} message, which only a host may send; ignoring it")
        }
    }

    // ---- losing and regaining the connection ---------------------------------------------------------

    private fun closeCause(code: Int, reason: String): UndraException =
        if (code == SESSION_LOST) {
            UndraSessionLostException(reason.ifEmpty { "the dev server no longer has this core's objects" })
        } else {
            UndraException("the Undra dev server closed the connection ($code ${reason.ifEmpty { "no reason" }})")
        }

    private fun wrap(cause: Throwable): Throwable =
        cause as? UndraException ?: UndraException("the connection to the Undra dev server at $uri failed: ${cause.message}", cause)

    /** [connection] ended. Before its handshake that fails the attempt; afterwards it is a loss to reconnect from (or to end on). */
    private fun lost(connection: Connection, cause: Throwable) {
        synchronized(stateLock) {
            if (connection.failure != null) return
            connection.failure = cause
            connection.hello.completeExceptionally(cause)
            if (connection !== current) return // still being set up: the attempt sees `failure`
            current = null
            connection.ws?.abort()
            if (closed.get()) return
            if (reconnect != null && retryable(cause)) {
                startReconnecting(cause)
            } else {
                giveUp(cause)
            }
        }
    }

    private fun retryable(cause: Throwable): Boolean =
        cause !is ProtocolError && cause !is UndraSchemaMismatchException && cause !is UndraSessionLostException

    /** Ends the transport for good and tells the core. Called with [stateLock] held. */
    private fun giveUp(cause: Throwable) {
        if (!closed.compareAndSet(false, true)) return
        events?.onClosed(cause as? UndraException ?: UndraException("the connection to the Undra dev server at $uri failed: ${cause.message}", cause))
    }

    private fun startReconnecting(cause: Throwable) {
        val policy = reconnect ?: return
        events?.onReconnecting(1, cause)
        reconnectThread = Thread({ reconnectLoop(policy, cause) }, "undra-reconnect").also {
            it.isDaemon = true
            it.start()
        }
    }

    private fun reconnectLoop(policy: ReconnectPolicy, firstCause: Throwable) {
        var attempt = 1
        var cause = firstCause
        val patience = minOf(timeout.inWholeMilliseconds, RECONNECT_ATTEMPT_CAP_MILLIS).coerceAtLeast(1)
        while (!closed.get()) {
            try {
                sleeper.sleep(policy.delayFor(attempt).inWholeMilliseconds)
            } catch (e: InterruptedException) {
                return
            }
            if (closed.get()) return
            val failure: Throwable? = try {
                establish(resume = events?.holdsObjects() == true, patience, checkSchema = true)
                null
            } catch (e: UndraException) {
                e
            }
            if (closed.get()) return
            if (failure == null) {
                synchronized(stateLock) {
                    // Lost again before the core heard of it: that loss started its own reconnecting.
                    if (current != null) events?.onReconnected()
                }
                return
            }
            if (!retryable(failure) || attempt >= policy.maxAttempts) {
                synchronized(stateLock) { giveUp(failure) }
                return
            }
            attempt++
            cause = failure
            synchronized(stateLock) { if (!closed.get()) events?.onReconnecting(attempt, cause) }
        }
    }

    private companion object {
        /** Largest message accepted from the server; a change-set or reply beyond this is a protocol violation. */
        const val MAX_MESSAGE_BYTES: Int = 64 * 1024 * 1024

        /** The close code with which the dev server says it no longer holds this client's session (ADR-034). */
        const val SESSION_LOST: Int = 4001

        /** A reconnect attempt (connect and `Hello`) takes at most this long, whatever the blocking timeout is. */
        const val RECONNECT_ATTEMPT_CAP_MILLIS: Long = 5_000L
    }
}
