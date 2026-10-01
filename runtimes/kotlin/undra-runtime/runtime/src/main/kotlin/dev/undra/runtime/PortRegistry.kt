package dev.undra.runtime

import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads.PortStatus
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
 * An [UndraPortException] answers with status `ERROR` and its body; any other exception is handed to [failed]
 * (logged and passed to `LoadOptions.onError`) and answers `UNAVAILABLE`, so the core sees a
 * `PortError::Unavailable` rather than a hung call.
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
        } catch (e: UndraPortException) {
            return PortOutcome.Sync(portReply(portCallId, PortStatus.ERROR, e.body))
        } catch (e: OutOfMemoryError) {
            throw e
        } catch (e: Throwable) {
            failed(e, operationName(portId, methodId))
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
            } catch (e: UndraPortException) {
                portReply(portCallId, PortStatus.ERROR, e.body)
            } catch (e: CancellationException) {
                // Our scope was cancelled (the core is closing): no one is left to answer. A cancellation
                // that the implementation raised itself (its own timeout) is just a failure.
                if (!isActive) throw e
                UndraLog.warn("async port $portId method $methodId was cancelled by the implementation", e)
                portReply(portCallId, PortStatus.UNAVAILABLE, EMPTY)
            } catch (e: OutOfMemoryError) {
                throw e
            } catch (e: Throwable) {
                failed(e, operationName(portId, methodId))
                portReply(portCallId, PortStatus.UNAVAILABLE, EMPTY)
            }
            reply(answer)
        }
        return PortOutcome.Async
    }

    /** Completion of a sync port body; it is only ever reached if the body wrongly suspended and resumed later. */
    private object IgnoredCompletion : Continuation<ByteArray> {
        override val context = EmptyCoroutineContext

        override fun resumeWith(result: Result<ByteArray>) = Unit
    }

    private companion object {
        val EMPTY = ByteArray(0)

        /** The operation a failed port is reported as: `port 0x1234abcd method 0x5678cdef`. */
        fun operationName(portId: UInt, methodId: UInt): String = "port 0x${portId.toString(16)} method 0x${methodId.toString(16)}"

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
