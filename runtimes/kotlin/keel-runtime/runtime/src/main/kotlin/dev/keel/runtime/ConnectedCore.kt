package dev.keel.runtime

import dev.keel.runtime.adapters.JulLog
import dev.keel.runtime.adapters.JvmAdapters
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.Handle
import dev.keel.runtime.wire.KeelWriter
import dev.keel.runtime.wire.Payloads
import dev.keel.runtime.wire.Payloads.CallTarget
import dev.keel.runtime.wire.Payloads.ReplyStatus
import dev.keel.runtime.wire.Payloads.StreamFlag
import dev.keel.runtime.wire.WireException
import dev.keel.runtime.wire.decodeAll
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
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.suspendCancellableCoroutine

/**
 * The [KeelCore] that [KeelCore.load] returns: call ids, the table of in-flight calls, streams, the
 * port registry and the mirror on top of one [Transport].
 *
 * Invariants:
 *  - a core callback ([TransportEvents]) only copies, queues or completes a future; application code
 *    runs on other threads (see [resumeSafely] and [KeelDispatchers.delivery]);
 *  - a call is in [pending] before it is sent, because an in-process core may reply on the sending thread
 *    before the send returns;
 *  - whoever removes a call from [pending] owns completing it.
 */
internal class ConnectedCore(
    private val transport: Transport,
    private val blockingTimeout: Duration,
    initialCallId: Int = 0,
) : KeelCore(), TransportEvents {

    private sealed interface Pending {
        class Suspended(val continuation: CancellableContinuation<ByteArray>) : Pending
        class Blocking(val future: CompletableFuture<Payloads.Reply>) : Pending
        class Streaming(val stream: StreamState) : Pending
    }

    /** What the collector of a stream waits on. Filled from [KeelDispatchers.delivery]. */
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
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + CoroutineName("keel-ports"))
    private val ports = PortRegistry(scope) { payload ->
        if (!closed.get()) {
            try {
                transport.portReply(payload)
            } catch (e: Exception) {
                KeelLog.warn("answering a port call failed", e)
            }
        }
    }
    private val liveMirror = Mirror()

    override val mode: Mode get() = transport.mode

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
        shutDown(null)
    }

    // ---- calls -------------------------------------------------------------------------------------------

    override fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        ensureOpen()
        val callId = nextCallId()
        val payload = encodeCall(target, methodId, callId, args)
        val reply = if (transport.isSynchronous) {
            val bytes = transport.callSync(payload)
            try {
                Payloads.Reply.decode(bytes)
            } catch (e: WireException) {
                throw KeelException("the core sent a malformed reply: ${e.message}", e)
            }
        } else {
            blockingCall(callId, payload)
        }
        if (reply.callId != callId) throw KeelException("protocol error: the reply is for call ${reply.callId}, not $callId")
        return replyBody(reply.status, reply.body)
    }

    override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        ensureOpen()
        val callId = nextCallId()
        val payload = encodeCall(target, methodId, callId, args)
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
            throw KeelException("the core returned a malformed handle: ${e.message}", e)
        }
        if (handle == 0L) throw KeelException("the core returned the null handle for a constructor")
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
            throw KeelException("the remote core did not answer within $blockingTimeout", e)
        } catch (e: ExecutionException) {
            throw (e.cause as? KeelException) ?: KeelException("the call failed: ${e.cause?.message}", e.cause)
        } catch (e: InterruptedException) {
            if (pending.remove(callId.toInt(), entry)) cancelQuietly(callId)
            Thread.currentThread().interrupt()
            throw KeelException("interrupted while waiting for the remote core", e)
        } catch (e: KeelException) {
            pending.remove(callId.toInt(), entry)
            throw e
        }
    }

    /** Hands a call to the transport; a rejected payload fails the call like a `bad request` reply would. */
    private fun submit(callId: UInt, payload: ByteArray) {
        val code = transport.call(payload)
        if (code != 0) {
            val entry = pending.remove(callId.toInt()) ?: return // a reply already answered it
            fail(entry, KeelReplyException(ReplyStatus.BAD_REQUEST, badRequestBody("the core rejected the call payload (code $code)")))
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
            KeelLog.warn("cancelling call $callId failed", e)
        }
    }

    private fun ensureOpen() {
        if (closed.get()) throw KeelException("this KeelCore is closed", closeCause)
    }

    // ---- objects, stores, ports ----------------------------------------------------------------------

    override fun observe(handle: Long, signalId: UInt, on: Boolean) {
        ensureOpen()
        transport.observe(handle, signalId, on)
        if (on && transport.isSynchronous && !liveMirror.awaitApplied(OBSERVE_APPLY_TIMEOUT_MILLIS)) {
            KeelLog.warn(
                "the initial values of ${Handle(handle)} were not applied within " +
                    "${OBSERVE_APPLY_TIMEOUT_MILLIS}ms (is the main thread blocked?); they will follow when it catches up",
            )
        }
    }

    override fun release(handle: Long) {
        liveMirror.unregister(handle)
        if (closed.get()) return
        transport.release(handle)
    }

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) {
        ensureOpen()
        transport.event(portId, methodId, payload)
    }

    override fun timerFired(timerId: UInt) {
        ensureOpen()
        transport.timerFired(timerId)
    }

    override fun registerPort(portId: UInt, impl: PortImpl) {
        ports.register(portId, impl)
    }

    override fun stats(): KeelStats {
        val hostPending = pending.size
        val mirrored = liveMirror.registeredCount
        val json = if (closed.get()) null else transport.statsJson()
        return if (json == null) {
            KeelStats(liveHandles = KeelStats.UNKNOWN, hostPendingCalls = hostPending, hostMirrorHandles = mirrored)
        } else {
            KeelStats.fromCoreJson(json, hostPending, mirrored)
        }
    }

    override fun snapshot(): ByteArray {
        ensureOpen()
        return transport.snapshot()
    }

    override fun restore(snapshot: ByteArray) {
        ensureOpen()
        val code = transport.restore(snapshot)
        if (code != 0) throw KeelException("the core rejected the snapshot (code $code)")
    }

    override fun close() {
        shutDown(null)
    }

    private fun shutDown(cause: Throwable?) {
        // The cause must be visible before `closed` is, or a caller that sees the closed flag reports no cause.
        synchronized(closeLock) {
            if (closed.get()) return
            closeCause = cause
            closed.set(true)
        }
        KeelCore.forget(this)
        failAll(KeelException("this KeelCore was closed", cause))
        scope.cancel()
        try {
            transport.close()
        } catch (e: Exception) {
            KeelLog.warn("closing the transport failed", e)
        }
    }

    // ---- completing calls ----------------------------------------------------------------------------

    private fun replyBody(status: ReplyStatus, body: ByteArray): ByteArray =
        if (status == ReplyStatus.OK) body else throw KeelReplyException(status, body)

    private fun complete(entry: Pending, status: ReplyStatus, body: ByteArray, callId: UInt) {
        when (entry) {
            is Pending.Suspended -> {
                val result = if (status == ReplyStatus.OK) Result.success(body) else Result.failure(KeelReplyException(status, body))
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
        KeelDispatchers.delivery.execute {
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
     * is resumed from [KeelDispatchers.delivery] instead: the current thread may be a core thread
     * holding the core lock, and application code must not run there.
     */
    private fun resumeSafely(continuation: CancellableContinuation<ByteArray>, result: Result<ByteArray>) {
        val dispatcher = continuation.context[ContinuationInterceptor] as? CoroutineDispatcher
        if (dispatcher != null && dispatcher.isDispatchNeeded(continuation.context)) {
            continuation.resumeWith(result)
        } else {
            KeelDispatchers.delivery.execute { continuation.resumeWith(result) }
        }
    }

    // ---- TransportEvents -------------------------------------------------------------------------------

    override fun onReply(callId: UInt, status: ReplyStatus, body: ByteArray) {
        val entry = pending[callId.toInt()]
        if (entry == null) {
            KeelLog.debug("ignoring the reply to call $callId: it is no longer pending (cancelled?)")
            return
        }
        if (entry is Pending.Streaming) {
            replyToStream(callId, entry.stream, status, body)
        } else if (pending.remove(callId.toInt(), entry)) {
            complete(entry, status, body, callId)
        }
    }

    private fun replyToStream(callId: UInt, stream: StreamState, status: ReplyStatus, body: ByteArray) {
        if (status == ReplyStatus.STREAM_OPENED) {
            KeelDispatchers.delivery.execute { stream.opened.complete(Unit) }
            return
        }
        pending.remove(callId.toInt())
        val error: Throwable = if (status == ReplyStatus.OK) {
            KeelException("call $callId answered with a single value, but it was called as a stream")
        } else {
            KeelReplyException(status, body)
        }
        failStream(stream, error)
    }

    override fun onStreamItem(callId: UInt, flag: StreamFlag, body: ByteArray) {
        val entry = pending[callId.toInt()] as? Pending.Streaming ?: return // cancelled meanwhile
        val stream = entry.stream
        when (flag) {
            StreamFlag.ITEM -> KeelDispatchers.delivery.execute { stream.items.trySend(body) }
            StreamFlag.END -> {
                pending.remove(callId.toInt())
                KeelDispatchers.delivery.execute {
                    stream.coreDone = true
                    stream.items.close()
                }
            }
            StreamFlag.ERROR -> {
                pending.remove(callId.toInt())
                failStream(stream, KeelReplyException(ReplyStatus.ERROR, body))
            }
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
        shutDown(cause)
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
            val w = KeelWriter(headerBytes + args.size) // exact, so takeArray() hands out the buffer itself
            Payloads.Call(target, callId, args).encode(w)
            return w.takeArray()
        }

        fun badRequestBody(reason: String): ByteArray {
            val w = KeelWriter()
            w.writeStr(reason)
            return w.toByteArray()
        }
    }
}
