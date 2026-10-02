package dev.undra.runtime

import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.StorageError
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads.PortStatus
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.ConcurrentHashMap
import kotlin.coroutines.Continuation
import kotlin.coroutines.EmptyCoroutineContext
import kotlin.coroutines.intrinsics.COROUTINE_SUSPENDED
import kotlin.coroutines.intrinsics.startCoroutineUninterceptedOrReturn
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * The platform side of the core's port calls (SPEC 5.7 and 6.3): a table from port id to [PortImpl] and
 * the logic that runs a call.
 *
 *  - A **sync** port (`PortImpl.sync`) is run inline, on whatever thread the core called from, and its
 *    answer is returned as [PortOutcome.Sync]. Its method must not suspend; if it does, the call is
 *    answered as unavailable (the runtime cannot wait: the core lock may be held).
 *  - An **async** port runs on [scope] (off the core's threads) and answers later through [reply] with a
 *    whole `PortReply` payload; the core is told [PortOutcome.Async] straight away.
 *  - A port or method that is not registered is [PortOutcome.Unavailable].
 *
 * A typed failure answers with status `ERROR` (1) and the encoded error: an [UndraPortException] with its body, and,
 * thrown as itself by a method of the standard port it belongs to, a [StorageError] (`Kv`, `SecureStore`; ADR-049), an
 * [FsError] (`Fs`) or an [HttpError] (`Http`). Any other exception is a bug in the port implementation: it is logged at
 * error level, naming the port and the method, handed to [failed] (which passes it to `LoadOptions.onError`) and
 * answered `UNAVAILABLE` (2), so the core sees a `PortError::Unavailable` rather than a hung call.
 */
internal class PortRegistry(
    private val scope: CoroutineScope,
    private val failed: (error: Throwable, operation: String) -> Unit,
    private val reply: (ByteArray) -> Unit,
) {
    private val ports = ConcurrentHashMap<Int, PortImpl>()

    /** Registers [impl] for [portId]; an implementation it replaces is detached ([PortImpl.detach]). */
    fun register(portId: UInt, impl: PortImpl) {
        val previous = ports.put(portId.toInt(), impl)
        if (previous != null && previous !== impl) detach(portId, previous)
    }

    /** Detaches every registered implementation (the core is closing); they stay registered but answer nothing new. */
    fun detachAll() {
        for ((id, impl) in ports) detach(id.toUInt(), impl)
    }

    private fun detach(portId: UInt, impl: PortImpl) {
        val hook = impl.detach ?: return
        try {
            hook()
        } catch (e: OutOfMemoryError) {
            throw e
        } catch (e: Throwable) {
            UndraLog.warn("detaching the implementation of port $portId failed", e)
        }
    }

    fun dispatch(portId: UInt, methodId: UInt, portCallId: UInt, args: ByteArray): PortOutcome {
        val impl = ports[portId.toInt()]
        if (impl == null) {
            UndraLog.debug("port call to unregistered port $portId")
            return PortOutcome.Unavailable
        }
        val method = impl.methods[methodId]
        if (method == null) {
            UndraLog.debug("port $portId has no method $methodId")
            return PortOutcome.Unavailable
        }
        return if (impl.sync) runSync(method, portId, methodId, portCallId, args) else runAsync(method, portId, methodId, portCallId, args)
    }

    private fun runSync(
        method: suspend (ByteArray) -> ByteArray,
        portId: UInt,
        methodId: UInt,
        portCallId: UInt,
        args: ByteArray,
    ): PortOutcome {
        val result: Any? = try {
            method.startCoroutineUninterceptedOrReturn(args, IgnoredCompletion)
        } catch (e: OutOfMemoryError) {
            throw e
        } catch (e: Throwable) {
            val typed = typedBody(portId, e)
            if (typed != null) return PortOutcome.Sync(portReply(portCallId, PortStatus.ERROR, typed))
            untyped(e, portId, methodId)
            return PortOutcome.Unavailable
        }
        if (result === COROUTINE_SUSPENDED) {
            UndraLog.warn("sync port $portId method $methodId suspended; a sync port must answer inline, so the call is reported as unavailable")
            return PortOutcome.Unavailable
        }
        @Suppress("UNCHECKED_CAST")
        return PortOutcome.Sync(portReply(portCallId, PortStatus.OK, result as ByteArray))
    }

    private fun runAsync(
        method: suspend (ByteArray) -> ByteArray,
        portId: UInt,
        methodId: UInt,
        portCallId: UInt,
        args: ByteArray,
    ): PortOutcome {
        scope.launch {
            val answer: ByteArray = try {
                portReply(portCallId, PortStatus.OK, method(args))
            } catch (e: CancellationException) {
                // Our scope was cancelled (the core is closing): no one is left to answer. A cancellation
                // that the implementation raised itself (its own timeout) is just a failure.
                if (!isActive) throw e
                UndraLog.warn("async port $portId method $methodId was cancelled by the implementation", e)
                portReply(portCallId, PortStatus.UNAVAILABLE, EMPTY)
            } catch (e: OutOfMemoryError) {
                throw e
            } catch (e: Throwable) {
                val typed = typedBody(portId, e)
                if (typed != null) {
                    portReply(portCallId, PortStatus.ERROR, typed)
                } else {
                    untyped(e, portId, methodId)
                    portReply(portCallId, PortStatus.UNAVAILABLE, EMPTY)
                }
            }
            reply(answer)
        }
        return PortOutcome.Async
    }

    /** A failure that is not the port's typed error: logged at error level, reported, and answered unavailable by the caller. */
    private fun untyped(error: Throwable, portId: UInt, methodId: UInt) {
        UndraLog.error(
            "${describe(portId, methodId)} failed with ${error.javaClass.name}, which is not the port's typed error, so the core " +
                "is answered unavailable (port status 2). A port implementation reports a failure as an UndraPortException with " +
                "its error type (a standard port: StorageError for Kv and SecureStore, FsError for Fs, HttpError for Http)",
            error,
        )
        failed(error, operationName(portId, methodId))
    }

    /** Completion of a sync port body; it is only ever reached if the body wrongly suspended and resumed later. */
    private object IgnoredCompletion : Continuation<ByteArray> {
        override val context = EmptyCoroutineContext

        override fun resumeWith(result: Result<ByteArray>) = Unit
    }

    internal companion object {
        private val EMPTY = ByteArray(0)

        /** `<Trait>.<method>` of every standard port method, by method id (ids are hashes of exactly that text). */
        private val STANDARD_METHODS: Map<UInt, String> = mapOf(
            StandardPorts.Clock.NOW_MS to "Clock.now_ms",
            StandardPorts.Clock.MONOTONIC_NS to "Clock.monotonic_ns",
            StandardPorts.Rng.FILL to "Rng.fill",
            StandardPorts.Log.LOG to "Log.log",
            StandardPorts.Http.REQUEST to "Http.request",
            StandardPorts.Kv.GET to "Kv.get",
            StandardPorts.Kv.SET to "Kv.set",
            StandardPorts.Kv.DELETE to "Kv.delete",
            StandardPorts.Kv.LIST to "Kv.list",
            StandardPorts.SecureStore.GET to "SecureStore.get",
            StandardPorts.SecureStore.SET to "SecureStore.set",
            StandardPorts.SecureStore.DELETE to "SecureStore.delete",
            StandardPorts.SecureStore.LIST to "SecureStore.list",
            StandardPorts.Fs.READ to "Fs.read",
            StandardPorts.Fs.WRITE to "Fs.write",
            StandardPorts.Fs.DELETE to "Fs.delete",
            StandardPorts.Fs.LIST to "Fs.list",
            StandardPorts.Timer.SET to "Timer.set",
        )

        /** The operation a failed port is reported as to `onError`: `port 0x1234abcd method 0x5678cdef` (docs/ERRORS.md). */
        fun operationName(portId: UInt, methodId: UInt): String = "port 0x${portId.toString(16)} method 0x${methodId.toString(16)}"

        /** The port method in a log line: `the Kv.get port method (port 0x5389110d method 0xf050bb1a)` for a standard one. */
        fun describe(portId: UInt, methodId: UInt): String {
            val name = STANDARD_METHODS[methodId]?.takeIf { it.substringBefore('.') == standardPortName(portId) }
            return if (name == null) operationName(portId, methodId) else "the $name port method (${operationName(portId, methodId)})"
        }

        private fun standardPortName(portId: UInt): String? = when (portId) {
            StandardPorts.Clock.PORT_ID -> "Clock"
            StandardPorts.Rng.PORT_ID -> "Rng"
            StandardPorts.Log.PORT_ID -> "Log"
            StandardPorts.Http.PORT_ID -> "Http"
            StandardPorts.Kv.PORT_ID -> "Kv"
            StandardPorts.SecureStore.PORT_ID -> "SecureStore"
            StandardPorts.Fs.PORT_ID -> "Fs"
            StandardPorts.Timer.PORT_ID -> "Timer"
            else -> null
        }

        /**
         * The body of the typed reply [error] stands for when a method of [portId] throws it, or `null` if it is not a
         * typed failure: an [UndraPortException]'s body, or a standard port's own error type, encoded, when thrown by a
         * method of that port (a [StorageError] from anything else would not decode as that port's error in the core).
         */
        fun typedBody(portId: UInt, error: Throwable): ByteArray? = when {
            error is UndraPortException -> error.body
            error is StorageError && (portId == StandardPorts.Kv.PORT_ID || portId == StandardPorts.SecureStore.PORT_ID) ->
                StorageError.encodeToByteArray(error)
            error is FsError && portId == StandardPorts.Fs.PORT_ID -> FsError.encodeToByteArray(error)
            error is HttpError && portId == StandardPorts.Http.PORT_ID -> HttpError.encodeToByteArray(error)
            else -> null
        }

        /** A whole `PortReply` payload: `port_call_id u32, status u8, body`. */
        fun portReply(portCallId: UInt, status: PortStatus, body: ByteArray): ByteArray {
            val w = UndraWriter(5 + body.size)
            w.writeU32(portCallId)
            w.writeU8(status.code)
            w.writeRaw(body)
            return w.takeArray()
        }
    }
}
