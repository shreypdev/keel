package dev.undra.runtime

import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.Payloads.StreamFlag
import dev.undra.runtime.wire.WireException
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/**
 * A core in this process, over the JNI natives of its generated `UndraCoreNative` ([native], ADR-044).
 *
 * The callbacks arrive on the core thread, a blocking-pool thread or the calling thread, possibly with
 * the core lock held, and hand out direct buffers that die when the callback returns. So each callback
 * **copies** what it needs into a fresh array, passes it to [TransportEvents] and returns; it never calls
 * a native method (a thread-local flag turns an attempt into an [UndraReplyException] with status `BAD_REQUEST` and the core's own
 * `E_REENTRANT` reason instead of a deadlock),
 * and it never lets an exception escape into native code.
 *
 * Each core's native runtime is global to its library, so the transport claims the core's namespace on a
 * successful [connect]: a second in-process core with the same namespace is refused while one is loaded, and
 * cores with different namespaces run side by side. [close] ends the native core's work through
 * [NativeApi.shutdown] (ADR-034: its tasks, timers and port traffic stop, in-flight calls end) and gives
 * the claim back, so a later load of the same core in the same process starts a fresh one.
 */
internal class InprocTransport(private val native: NativeApi) : Transport {
    override val mode: Mode get() = Mode.INPROC
    override val isSynchronous: Boolean get() = true

    @Volatile
    private var events: TransportEvents? = null
    private val closed = AtomicBoolean(false)
    /** The namespace this transport claimed and started the native core under (so it owns the shutdown and the claim), or `null`. */
    @Volatile
    private var started: String? = null
    private val syncReply = ThreadLocal<ByteArray?>()
    private val insideCallback = ThreadLocal.withInitial { false }

    override fun connect(events: TransportEvents, expectedSchemaHash: ULong): ULong {
        val namespace = native.namespace
        if (!native.isAvailable) {
            throw UndraException(
                "the native library of the Undra core `$namespace` could not be loaded (${native.unavailableReason?.message}); " +
                    "put lib$namespace on java.library.path (Android: jniLibs/<abi>/lib$namespace.so), " +
                    "or set -D${NativeLibrary.pathProperty(namespace)}=<file>, or use Mode.REMOTE",
                native.unavailableReason,
            )
        }
        val abi = native.abiVersion()
        if (abi != ABI_VERSION) {
            throw UndraException(
                "the native Undra core `$namespace` speaks ABI version $abi, but this runtime speaks $ABI_VERSION; " +
                    "rebuild the core with the undra-ffi that matches this runtime",
            )
        }
        // Compare before initializing so that a core built from another schema never starts.
        val got = native.schemaHash().toULong()
        if (got != expectedSchemaHash) return got
        if (!claimed.add(namespace)) {
            throw UndraException(
                "the Undra core `$namespace` is already loaded in this process; use the core its load returned " +
                    "(its generated Undra<Namespace>.core) instead of loading it again, or close it before loading it again",
            )
        }
        this.events = events
        val code = try {
            native.init(encodeConfig(), callbacks)
        } catch (e: Throwable) {
            this.events = null
            claimed.remove(namespace)
            throw e
        }
        if (code != 0) {
            this.events = null
            claimed.remove(namespace)
            throw UndraException("init of the Undra core `$namespace` failed with code $code")
        }
        started = namespace
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

    /**
     * Ends the native core and **waits for it**: [NativeApi.shutdown] returns after the core's threads are joined
     * and the port callbacks running on other threads have returned (SPEC 6, host contract 5). Refused from a
     * callback, where it would wait for its own thread.
     */
    override fun close() {
        // Refused from a callback, like every native entry: the shutdown would wait for this very thread.
        checkNotInCallback("close")
        if (!closed.compareAndSet(false, true)) return
        // Detached first: what the shutdown answers (status 3, cancelled streams) has nowhere to go; the
        // UndraCore above has already failed its pending calls as closed.
        events = null
        // Only the transport that started the core ends it: a failed or refused connect owns nothing.
        val namespace = started ?: return
        try {
            native.shutdown()
        } finally {
            claimed.remove(namespace)
        }
    }

    private fun checkNotInCallback(what: String) {
        if (insideCallback.get()) {
            // The refusal the core itself makes (status 5, E_REENTRANT, SPEC 6), so a generated call reports it as
            // UndraCallError.Refused like every other platform does.
            throw UndraReplyException(
                ReplyStatus.BAD_REQUEST,
                reasonBody(
                    "E_REENTRANT: $what was called from inside a core callback (a sync port implementation?); " +
                        "the core lock may be held, so this would deadlock. Hand the work to another thread.",
                ),
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

    private val callbacks = object : NativeCallbacks {
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
                target.onMalformed(callId.toUInt(), UndraProtocolException("the core sent a malformed reply: ${e.message}", e))
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
                target.onMalformed(callId.toUInt(), UndraProtocolException("the core sent a malformed stream item: ${e.message}", e))
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
        /** The native ABI this runtime speaks: version 2 is the per-core function table of ADR-044. */
        const val ABI_VERSION: Int = 2
        const val PORT_SYNC: Int = 0
        const val PORT_ASYNC: Int = 1
        const val PORT_UNAVAILABLE: Int = 2
        val NO_BYTES = ByteArray(0)

        /** The namespaces of the cores started in this process (ADR-044: one in-process core per namespace). */
        val claimed: MutableSet<String> = ConcurrentHashMap.newKeySet()

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
