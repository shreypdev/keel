package dev.undra.runtime

import dev.undra.runtime.adapters.JulLog
import dev.undra.runtime.adapters.JvmAdapters
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.Payloads.StreamFlag
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.decodeAll
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.coroutines.ContinuationInterceptor
import kotlin.time.Duration
import kotlinx.coroutines.CancellableContinuation
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.suspendCancellableCoroutine

/**
 * The [UndraCore] that [UndraCore.load] returns: call ids, the table of in-flight calls, streams, the
 * port registry and the mirror on top of one [Transport].
 *
 * Invariants:
 *  - a core callback ([TransportEvents]) only copies, queues or completes a future; application code
 *    runs on other threads (see [resumeSafely] and [UndraDispatchers.delivery]);
 *  - a call is in [pending] before it is sent, because an in-process core may reply on the sending thread
 *    before the send returns;
 *  - whoever removes a call from [pending] owns completing it;
 *  - read-your-writes (ADR-031): the change-sets that arrived before a reply are applied before a caller on
 *    the main thread continues, whether it suspended ([call]) or blocked ([callSync], [construct]).
 */
internal class ConnectedCore(
    private val transport: Transport,
    private val blockingTimeout: Duration,
    initialCallId: Int = 0,
    mirrorOptions: MirrorOptions = MirrorOptions(),
    main: MainThread = UndraDispatchers.mainThread(),
    private val onConnectionChange: ((ConnectionState) -> Unit)? = null,
    private val onError: ((UndraUnhandledError) -> Unit)? = null,
) : UndraCore(), TransportEvents {

    private sealed interface Pending {
        class Suspended(val continuation: CancellableContinuation<ByteArray>) : Pending
        class Blocking(val future: CompletableFuture<Payloads.Reply>) : Pending
        class Streaming(val stream: StreamState) : Pending
    }

    /** What the collector of a stream waits on. Filled from [UndraDispatchers.delivery]. */
    private class StreamState {
        /** Bounded by the credit protocol (at most 16 granted items are ever buffered), so unlimited is safe. */
        val items = Channel<ByteArray>(Channel.UNLIMITED)
        val opened = CompletableDeferred<Unit>()

        @Volatile
        var coreDone = false
    }

    private val callIds = AtomicInteger(initialCallId)
    private val pending = ConcurrentHashMap<Int, Pending>()
    private val closed = AtomicBoolean(false)

    private val closeLock = Any()

    @Volatile
    private var closeCause: Throwable? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + CoroutineName("undra-ports"))
    private val ports = PortRegistry(scope, ::reportFromCore) { payload ->
        if (!closed.get()) {
            try {
                transport.portReply(payload)
            } catch (e: Exception) {
                reportFromCore(e, "port reply")
            }
        }
    }
    private val liveMirror = Mirror(main, mirrorOptions, ::resync, ::report)

    // What a reconnect needs to put the core back where the app left it (ADR-051).

    /** The signals the app observes, per store: observed again after a reconnect. */
    private val observedLock = Any()
    private val observed = HashMap<Long, MutableSet<UInt>>()

    /** The objects the app's constructors made and it has not released: what the server is asked to keep for it. */
    private val constructed = ConcurrentHashMap.newKeySet<Long>()

    /** Handles released while the connection was down: released at the server once it is back. */
    private val releasedWhileDown = ConcurrentHashMap.newKeySet<Long>()

    /** Counts the times the connection was lost, so that a replay that a newer loss overtook does not announce a connection. */
    private val lossEpoch = AtomicInteger(0)
    private val connection = MutableStateFlow<ConnectionState>(ConnectionState.Connecting)

    init {
        notifyConnectionChange(ConnectionState.Connecting)
    }

    override val mode: Mode get() = transport.mode

    override val connectionState: StateFlow<ConnectionState> get() = connection

    override val mirror: Mirror get() = liveMirror

    // ---- setup -------------------------------------------------------------------------------------------

    /** Registers the ports of [options]: the defaults for what is missing, then the explicit adapters. */
    fun installPorts(options: LoadOptions) {
        if (options.defaultAdapters) {
            for ((id, impl) in JvmAdapters.defaults(this::timerFired)) ports.register(id, impl)
        }
        for ((id, impl) in options.adapters) ports.register(id, impl)
    }

    /** Tears down a core whose start failed. */
    fun abandon() {
        shutDown(null, ClosedReason.FAILED)
    }

    /** The handshake is done and the schema checked: the core is reachable. */
    fun markConnected() {
        setConnection(ConnectionState.Connected)
    }

    private fun setConnection(state: ConnectionState) {
        connection.value = state
        notifyConnectionChange(state)
    }

    private fun notifyConnectionChange(state: ConnectionState) {
        try {
            onConnectionChange?.invoke(state)
        } catch (e: Exception) {
            UndraLog.warn("LoadOptions.onConnectionChange failed", e)
        }
    }

    // ---- calls -------------------------------------------------------------------------------------------

    override fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        ensureOpen()
        val callId = nextCallId()
        val payload = encodeCall(target, methodId, callId, args)
        val reply = if (transport.isSynchronous) {
            val bytes = transport.callSync(payload)
            // The call's change-sets are queued by now: on the main thread they are applied before it returns.
            liveMirror.drainIfOnMainThread()
            try {
                Payloads.Reply.decode(bytes)
            } catch (e: WireException) {
                throw UndraProtocolException("the core sent a malformed reply: ${e.message}", e)
            }
        } else {
            blockingCall(callId, payload).also { liveMirror.drainIfOnMainThread() }
        }
        if (reply.callId != callId) throw UndraProtocolException("protocol error: the reply is for call ${reply.callId}, not $callId")
        return replyBody(reply.status, reply.body)
    }

    override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        ensureOpen()
        val callId = nextCallId()
        val payload = encodeCall(target, methodId, callId, args)
        try {
            return suspendCancellableCoroutine { continuation ->
                val entry = Pending.Suspended(continuation)
                pending[callId.toInt()] = entry
                continuation.invokeOnCancellation {
                    if (pending.remove(callId.toInt(), entry)) cancelQuietly(callId)
                }
                try {
                    submit(callId, payload)
                } catch (e: Throwable) {
                    if (pending.remove(callId.toInt(), entry)) continuation.resumeWith(Result.failure(e))
                }
            }
        } finally {
            // The reply posted a drain ahead of this continuation (onReply); a dispatcher that does not
            // run in posting order on the main thread (Compose's frame-driven one) is covered here.
            liveMirror.drainIfOnMainThread()
        }
    }

    override fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray> = flow {
        ensureOpen()
        val callId = nextCallId()
        val payload = encodeCall(target, methodId, callId, args)
        val state = StreamState()
        pending[callId.toInt()] = Pending.Streaming(state)
        try {
            submit(callId, payload)
            state.opened.await()
            // Credit only makes sense once the core knows the stream, i.e. after it said so.
            transport.streamCredit(callId, INITIAL_CREDIT)
            var outstanding = INITIAL_CREDIT
            while (true) {
                val received = state.items.receiveCatching()
                val item = received.getOrNull()
                if (item == null) {
                    received.exceptionOrNull()?.let { throw it }
                    break // the core ended the stream
                }
                emit(item)
                outstanding--
                if (outstanding < TOP_UP_BELOW) {
                    transport.streamCredit(callId, TOP_UP)
                    outstanding += TOP_UP
                }
            }
        } finally {
            pending.remove(callId.toInt())
            if (!state.coreDone) cancelQuietly(callId)
        }
    }

    override fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long {
        val body = callSync(CallTarget.Constructor(typeId, methodId), methodId, args)
        val handle = try {
            Codecs.handle.decodeAll(body)
        } catch (e: WireException) {
            throw UndraProtocolException("the core returned a malformed handle: ${e.message}", e)
        }
        if (handle == 0L) throw UndraProtocolException("the core returned the null handle for a constructor")
        constructed.add(handle)
        return handle
    }

    private fun blockingCall(callId: UInt, payload: ByteArray): Payloads.Reply {
        val future = CompletableFuture<Payloads.Reply>()
        val entry = Pending.Blocking(future)
        pending[callId.toInt()] = entry
        try {
            submit(callId, payload)
            return future.get(blockingTimeout.inWholeMilliseconds, TimeUnit.MILLISECONDS)
        } catch (e: TimeoutException) {
            if (pending.remove(callId.toInt(), entry)) cancelQuietly(callId)
            throw UndraTransportException(UndraTransportException.Reason.TIMEOUT, "the remote core did not answer within $blockingTimeout", e)
        } catch (e: ExecutionException) {
            throw (e.cause as? UndraException)
                ?: UndraTransportException(UndraTransportException.Reason.CONNECTION_LOST, "the call failed: ${e.cause?.message}", e.cause)
        } catch (e: InterruptedException) {
            if (pending.remove(callId.toInt(), entry)) cancelQuietly(callId)
            Thread.currentThread().interrupt()
            throw UndraTransportException(UndraTransportException.Reason.INTERRUPTED, "interrupted while waiting for the remote core", e)
        } catch (e: UndraException) {
            pending.remove(callId.toInt(), entry)
            throw e
        }
    }

    /** Hands a call to the transport; a rejected payload fails the call like a `bad request` reply would. */
    private fun submit(callId: UInt, payload: ByteArray) {
        val code = transport.call(payload)
        if (code != 0) {
            val entry = pending.remove(callId.toInt()) ?: return // a reply already answered it
            fail(entry, UndraReplyException(ReplyStatus.BAD_REQUEST, badRequestBody("the core rejected the call payload (code $code)")))
        }
    }

    private fun nextCallId(): UInt {
        while (true) {
            val id = callIds.incrementAndGet()
            if (id != 0 && !pending.containsKey(id)) return id.toUInt()
        }
    }

    private fun cancelQuietly(callId: UInt) {
        if (closed.get()) return
        try {
            transport.cancel(callId)
        } catch (e: Exception) {
            UndraLog.warn("cancelling call $callId failed", e)
        }
    }

    /** What every call on a closed core fails with: the host closed it, or the connection to it was lost. */
    private fun closedException(cause: Throwable?): UndraTransportException =
        if (cause == null) {
            UndraTransportException(UndraTransportException.Reason.CLOSED, "this UndraCore is closed")
        } else {
            UndraTransportException(UndraTransportException.Reason.CONNECTION_LOST, "this UndraCore is closed: ${cause.message}", cause)
        }

    private fun ensureOpen() {
        if (closed.get()) throw closedException(closeCause)
    }

    // ---- objects, stores, ports ----------------------------------------------------------------------

    override fun observe(handle: Long, signalId: UInt, on: Boolean) {
        ensureOpen()
        transport.observe(handle, signalId, on)
        noteObserved(handle, signalId, on)
        if (on && transport.isSynchronous && !liveMirror.awaitApplied(OBSERVE_APPLY_TIMEOUT_MILLIS)) {
            UndraLog.warn(
                "the initial values of ${Handle(handle)} were not applied within " +
                    "${OBSERVE_APPLY_TIMEOUT_MILLIS}ms (is the main thread blocked?); they will follow when it catches up",
            )
        }
    }

    override fun release(handle: Long) {
        liveMirror.unregister(handle)
        constructed.remove(handle)
        synchronized(observedLock) { observed.remove(handle) }
        if (closed.get()) return
        if (connection.value is ConnectionState.Reconnecting) {
            // The server keeps the object for us (ADR-051); it is released when the connection is back.
            releasedWhileDown.add(handle)
            return
        }
        try {
            transport.release(handle)
        } catch (e: UndraException) {
            if (connection.value is ConnectionState.Reconnecting) releasedWhileDown.add(handle) else throw e
        }
    }

    /** Remembers what the app observes, so that a reconnect can observe it again. */
    private fun noteObserved(handle: Long, signalId: UInt, on: Boolean) {
        synchronized(observedLock) {
            if (on) {
                observed.getOrPut(handle) { HashSet() }.add(signalId)
            } else if (signalId == UInt.MAX_VALUE) {
                observed.remove(handle)
            } else {
                observed[handle]?.let {
                    it.remove(signalId)
                    if (it.isEmpty()) observed.remove(handle)
                }
            }
        }
    }

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) {
        ensureOpen()
        transport.event(portId, methodId, payload)
    }

    override fun timerFired(timerId: UInt) {
        // A timer that comes due after close() has nobody to tell; timer adapters race with close, so this is not an error.
        if (closed.get()) return
        transport.timerFired(timerId)
    }

    override fun registerPort(portId: UInt, impl: PortImpl) {
        ports.register(portId, impl)
    }

    // ---- reporting (ADR-032, amendment A) -----------------------------------------------------------

    /** True while `onError` runs on this thread, so a handler that makes a failing call is only logged, never reported again. */
    private val reporting: ThreadLocal<Boolean> = ThreadLocal.withInitial { false }

    override fun report(error: Throwable, operation: String) {
        val unhandled = UndraUnhandledError(operation, UndraCallError.asCallError(error))
        if (isLostConnection(unhandled.error)) {
            // The connection state says it (Reconnecting, or Closed with the cause) and `onConnectionChange` heard it once:
            // a command tapped while `undra dev` is away is not a second failure to hand to the crash reporter.
            UndraLog.warn("${unhandled.message} (the connection to the core was lost: see connectionState)")
            return
        }
        UndraLog.error(unhandled.message.orEmpty(), error)
        val handler = onError ?: return
        if (reporting.get()) return
        reporting.set(true)
        try {
            handler(unhandled)
        } catch (e: Exception) {
            UndraLog.warn("the onError handler threw while handling \"${unhandled.message}\"", e)
        } finally {
            reporting.set(false)
        }
    }

    /**
     * Whether [error] is the connection to the core being down, which [connectionState] already reports: every call on a
     * [Mode.REMOTE] core that is [ConnectionState.Reconnecting] or [ConnectionState.Closed] after a failure fails this way.
     * A core the app closed itself ([UndraTransportException.Reason.CLOSED]) and a call that timed out are not.
     */
    private fun isLostConnection(error: UndraCallError): Boolean =
        error is UndraCallError.Unavailable && error.transport.reason == UndraTransportException.Reason.CONNECTION_LOST

    /**
     * [report] for a failure found on a thread the core may be holding its lock on (a core callback: a malformed
     * change-set, a failed port) or by the mirror: the handler runs on the delivery thread, never inside the callback.
     */
    private fun reportFromCore(error: Throwable, operation: String) {
        UndraDispatchers.delivery.execute { report(error, operation) }
    }

    override fun stats(): UndraStats {
        val hostPending = pending.size
        val mirrored = liveMirror.registeredCount
        val mirrorStats = liveMirror.stats()
        val json = if (closed.get()) null else transport.statsJson()
        return if (json == null) {
            UndraStats(liveHandles = UndraStats.UNKNOWN, hostPendingCalls = hostPending, hostMirrorHandles = mirrored, mirror = mirrorStats)
        } else {
            UndraStats.fromCoreJson(json, hostPending, mirrored, mirrorStats)
        }
    }

    /**
     * Asks the core for the current value of a signal whose merged patch the mirror dropped (ADR-031): the
     * value arrives as a change-set (in process, before this returns). Called by the mirror on the main
     * thread, never from a core callback.
     */
    private fun resync(handle: Long, signalId: UInt) {
        if (closed.get()) return
        try {
            transport.observe(handle, signalId, true)
        } catch (e: Exception) {
            UndraLog.warn("re-observing signal $signalId of ${Handle(handle)} failed", e)
        }
    }

    override fun snapshot(): ByteArray {
        ensureOpen()
        return transport.snapshot()
    }

    override fun restore(snapshot: ByteArray) {
        ensureOpen()
        val code = transport.restore(snapshot)
        // Like any synchronous call made on the main thread, the restored values are applied before it returns.
        liveMirror.drainIfOnMainThread()
        if (code != 0) throw UndraRestoreException(code)
    }

    override fun close() {
        // A close the transport refuses (from inside a core callback) must leave the core as it was.
        if (!closed.get()) transport.checkClose()
        shutDown(null, ClosedReason.REQUESTED)
    }

    /**
     * Fails everything pending, cancels the scope and closes the transport, in that order. For an in-process
     * core the last step blocks until the native shutdown has finished (see [UndraCore.close]), which is why
     * the pending calls are failed first: the shutdown's own answers have nowhere to go.
     */
    private fun shutDown(cause: Throwable?, reason: ClosedReason) {
        // The cause must be visible before `closed` is, or a caller that sees the closed flag reports no cause.
        synchronized(closeLock) {
            if (closed.get()) return
            closeCause = cause
            closed.set(true)
        }
        UndraCore.forget(this)
        setConnection(ConnectionState.Closed(reason, cause))
        failAll(closedException(cause))
        scope.cancel()
        try {
            transport.close()
        } catch (e: Exception) {
            UndraLog.warn("closing the transport failed", e)
        }
    }

    // ---- completing calls ----------------------------------------------------------------------------

    private fun replyBody(status: ReplyStatus, body: ByteArray): ByteArray =
        if (status == ReplyStatus.OK) body else throw UndraReplyException(status, body)

    private fun complete(entry: Pending, status: ReplyStatus, body: ByteArray, callId: UInt) {
        when (entry) {
            is Pending.Suspended -> {
                val result = if (status == ReplyStatus.OK) Result.success(body) else Result.failure(UndraReplyException(status, body))
                resumeSafely(entry.continuation, result)
            }
            is Pending.Blocking -> entry.future.complete(Payloads.Reply(callId, status, body))
            is Pending.Streaming -> Unit // handled by the caller
        }
    }

    private fun fail(entry: Pending, error: Throwable) {
        when (entry) {
            is Pending.Suspended -> resumeSafely(entry.continuation, Result.failure(error))
            is Pending.Blocking -> entry.future.completeExceptionally(error)
            is Pending.Streaming -> failStream(entry.stream, error)
        }
    }

    private fun failStream(stream: StreamState, error: Throwable) {
        UndraDispatchers.delivery.execute {
            stream.coreDone = true
            stream.opened.completeExceptionally(error)
            stream.items.close(error)
        }
    }

    private fun failAll(error: Throwable) {
        for (id in pending.keys.toList()) {
            val entry = pending.remove(id) ?: continue
            fail(entry, error)
        }
    }

    /**
     * Resumes [continuation] without ever running its code on the current thread unless the
     * continuation's own dispatcher will move it elsewhere first. A continuation whose dispatcher would
     * run it inline (`Dispatchers.Unconfined`, an immediate dispatcher on its own thread, none at all)
     * is resumed from [UndraDispatchers.delivery] instead: the current thread may be a core thread
     * holding the core lock, and application code must not run there.
     */
    private fun resumeSafely(continuation: CancellableContinuation<ByteArray>, result: Result<ByteArray>) {
        val dispatcher = continuation.context[ContinuationInterceptor] as? CoroutineDispatcher
        if (dispatcher != null && dispatcher.isDispatchNeeded(continuation.context)) {
            continuation.resumeWith(result)
        } else {
            UndraDispatchers.delivery.execute { continuation.resumeWith(result) }
        }
    }

    // ---- TransportEvents -------------------------------------------------------------------------------

    override fun onReply(callId: UInt, status: ReplyStatus, body: ByteArray) {
        val entry = pending[callId.toInt()]
        if (entry == null) {
            UndraLog.debug("ignoring the reply to call $callId: it is no longer pending (cancelled?)")
            return
        }
        if (entry is Pending.Streaming) {
            replyToStream(callId, entry.stream, status, body)
        } else if (pending.remove(callId.toInt(), entry)) {
            // Read-your-writes (ADR-031): a drain posted now runs on the main thread before a caller there resumes.
            liveMirror.drainSoon()
            complete(entry, status, body, callId)
        }
    }

    private fun replyToStream(callId: UInt, stream: StreamState, status: ReplyStatus, body: ByteArray) {
        if (status == ReplyStatus.STREAM_OPENED) {
            UndraDispatchers.delivery.execute { stream.opened.complete(Unit) }
            return
        }
        pending.remove(callId.toInt())
        val error: Throwable = if (status == ReplyStatus.OK) {
            UndraProtocolException("call $callId answered with a single value, but it was called as a stream")
        } else {
            UndraReplyException(status, body)
        }
        failStream(stream, error)
    }

    override fun onStreamItem(callId: UInt, flag: StreamFlag, body: ByteArray) {
        val entry = pending[callId.toInt()] as? Pending.Streaming ?: return // cancelled meanwhile
        val stream = entry.stream
        when (flag) {
            StreamFlag.ITEM -> UndraDispatchers.delivery.execute { stream.items.trySend(body) }
            StreamFlag.END -> {
                pending.remove(callId.toInt())
                UndraDispatchers.delivery.execute {
                    stream.coreDone = true
                    stream.items.close()
                }
            }
            StreamFlag.ERROR -> {
                // The stream's own typed error E: generated code decodes it from the ERROR body.
                pending.remove(callId.toInt())
                failStream(stream, UndraReplyException(ReplyStatus.ERROR, body))
            }
            StreamFlag.FAILED -> {
                pending.remove(callId.toInt())
                failStream(stream, streamFailure(callId, body))
            }
        }
    }

    /**
     * What a [StreamFlag.FAILED] item ends its stream with: exactly the failed reply with the same status and
     * the SPEC 3.4 body (ADR-036), so generated `fromReply` passes it through and [UndraReplyException.panicInfo]
     * and [UndraReplyException.badRequestReason] read it. A body that does not decode is an [UndraProtocolException]
     * (generated code reports it as `UndraCallError.Malformed`, as for a reply the bindings cannot read).
     */
    private fun streamFailure(callId: UInt, body: ByteArray): Throwable {
        val failure = try {
            Payloads.StreamFailure.decode(body)
        } catch (e: WireException) {
            return UndraProtocolException("the core sent a malformed stream failure: ${e.message}", e)
        }
        // A status 3 body is empty (SPEC 3.4), so the reason would be lost without this.
        if (failure.status == ReplyStatus.CANCELLED) UndraLog.debug("the core cancelled stream $callId: ${failure.message}")
        return UndraReplyException(failure.status, failure.replyBody())
    }

    override fun onMalformed(callId: UInt, error: UndraProtocolException) {
        val entry = pending.remove(callId.toInt()) ?: return
        if (entry !is Pending.Streaming) liveMirror.drainSoon()
        fail(entry, error)
        if (entry is Pending.Streaming) {
            // The collector is told the stream failed, but the core's side did not end: it keeps the stream open and
            // waits for credit until shutdown, and the collector's own cancel is skipped (`failStream` marks the
            // stream `coreDone`, which is right for an end the core sent). Cancel it here, off the callback (a native
            // call from one is refused, SPEC 6 host contract 4), like Swift's `cancelDeferred`.
            UndraDispatchers.delivery.execute { cancelQuietly(callId) }
        }
    }

    override fun onChangeSet(changeSet: ByteArray) {
        liveMirror.submit(changeSet)
    }

    override fun onPortCall(portId: UInt, methodId: UInt, portCallId: UInt, args: ByteArray): PortOutcome =
        ports.dispatch(portId, methodId, portCallId, args)

    override fun onLog(level: UByte, target: String, message: String) {
        JulLog.log(level, target, message)
    }

    override fun onClosed(cause: Throwable?) {
        val reason = when (cause) {
            null -> ClosedReason.REQUESTED
            is UndraSchemaMismatchException -> ClosedReason.SCHEMA_MISMATCH
            is UndraSessionLostException -> ClosedReason.SESSION_LOST
            else -> ClosedReason.FAILED
        }
        shutDown(cause, reason)
    }

    override fun onReconnecting(attempt: Int, cause: Throwable?) {
        if (closed.get()) return
        if (attempt == 1) {
            lossEpoch.incrementAndGet()
            // What was in flight fails with the same typed value every later call gets while the connection is down:
            // `UndraCallError.Unavailable` for a generated call (ADR-032 amendment A, ADR-051).
            failAll(
                UndraTransportException(
                    UndraTransportException.Reason.CONNECTION_LOST,
                    "the connection to the Undra dev server was lost (${cause?.message}); reconnecting",
                    cause,
                ),
            )
        }
        setConnection(ConnectionState.Reconnecting(attempt, cause))
    }

    override fun onReconnected() {
        val epoch = lossEpoch.get()
        // On a thread of ours: the transport's callback must not call back into it.
        UndraDispatchers.delivery.execute { observeAgain(epoch) }
    }

    override fun holdsObjects(): Boolean = constructed.isNotEmpty()

    /**
     * The connection is back: release what was released meanwhile and observe what the app observes again. The core
     * answers each observation with the current values, so every mirror converges by itself.
     */
    private fun observeAgain(epoch: Int) {
        if (closed.get() || lossEpoch.get() != epoch) return
        try {
            for (handle in releasedWhileDown.toList()) {
                transport.release(handle)
                releasedWhileDown.remove(handle)
            }
            val again = synchronized(observedLock) { observed.entries.map { it.key to it.value.toList() } }
            for ((handle, signals) in again) for (signal in signals) transport.observe(handle, signal, true)
        } catch (e: UndraException) {
            // The connection dropped again already: the transport reports it, and the next reconnect replays.
            UndraLog.debug("observing again after a reconnect failed (the connection dropped again; the next reconnect replays)")
        }
        if (!closed.get() && lossEpoch.get() == epoch) setConnection(ConnectionState.Connected)
    }

    private companion object {
        /** Items the collector grants a stream when it opens (SPEC 3.7). */
        const val INITIAL_CREDIT: UInt = 16u

        /** Once fewer than this many granted items remain, the collector grants [TOP_UP] more. */
        const val TOP_UP_BELOW: UInt = 8u
        const val TOP_UP: UInt = 8u

        /** How long `observe` waits for the main thread to apply the initial values. */
        const val OBSERVE_APPLY_TIMEOUT_MILLIS: Long = 5_000L

        fun encodeCall(target: CallTarget, methodId: UInt, callId: UInt, args: ByteArray): ByteArray {
            val declared = when (target) {
                is CallTarget.FreeFunction -> target.methodId
                is CallTarget.ObjectMethod -> target.methodId
                is CallTarget.Constructor -> target.methodId
                is CallTarget.LazyListPage -> methodId // carries no method id
            }
            require(declared == methodId) { "methodId $methodId does not match the method id $declared inside $target" }
            val headerBytes = when (target) {
                is CallTarget.FreeFunction, is CallTarget.ObjectMethod -> 17
                is CallTarget.Constructor -> 13
                is CallTarget.LazyListPage -> 21
            }
            val w = UndraWriter(headerBytes + args.size) // exact, so takeArray() hands out the buffer itself
            Payloads.Call(target, callId, args).encode(w)
            return w.takeArray()
        }

        fun badRequestBody(reason: String): ByteArray {
            val w = UndraWriter()
            w.writeStr(reason)
            return w.toByteArray()
        }
    }
}
