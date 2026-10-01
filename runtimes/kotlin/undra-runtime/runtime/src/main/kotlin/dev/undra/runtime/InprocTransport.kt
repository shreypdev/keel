package dev.undra.runtime

import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.Payloads.StreamFailure
import dev.undra.runtime.wire.Payloads.StreamFlag
import dev.undra.runtime.wire.WireException
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The [UndraNative] entry points as an interface, so that the in-process transport can be exercised
 * against an in-memory fake of the JNI contract when the native library is not around.
 */
internal interface NativeApi {
    val isAvailable: Boolean
    val unavailableReason: Throwable?
    fun abiVersion(): Int
    fun schemaHash(): Long
    fun init(cfg: ByteArray, cb: UndraNative.Callbacks): Int
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
    fun shutdown()
}

/** The real thing: straight calls to [UndraNative]. */
internal object JniNativeApi : NativeApi {
    override val isAvailable: Boolean get() = UndraNative.isAvailable
    override val unavailableReason: Throwable? get() = UndraNative.unavailableReason
    override fun abiVersion(): Int = UndraNative.abiVersion()
    override fun schemaHash(): Long = UndraNative.schemaHash()
    override fun init(cfg: ByteArray, cb: UndraNative.Callbacks): Int = UndraNative.init(cfg, cb)
    override fun call(payload: ByteArray): Int = UndraNative.call(payload)
    override fun callSync(payload: ByteArray): ByteArray = UndraNative.callSync(payload)
    override fun cancel(callId: Int) = UndraNative.cancel(callId)
    override fun streamCredit(callId: Int, credit: Int) = UndraNative.streamCredit(callId, credit)
    override fun observe(handle: Long, signalId: Int, on: Boolean) = UndraNative.observe(handle, signalId, on)
    override fun release(handle: Long) = UndraNative.release(handle)
    override fun portReply(payload: ByteArray) = UndraNative.portReply(payload)
    override fun event(portId: Int, methodId: Int, payload: ByteArray) = UndraNative.event(portId, methodId, payload)
    override fun timerFired(timerId: Int) = UndraNative.timerFired(timerId)
    override fun snapshot(): ByteArray = UndraNative.snapshot()
    override fun restore(snapshot: ByteArray): Int = UndraNative.restore(snapshot)
    override fun statsJson(): String = UndraNative.statsJson()
    override fun shutdown() = UndraNative.shutdown()
}

/**
 * The core in this process, over JNI ([UndraNative]).
 *
 * The callbacks arrive on the core thread, a blocking-pool thread or the calling thread, possibly with
 * the core lock held, and hand out direct buffers that die when the callback returns. So each callback
 * **copies** what it needs into a fresh array, passes it to [TransportEvents] and returns; it never calls
 * a native method (a thread-local flag turns an attempt into an [UndraException] instead of a deadlock),
 * and it never lets an exception escape into native code.
 *
 * The native runtime is process-global, so the transport claims it on a successful [connect]: a second
 * in-process core is refused while one is loaded. [close] ends the native core's work through
 * [UndraNative.shutdown] (ADR-034: its tasks, timers and port traffic stop, in-flight calls end) and gives
 * the claim back, so a later [UndraCore.load] in the same process starts a fresh core.
 */
internal class InprocTransport(private val native: NativeApi = JniNativeApi) : Transport {
    override val mode: Mode get() = Mode.INPROC
    override val isSynchronous: Boolean get() = true

    @Volatile
    private var events: TransportEvents? = null
    private val closed = AtomicBoolean(false)
    /** This transport started the native core (and so owns its shutdown and the claim). */
    @Volatile
    private var started = false
    private val syncReply = ThreadLocal<ByteArray?>()
    private val insideCallback = ThreadLocal.withInitial { false }

    override fun connect(events: TransportEvents, expectedSchemaHash: ULong): ULong {
        if (!native.isAvailable) {
            throw UndraException(
                "the native Undra core library could not be loaded (${native.unavailableReason?.message}); " +
                    "put it on java.library.path, or set -D${UndraNative.PATH_PROPERTY}=<file> / -D${UndraNative.NAME_PROPERTY}=<name> " +
                    "(default ${UndraNative.DEFAULT_NAME}), or use Mode.REMOTE",
                native.unavailableReason,
            )
        }
        val abi = native.abiVersion()
        if (abi != ABI_VERSION) {
            throw UndraException("the native Undra core speaks ABI version $abi, but this runtime speaks $ABI_VERSION; use matching builds")
        }
        // Compare before initializing so that a core built from another schema never starts.
        val got = native.schemaHash().toULong()
        if (got != expectedSchemaHash) return got
        if (!claimed.add(native)) {
            throw UndraException(
                "an in-process Undra core is already loaded in this process and cannot be unloaded; " +
                    "use UndraCore.shared instead of loading it again",
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
            throw UndraException("undra_init failed with code $code")
        }
        started = true
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

    override fun checkClose() {
        checkNotInCallback("close")
    }

    override fun close() {
        // Refused from a callback, like every native entry: the shutdown would wait for this very thread.
        checkNotInCallback("close")
        if (!closed.compareAndSet(false, true)) return
        // Detached first: what the shutdown answers (status 3, cancelled streams) has nowhere to go; the
        // UndraCore above has already failed its pending calls as closed.
        events = null
        // Only the transport that started the core ends it: a failed or refused connect owns nothing.
        if (!started) return
        try {
            native.shutdown()
        } finally {
            claimed.remove(native)
        }
    }

    private fun checkNotInCallback(what: String) {
        if (insideCallback.get()) {
            throw UndraException(
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
            UndraLog.warn("handling a core $what callback failed", e)
        } finally {
            insideCallback.set(false)
        }
    }

    private val callbacks = object : UndraNative.Callbacks {
        override fun onReply(callId: Int, reply: ByteBuffer) = inCallback("reply") {
            val target = events ?: return@inCallback
            try {
                val r = UndraReader(reply)
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
                val r = UndraReader(item)
                val id = r.readU32()
                val at = r.position
                val flag = StreamFlag.fromByte(r.readU8(), at)
                target.onStreamItem(id, flag, r.readRemaining())
            } catch (e: WireException) {
                // A failure, not flag 2: that one carries the stream's own typed error E (ADR-036).
                val failure = StreamFailure(ReplyStatus.BAD_REQUEST, "the core sent a malformed stream item: ${e.message}", "")
                target.onStreamItem(callId.toUInt(), StreamFlag.FAILED, failure.toByteArray())
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
            val w = UndraWriter()
            w.writeStr(reason)
            return w.toByteArray()
        }

        /** `RuntimeConfig`: `platform String, mode String, core_threads u8, blocking_threads u8, log_level u8` (SPEC 6). */
        fun encodeConfig(): ByteArray {
            val w = UndraWriter()
            w.writeStr(Platform.name)
            w.writeStr("inproc")
            w.writeU8(1u) // core threads: the single undra-core thread
            w.writeU8(0u) // blocking threads: the core picks min(4, cores)
            w.writeU8(2u) // log level: info and above
            return w.toByteArray()
        }
    }
}
