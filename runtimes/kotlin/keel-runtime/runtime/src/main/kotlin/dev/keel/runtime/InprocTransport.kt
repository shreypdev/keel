package dev.keel.runtime

import dev.keel.runtime.wire.KeelReader
import dev.keel.runtime.wire.KeelWriter
import dev.keel.runtime.wire.Payloads.ReplyStatus
import dev.keel.runtime.wire.Payloads.StreamFlag
import dev.keel.runtime.wire.WireException
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The [KeelNative] entry points as an interface, so that the in-process transport can be exercised
 * against an in-memory fake of the JNI contract when the native library is not around.
 */
internal interface NativeApi {
    val isAvailable: Boolean
    val unavailableReason: Throwable?
    fun abiVersion(): Int
    fun schemaHash(): Long
    fun init(cfg: ByteArray, cb: KeelNative.Callbacks): Int
    fun call(payload: ByteArray): Int
    fun callSync(payload: ByteArray): ByteArray
    fun cancel(callId: Int)
    fun streamCredit(callId: Int, credit: Int)
    fun observe(handle: Long, signalId: Int, on: Boolean)
    fun release(handle: Long)
    fun portReply(payload: ByteArray)
    fun event(portId: Int, methodId: Int, payload: ByteArray)
    fun timerFired(timerId: Int)
    fun snapshot(): ByteArray
    fun restore(snapshot: ByteArray): Int
    fun statsJson(): String
}

/** The real thing: straight calls to [KeelNative]. */
internal object JniNativeApi : NativeApi {
    override val isAvailable: Boolean get() = KeelNative.isAvailable
    override val unavailableReason: Throwable? get() = KeelNative.unavailableReason
    override fun abiVersion(): Int = KeelNative.abiVersion()
    override fun schemaHash(): Long = KeelNative.schemaHash()
    override fun init(cfg: ByteArray, cb: KeelNative.Callbacks): Int = KeelNative.init(cfg, cb)
    override fun call(payload: ByteArray): Int = KeelNative.call(payload)
    override fun callSync(payload: ByteArray): ByteArray = KeelNative.callSync(payload)
    override fun cancel(callId: Int) = KeelNative.cancel(callId)
    override fun streamCredit(callId: Int, credit: Int) = KeelNative.streamCredit(callId, credit)
    override fun observe(handle: Long, signalId: Int, on: Boolean) = KeelNative.observe(handle, signalId, on)
    override fun release(handle: Long) = KeelNative.release(handle)
    override fun portReply(payload: ByteArray) = KeelNative.portReply(payload)
    override fun event(portId: Int, methodId: Int, payload: ByteArray) = KeelNative.event(portId, methodId, payload)
    override fun timerFired(timerId: Int) = KeelNative.timerFired(timerId)
    override fun snapshot(): ByteArray = KeelNative.snapshot()
    override fun restore(snapshot: ByteArray): Int = KeelNative.restore(snapshot)
    override fun statsJson(): String = KeelNative.statsJson()
}

/**
 * The core in this process, over JNI ([KeelNative]).
 *
 * The callbacks arrive on the core thread, a blocking-pool thread or the calling thread, possibly with
 * the core lock held, and hand out direct buffers that die when the callback returns. So each callback
 * **copies** what it needs into a fresh array, passes it to [TransportEvents] and returns; it never calls
 * a native method (a thread-local flag turns an attempt into a [KeelException] instead of a deadlock),
 * and it never lets an exception escape into native code.
 *
 * The native runtime is process-global and cannot be shut down through JNI, so the transport claims it
 * on the first successful [connect] and never gives it back: a second in-process core is refused.
 */
internal class InprocTransport(private val native: NativeApi = JniNativeApi) : Transport {
    override val mode: Mode get() = Mode.INPROC
    override val isSynchronous: Boolean get() = true

    @Volatile
    private var events: TransportEvents? = null
    private val closed = AtomicBoolean(false)
    private val syncReply = ThreadLocal<ByteArray?>()
    private val insideCallback = ThreadLocal.withInitial { false }

    override fun connect(events: TransportEvents, expectedSchemaHash: ULong): ULong {
        if (!native.isAvailable) {
            throw KeelException(
                "the native Keel core library could not be loaded (${native.unavailableReason?.message}); " +
                    "put it on java.library.path, or set -D${KeelNative.PATH_PROPERTY}=<file> / -D${KeelNative.NAME_PROPERTY}=<name> " +
                    "(default ${KeelNative.DEFAULT_NAME}), or use Mode.REMOTE",
                native.unavailableReason,
            )
        }
        val abi = native.abiVersion()
        if (abi != ABI_VERSION) {
            throw KeelException("the native Keel core speaks ABI version $abi, but this runtime speaks $ABI_VERSION; use matching builds")
        }
        // Compare before initializing so that a core built from another schema never starts.
        val got = native.schemaHash().toULong()
        if (got != expectedSchemaHash) return got
        if (!claimed.add(native)) {
            throw KeelException(
                "an in-process Keel core is already loaded in this process and cannot be unloaded; " +
                    "use KeelCore.shared instead of loading it again",
            )
        }
        this.events = events
        val code = try {
            native.init(encodeConfig(), callbacks)
        } catch (e: Throwable) {
            this.events = null
            claimed.remove(native)
            throw e
        }
        if (code != 0) {
            this.events = null
            claimed.remove(native)
            throw KeelException("keel_init failed with code $code")
        }
        return got
    }

    override fun call(payload: ByteArray): Int {
        checkNotInCallback("call")
        return native.call(payload)
    }

