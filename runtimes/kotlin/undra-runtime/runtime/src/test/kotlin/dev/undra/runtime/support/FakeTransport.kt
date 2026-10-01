package dev.undra.runtime.support

import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraModeException
import dev.undra.runtime.UndraTransportException
import dev.undra.runtime.Mode
import dev.undra.runtime.PortOutcome
import dev.undra.runtime.Transport
import dev.undra.runtime.TransportEvents
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.Payloads.StreamFlag
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * A core that lives in the test: speaks the transport contract in-process, records everything the host
 * sends, and lets a test script the core's side (replies, streams that honour credit, change-sets, port
 * calls) either inline on the calling thread (like an in-process core's synchronous path) or from a
 * thread named `fake-core` (like the real core thread).
 */
internal class FakeTransport(
    var schemaHash: ULong = HASH,
    override val isSynchronous: Boolean = true,
) : Transport {
    override val mode: Mode get() = if (isSynchronous) Mode.INPROC else Mode.REMOTE

    lateinit var events: TransportEvents

    // ---- what the host did ----
    val calls = CopyOnWriteArrayList<Payloads.Call>()
    val syncCalls = CopyOnWriteArrayList<Payloads.Call>()
    val cancels = CopyOnWriteArrayList<UInt>()
    val credits = CopyOnWriteArrayList<Pair<UInt, UInt>>()
    val observes = CopyOnWriteArrayList<Triple<Long, UInt, Boolean>>()
    val releases = CopyOnWriteArrayList<Long>()
    val portReplies = CopyOnWriteArrayList<Payloads.PortReply>()
    val sentEvents = CopyOnWriteArrayList<Payloads.Event>()
    val timers = CopyOnWriteArrayList<UInt>()
    val connects = AtomicInteger()
    @Volatile var closed = false

    // ---- scripting ----
    /** Runs inside `call`, on the caller's thread, for every submitted call. */
    @Volatile var onCall: (Payloads.Call) -> Unit = {}

    /** The whole `Reply` payload for a synchronous call. */
    @Volatile var onCallSync: (Payloads.Call) -> ByteArray = { replyPayload(it.callId, ReplyStatus.OK) }

    /** Runs inside `observe`, on the caller's thread. */
    @Volatile var onObserve: (handle: Long, signal: UInt, on: Boolean) -> Unit = { _, _, _ -> }

    /** What `call` returns (`0` accepted). */
    @Volatile var callResult: Int = 0

    /** When set, `call` throws it. */
    @Volatile var callFailure: RuntimeException? = null

    @Volatile var statsJson: String? = null
    @Volatile var snapshotBytes: ByteArray? = null

    /** Runs inside [restore], on the calling thread: where an in-process core delivers the restored values. */
    @Volatile var onRestore: (() -> Unit)? = null
    @Volatile var restoreResult: Int = 0
    @Volatile var connectFailure: RuntimeException? = null

    /** `false` while the fake plays a remote core that is unreachable: everything the host sends fails like a remote transport's. */
    @Volatile var up = true

    private fun requireUp() {
        // What the real remote transport throws while it reconnects: typed, so a generated call maps it onto Unavailable.
        if (!up) throw UndraTransportException(UndraTransportException.Reason.CONNECTION_LOST, "not connected to the fake dev server: reconnecting")
    }

    /** The connection drops and the (pretend) transport starts reconnecting: attempt 1 is the loss. */
    fun drop(cause: Throwable = UndraException("the fake connection was lost")) {
        up = false
        events.onReconnecting(1, cause)
    }

    /** A reconnect attempt failed; the transport tries again. */
    fun retry(attempt: Int, cause: Throwable = UndraException("the fake server is still down")) = events.onReconnecting(attempt, cause)

    /** The connection is back. */
    fun reconnect() {
        up = true
        events.onReconnected()
    }

    /** The thread a real core would call back from. */
    private val coreThread = Executors.newSingleThreadExecutor { r -> Thread(r, "fake-core").also { it.isDaemon = true } }

    /** Runs [block] on the `fake-core` thread and returns immediately. */
    fun onCore(block: () -> Unit) {
        coreThread.execute {
            try {
                block()
            } catch (e: Throwable) {
                e.printStackTrace()
            }
        }
    }

    /** Waits until everything queued on the core thread so far has run. */
    fun awaitCore() {
        coreThread.submit {}.get(10, TimeUnit.SECONDS)
    }

    // ---- core to host, from a test ----
    fun replyOnCore(callId: UInt, status: ReplyStatus, body: ByteArray = NO_BYTES) =
        onCore { events.onReply(callId, status, body) }

    fun itemOnCore(callId: UInt, body: ByteArray) = onCore { events.onStreamItem(callId, StreamFlag.ITEM, body) }

    fun endOnCore(callId: UInt) = onCore { events.onStreamItem(callId, StreamFlag.END, NO_BYTES) }

    fun errorOnCore(callId: UInt, body: ByteArray) = onCore { events.onStreamItem(callId, StreamFlag.ERROR, body) }

    // ---- streams that honour credit, like the real core ----
    private val producers = java.util.concurrent.ConcurrentHashMap<UInt, StreamProducer>()

    /** Answers [call] as an opened stream that will produce [items], obeying the credit the host grants, then end (or fail with [failure]). */
    fun serveStream(call: Payloads.Call, items: List<ByteArray>, failure: ByteArray? = null): StreamProducer {
        val producer = StreamProducer(call.callId, ArrayDeque(items), failure)
        producers[call.callId] = producer
        replyOnCore(call.callId, ReplyStatus.STREAM_OPENED)
        return producer
    }

    inner class StreamProducer(val callId: UInt, private val items: ArrayDeque<ByteArray>, private val failure: ByteArray?) {
        @Volatile var sent = 0
        @Volatile var totalCredit = 0L
        @Volatile var finished = false
        private var outstanding = 0L

        fun grant(credit: UInt) {
            onCore {
                totalCredit += credit.toLong()
                outstanding += credit.toLong()
                while (outstanding > 0 && items.isNotEmpty()) {
                    events.onStreamItem(callId, StreamFlag.ITEM, items.removeFirst())
                    outstanding--
                    sent++
                }
                if (items.isEmpty() && !finished) {
                    finished = true
                    if (failure != null) events.onStreamItem(callId, StreamFlag.ERROR, failure) else events.onStreamItem(callId, StreamFlag.END, NO_BYTES)
                }
            }
        }
    }

    // ---- Transport ----
    override fun connect(events: TransportEvents, expectedSchemaHash: ULong): ULong {
        connects.incrementAndGet()
        connectFailure?.let { throw it }
        this.events = events
        return schemaHash
    }

    override fun call(payload: ByteArray): Int {
        requireUp()
        callFailure?.let { throw it }
        val call = Payloads.Call.decode(payload)
        calls.add(call)
        onCall(call)
        return callResult
    }

    override fun callSync(payload: ByteArray): ByteArray {
        if (!isSynchronous) throw UndraModeException("fake: no sync calls")
        val call = Payloads.Call.decode(payload)
        syncCalls.add(call)
        return onCallSync(call)
    }

    override fun cancel(callId: UInt) {
        requireUp()
        cancels.add(callId)
    }

    override fun streamCredit(callId: UInt, credit: UInt) {
        requireUp()
        credits.add(callId to credit)
        producers[callId]?.grant(credit)
    }

    override fun observe(handle: Long, signalId: UInt, on: Boolean) {
        requireUp()
        observes.add(Triple(handle, signalId, on))
        onObserve(handle, signalId, on)
    }

    override fun release(handle: Long) {
        requireUp()
        releases.add(handle)
    }

    override fun portReply(payload: ByteArray) {
        portReplies.add(Payloads.PortReply.decode(payload))
    }

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) {
        sentEvents.add(Payloads.Event(portId, methodId, payload))
    }

    override fun timerFired(timerId: UInt) {
        timers.add(timerId)
    }

    override fun snapshot(): ByteArray = snapshotBytes ?: throw UndraModeException("fake: no snapshots")

    override fun restore(snapshot: ByteArray): Int {
        if (snapshotBytes == null) throw UndraModeException("fake: no snapshots")
        onRestore?.invoke()
        return restoreResult
    }

    override fun statsJson(): String? = statsJson

    override fun close() {
        closed = true
        coreThread.shutdown()
    }

    /** A port call from the core; returns how the host said it would serve it. */
    fun portCall(portId: UInt, methodId: UInt, portCallId: UInt, args: ByteArray): PortOutcome =
        events.onPortCall(portId, methodId, portCallId, args)
}
