package dev.undra.runtime.support

import dev.undra.runtime.NativeApi
import dev.undra.runtime.NativeCallbacks
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.ReplyStatus
import java.nio.ByteBuffer
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * An in-memory stand-in for the JNI shim that enforces the contract of SPEC 6.1 the way the real one
 * would punish a violation:
 *
 *  - callbacks receive **direct** buffers that are overwritten with `0xEE` the moment the callback
 *    returns, so a runtime that keeps a view instead of copying reads garbage;
 *  - a native method called on a thread that is inside a callback is recorded in [violations] (the real
 *    core would deadlock or answer `E_REENTRANT`).
 *
 * Each fake is a core of its own: its [namespace] is unique unless a test gives it one, so fakes never share the
 * in-process transport's per-namespace claim by accident.
 */
internal class FakeNative(override val namespace: String = "fake_core_${counter.incrementAndGet()}") : NativeApi {
    override var isAvailable: Boolean = true
    override var unavailableReason: Throwable? = null
    var abi: Int = 2
    var hash: Long = HASH.toLong()
    var initResult: Int = 0

    val inits = AtomicInteger()
    @Volatile var config: ByteArray? = null
    @Volatile lateinit var callbacks: NativeCallbacks

    val violations = CopyOnWriteArrayList<String>()
    val calls = CopyOnWriteArrayList<Payloads.Call>()
    val syncCalls = CopyOnWriteArrayList<Payloads.Call>()
    val cancels = CopyOnWriteArrayList<Int>()
    val credits = CopyOnWriteArrayList<Pair<Int, Int>>()
    val observes = CopyOnWriteArrayList<Triple<Long, Int, Boolean>>()
    val releases = CopyOnWriteArrayList<Long>()
    val portReplies = CopyOnWriteArrayList<ByteArray>()
    val events = CopyOnWriteArrayList<Triple<Int, Int, ByteArray>>()
    val timers = CopyOnWriteArrayList<Int>()

    /** Runs inside `call`, on the caller's thread. */
    @Volatile var onCall: (Payloads.Call) -> Unit = {}
    @Volatile var onCallSync: (Payloads.Call) -> ByteArray = { replyPayload(it.callId, ReplyStatus.OK) }
    @Volatile var onObserve: (Long, Int, Boolean) -> Unit = { _, _, _ -> }
    @Volatile var callResult: Int = 0
    @Volatile var snapshotBytes: ByteArray = ByteArray(8)
    @Volatile var restoreResult: Int = 0
    @Volatile var stats: String = "{\"live_handles\":0}"

    private val insideCallback = ThreadLocal.withInitial { false }
    private val coreThread = Executors.newSingleThreadExecutor { r -> Thread(r, "fake-native-core").also { it.isDaemon = true } }

    /** Runs [block] on the core thread and waits for it. */
    fun onCore(block: () -> Unit) {
        coreThread.submit(block).get(10, TimeUnit.SECONDS)
    }

    // ---- what the core does: invoke callbacks with buffers that die on return ----

    private inline fun <R> asCallback(block: () -> R): R {
        insideCallback.set(true)
        try {
            return block()
        } finally {
            insideCallback.set(false)
        }
    }

    private inline fun <R> withDirect(payload: ByteArray, block: (ByteBuffer) -> R): R {
        val buffer = ByteBuffer.allocateDirect(payload.size)
        buffer.put(payload)
        buffer.flip()
        try {
            return asCallback { block(buffer) }
        } finally {
            buffer.clear()
            while (buffer.hasRemaining()) buffer.put(0xEE.toByte())
        }
    }

    fun emitReply(payload: ByteArray) {
        val callId = Payloads.Reply.decode(payload).callId.toInt()
        withDirect(payload) { callbacks.onReply(callId, it) }
    }

    /** Delivers [payload] as the reply of [callId] even if its own call id says otherwise. */
    fun emitReplyFor(callId: Int, payload: ByteArray) = withDirect(payload) { callbacks.onReply(callId, it) }

    fun emitChangeSet(payload: ByteArray) = withDirect(payload) { callbacks.onChangeSet(it) }

    fun emitStream(callId: Int, payload: ByteArray) = withDirect(payload) { callbacks.onStream(callId, it) }

    /** Calls a port like the core does; returns what `onPortCall` returned, and leaves a sync answer in [syncPortReply]. */
    fun portCall(portId: Int, methodId: Int, portCallId: Int, args: ByteArray): Int {
        val (result, reply) = portCallFull(portId, methodId, portCallId, args)
        syncPortReply = reply
        return result
    }

    /** Like [portCall], but hands the return code and the sync answer back together, so concurrent callers do not share state. */
    fun portCallFull(portId: Int, methodId: Int, portCallId: Int, args: ByteArray): Pair<Int, ByteArray?> =
        withDirect(args) { buffer ->
            val result = callbacks.onPortCall(portId, methodId, portCallId, buffer)
            // The core reads the answer right after a 0, on the same thread, like the C ABI's out_reply.
            result to (if (result == 0) callbacks.portSyncReply() else null)
        }

    @Volatile var syncPortReply: ByteArray? = null

    // ---- NativeApi: what the host calls ----

    private fun checkNotInCallback(what: String) {
        if (insideCallback.get()) violations.add("$what called from inside a callback on ${Thread.currentThread().name}")
    }

    override fun abiVersion(): Int = abi

    override fun schemaHash(): Long = hash

    @Volatile var schema: ByteArray = "{}".toByteArray()

    override fun schemaJson(): ByteArray = schema

    override fun init(cfg: ByteArray, cb: NativeCallbacks): Int {
        checkNotInCallback("init")
        inits.incrementAndGet()
        config = cfg
        callbacks = cb
        return initResult
    }

    override fun call(payload: ByteArray): Int {
        checkNotInCallback("call")
        val call = Payloads.Call.decode(payload)
        calls.add(call)
        onCall(call)
        return callResult
    }

    override fun callSync(payload: ByteArray): ByteArray {
        checkNotInCallback("callSync")
        val call = Payloads.Call.decode(payload)
        syncCalls.add(call)
        return onCallSync(call)
    }

    override fun cancel(callId: Int) {
        checkNotInCallback("cancel")
        cancels.add(callId)
    }

    override fun streamCredit(callId: Int, credit: Int) {
        checkNotInCallback("streamCredit")
        credits.add(callId to credit)
    }

    override fun observe(handle: Long, signalId: Int, on: Boolean) {
        checkNotInCallback("observe")
        observes.add(Triple(handle, signalId, on))
        onObserve(handle, signalId, on)
    }

    override fun release(handle: Long) {
        checkNotInCallback("release")
        releases.add(handle)
    }

    override fun portReply(payload: ByteArray) {
        checkNotInCallback("portReply")
        portReplies.add(payload)
    }

    override fun event(portId: Int, methodId: Int, payload: ByteArray) {
        checkNotInCallback("event")
        events.add(Triple(portId, methodId, payload))
    }

    override fun timerFired(timerId: Int) {
        checkNotInCallback("timerFired")
        timers.add(timerId)
    }

    override fun snapshot(): ByteArray {
        checkNotInCallback("snapshot")
        return snapshotBytes
    }

    override fun restore(snapshot: ByteArray): Int {
        checkNotInCallback("restore")
        return restoreResult
    }

    override fun statsJson(): String = stats

    val shutdowns = AtomicInteger()

    override fun shutdown() {
        checkNotInCallback("shutdown")
        shutdowns.incrementAndGet()
    }

    private companion object {
        val counter = AtomicInteger()
    }
}