    override fun callSync(payload: ByteArray): ByteArray {
        checkNotInCallback("callSync")
        return native.callSync(payload)
    }

    override fun cancel(callId: UInt) {
        checkNotInCallback("cancel")
        native.cancel(callId.toInt())
    }

    override fun streamCredit(callId: UInt, credit: UInt) {
        checkNotInCallback("streamCredit")
        native.streamCredit(callId.toInt(), credit.toInt())
    }

    override fun observe(handle: Long, signalId: UInt, on: Boolean) {
        checkNotInCallback("observe")
        native.observe(handle, signalId.toInt(), on)
    }

    override fun release(handle: Long) {
        checkNotInCallback("release")
        native.release(handle)
    }

    override fun portReply(payload: ByteArray) {
        checkNotInCallback("portReply")
        native.portReply(payload)
    }

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) {
        checkNotInCallback("event")
        native.event(portId.toInt(), methodId.toInt(), payload)
    }

    override fun timerFired(timerId: UInt) {
        checkNotInCallback("timerFired")
        native.timerFired(timerId.toInt())
    }

    override fun snapshot(): ByteArray {
        checkNotInCallback("snapshot")
        return native.snapshot()
    }

    override fun restore(snapshot: ByteArray): Int {
        checkNotInCallback("restore")
        return native.restore(snapshot)
    }

    override fun statsJson(): String? = native.statsJson()

    override fun close() {
        if (closed.compareAndSet(false, true)) events = null
    }

    private fun checkNotInCallback(what: String) {
        if (insideCallback.get()) {
            throw KeelException(
                "$what was called from inside a core callback (a sync port implementation?); " +
                    "the core lock may be held, so this would deadlock. Hand the work to another thread.",
            )
        }
    }

    /** Runs a callback body with the re-entrancy flag set and nothing escaping into native code. */
    private inline fun inCallback(what: String, body: () -> Unit) {
        insideCallback.set(true)
        try {
            body()
        } catch (e: Throwable) {
            KeelLog.warn("handling a core $what callback failed", e)
        } finally {
            insideCallback.set(false)
        }
    }

    private val callbacks = object : KeelNative.Callbacks {
        override fun onReply(callId: Int, reply: ByteBuffer) = inCallback("reply") {
            val target = events ?: return@inCallback
            try {
                val r = KeelReader(reply)
                val id = r.readU32()
                val at = r.position
                val status = ReplyStatus.fromByte(r.readU8(), at)
                target.onReply(id, status, r.readRemaining())
            } catch (e: WireException) {
                // The call id is known from the JNI argument, so the caller can still be told.
                target.onReply(callId.toUInt(), ReplyStatus.BAD_REQUEST, reasonBody("the core sent a malformed reply: ${e.message}"))
            }
        }

        override fun onChangeSet(changes: ByteBuffer) = inCallback("change-set") {
            events?.onChangeSet(copy(changes))
        }

        override fun onStream(callId: Int, item: ByteBuffer) = inCallback("stream") {
            val target = events ?: return@inCallback
            try {
                val r = KeelReader(item)
                val id = r.readU32()
                val at = r.position
                val flag = StreamFlag.fromByte(r.readU8(), at)
                target.onStreamItem(id, flag, r.readRemaining())
            } catch (e: WireException) {
                target.onStreamItem(callId.toUInt(), StreamFlag.ERROR, reasonBody("the core sent a malformed stream item: ${e.message}"))
            }
        }

        override fun onPortCall(portId: Int, methodId: Int, portCallId: Int, args: ByteBuffer): Int {
            var answer = PORT_UNAVAILABLE
            inCallback("port call") {
                val target = events ?: return@inCallback
                answer = when (val outcome = target.onPortCall(portId.toUInt(), methodId.toUInt(), portCallId.toUInt(), copy(args))) {
                    is PortOutcome.Sync -> {
                        syncReply.set(outcome.reply)
                        PORT_SYNC
                    }
                    PortOutcome.Async -> PORT_ASYNC
                    PortOutcome.Unavailable -> PORT_UNAVAILABLE
                }
            }
            return answer
        }

        override fun portSyncReply(): ByteArray {
            val reply = syncReply.get()
            syncReply.remove()
            return reply ?: NO_BYTES
        }
    }

    private companion object {
        const val ABI_VERSION: Int = 1
        const val PORT_SYNC: Int = 0
        const val PORT_ASYNC: Int = 1
        const val PORT_UNAVAILABLE: Int = 2
        val NO_BYTES = ByteArray(0)

        /** The processes' natives that have a core started on them. */
        val claimed: MutableSet<NativeApi> = ConcurrentHashMap.newKeySet()

        /** Copies the readable bytes of [buffer] without touching its position or limit. */
        fun copy(buffer: ByteBuffer): ByteArray {
            val out = ByteArray(buffer.remaining())
            buffer.duplicate().get(out)
            return out
        }

        fun reasonBody(reason: String): ByteArray {
            val w = KeelWriter()
            w.writeStr(reason)
            return w.toByteArray()
        }

        /** `RuntimeConfig`: `platform String, mode String, core_threads u8, blocking_threads u8, log_level u8` (SPEC 6). */
        fun encodeConfig(): ByteArray {
            val w = KeelWriter()
            w.writeStr(Platform.name)
            w.writeStr("inproc")
            w.writeU8(1u) // core threads: the single keel-core thread
            w.writeU8(0u) // blocking threads: the core picks min(4, cores)
            w.writeU8(2u) // log level: info and above
            return w.toByteArray()
        }
    }
}
