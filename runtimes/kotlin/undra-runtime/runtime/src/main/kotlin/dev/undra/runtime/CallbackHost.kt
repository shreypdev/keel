package dev.undra.runtime

import dev.undra.runtime.wire.Payloads.PortStatus
import dev.undra.runtime.wire.UndraReader
import java.util.concurrent.ConcurrentHashMap
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * Runs the core's calls into the app's callback implementations (ADR-041), for one core.
 *
 * The core calls a callback instance through an ordinary port call on the interface's port, whose arguments start
 * with the instance handle. [dispatch] runs inside that core callback (possibly with the core lock held), so it only
 * decodes and **enqueues**, and tells the core the answer comes later; the implementation runs elsewhere:
 *
 *  - **`main`** (the default): in the mirror's drain on the main thread ([Mirror.submitInvocation]), in arrival
 *    order with the change-sets, so a listener sees the stores as they were when the core called it;
 *  - **`background`**: on a serial dispatcher of the instance (`Dispatchers.Default.limitedParallelism(1)`), in call
 *    order, without waiting for a frame.
 *
 * An `async` method answers through [reply] (status 0 with the value, 1 with the method's own error, 2 when the
 * implementation is gone or failed otherwise; the last is reported as `Interface.method`); a fire-and-forget method
 * (`port_call_id 0`) answers nothing and a failure is only reported. `__release` gives a reference back when the
 * queue reaches it; `__cancel` cancels the running `Job` at once, or drops the call before it starts.
 */
internal class CallbackHost(
    private val registry: UndraCallbacks,
    private val mirror: Mirror,
    private val scope: CoroutineScope,
    private val main: CoroutineDispatcher,
    private val reply: (ByteArray) -> Unit,
    private val report: (error: Throwable, operation: String) -> Unit,
) {
    /** An `async` call in progress: guarded by itself. */
    private class Call {
        var cancelled = false
        var job: Job? = null
    }

    private val bridges = ConcurrentHashMap<Int, UndraCallbackBridge<*>>()
    private val calls = ConcurrentHashMap<Int, Call>()
    private val serial = ConcurrentHashMap<Long, CoroutineDispatcher>()

    init {
        registry.onGone = { instance -> serial.remove(instance.toLong()) }
    }

    fun install(bridge: UndraCallbackBridge<*>) {
        bridges[bridge.portId.toInt()] = bridge
    }

    /**
     * Takes a port call if [portId] is a callback interface's (`null` otherwise): decodes it, queues it and answers
     * [PortOutcome.Async], or [PortOutcome.Unavailable] for a call that does not decode or a method the interface does
     * not have. Never runs app code.
     */
    fun dispatch(portId: UInt, methodId: UInt, portCallId: UInt, args: ByteArray): PortOutcome? {
        val bridge = bridges[portId.toInt()] ?: return null
        val reader = UndraReader(args)
        try {
            val instance = reader.readU64()
            when (methodId) {
                bridge.releaseInstance -> {
                    reader.finish()
                    deliver(bridge, instance, null, 0) { registry.release(instance) }
                }
                bridge.cancelCall -> {
                    val call = reader.readU32()
                    reader.finish()
                    // At once, but off the core's thread: cancelling runs the job's completion handlers.
                    UndraDispatchers.delivery.execute { cancel(call) }
                }
                else -> {
                    val invocation = bridge.invocation(methodId, reader)
                    if (invocation == null) {
                        UndraLog.debug("callback interface ${bridge.name} has no method 0x${methodId.toString(16)}")
                        return PortOutcome.Unavailable
                    }
                    queue(bridge, instance, methodId, portCallId, args.size, invocation)
                }
            }
        } catch (e: Exception) {
            // A call that does not decode (a WireException), or a bridge that failed to decode it.
            UndraDispatchers.delivery.execute { report(e, "${bridge.name} (a call the core sent does not decode)") }
            return PortOutcome.Unavailable
        }
        return PortOutcome.Async
    }

    /** Drops everything: the core is closed, or the connection to it was lost (its proxies answer unavailable). */
    fun clear() {
        for (id in calls.keys.toList()) calls.remove(id)?.let { call -> synchronized(call) { call.cancelled = true; call.job }?.cancel() }
        registry.clear()
        serial.clear()
    }

    private fun queue(
        bridge: UndraCallbackBridge<*>,
        instance: ULong,
        methodId: UInt,
        portCallId: UInt,
        bytes: Int,
        invocation: UndraCallbackInvocation<*>,
    ) {
        @Suppress("UNCHECKED_CAST") // the implementation was lent as this interface; a mismatch fails as a throw
        val any = invocation as UndraCallbackInvocation<Any>
        when (any) {
            is UndraCallbackInvocation.Notify -> {
                val key = if (methodId in bridge.coalesced) CoalesceKey(instance, methodId) else null
                deliver(bridge, instance, key, bytes) { notify(bridge, instance, any) }
            }
            is UndraCallbackInvocation.Ask -> {
                if (portCallId == 0u) {
                    // Called without a reply id: run it, answer nothing.
                    deliver(bridge, instance, null, bytes) { start(bridge, instance, 0u, Call(), any) }
                    return
                }
                val call = Call()
                calls[portCallId.toInt()] = call
                deliver(bridge, instance, null, bytes) { start(bridge, instance, portCallId, call, any) }
            }
        }
    }

    /** Runs [task] where [bridge]'s calls run: in the mirror's drain, or on the instance's serial dispatcher. */
    private fun deliver(bridge: UndraCallbackBridge<*>, instance: ULong, key: Any?, bytes: Int, task: () -> Unit) {
        if (bridge.background) {
            scope.launch(serialOf(instance)) { task() }
        } else {
            mirror.submitInvocation(MirrorInvocation(key, bytes, task))
        }
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private fun serialOf(instance: ULong): CoroutineDispatcher =
        serial.getOrPut(instance.toLong()) { Dispatchers.Default.limitedParallelism(1) }

    private fun notify(bridge: UndraCallbackBridge<*>, instance: ULong, invocation: UndraCallbackInvocation.Notify<Any>) {
        val implementation = registry.implementation(instance)
        if (implementation == null) {
            UndraLog.debug("${bridge.name}.${invocation.method} for instance $instance, which the host no longer holds; dropped")
            return
        }
        try {
            invocation.run(implementation)
        } catch (e: OutOfMemoryError) {
            throw e
        } catch (e: Throwable) {
            report(e, "${bridge.name}.${invocation.method}")
        }
    }

    /** Starts an `async` call: in the drain (undispatched, so it starts in call order) or on the serial dispatcher. */
    private fun start(bridge: UndraCallbackBridge<*>, instance: ULong, portCallId: UInt, call: Call, invocation: UndraCallbackInvocation.Ask<Any>) {
        if (synchronized(call) { call.cancelled }) {
            calls.remove(portCallId.toInt(), call)
            return
        }
        val dispatcher = if (bridge.background) serialOf(instance) else main
        val job = scope.launch(dispatcher, start = CoroutineStart.UNDISPATCHED) {
            val answer = answer(bridge, instance, portCallId, call, invocation)
            calls.remove(portCallId.toInt(), call)
            if (answer != null && portCallId != 0u) reply(answer)
        }
        val cancelNow = synchronized(call) {
            call.job = job
            call.cancelled
        }
        if (cancelNow) job.cancel()
    }

    private suspend fun answer(
        bridge: UndraCallbackBridge<*>,
        instance: ULong,
        portCallId: UInt,
        call: Call,
        invocation: UndraCallbackInvocation.Ask<Any>,
    ): ByteArray? {
        val operation = "${bridge.name}.${invocation.method}"
        val implementation = registry.implementation(instance)
            ?: return PortRegistry.portReply(portCallId, PortStatus.UNAVAILABLE, EMPTY)
        return try {
            PortRegistry.portReply(portCallId, PortStatus.OK, invocation.run(implementation))
        } catch (e: CancellationException) {
            // Cancelled by the core (`__cancel`) or because the core is closing: nobody waits for an answer.
            if (synchronized(call) { call.cancelled } || !scope.isActive) return null
            UndraLog.warn("$operation was cancelled by the implementation; the core is answered unavailable", e)
            PortRegistry.portReply(portCallId, PortStatus.UNAVAILABLE, EMPTY)
        } catch (e: UndraPortException) {
            PortRegistry.portReply(portCallId, PortStatus.ERROR, e.body)
        } catch (e: UndraCallbackGoneException) {
            PortRegistry.portReply(portCallId, PortStatus.UNAVAILABLE, EMPTY)
        } catch (e: OutOfMemoryError) {
            throw e
        } catch (e: Throwable) {
            report(e, operation)
            PortRegistry.portReply(portCallId, PortStatus.UNAVAILABLE, EMPTY)
        }
    }

    private fun cancel(portCallId: UInt) {
        val call = calls.remove(portCallId.toInt()) ?: return
        val job = synchronized(call) {
            call.cancelled = true
            call.job
        }
        job?.cancel()
    }

    private data class CoalesceKey(val instance: ULong, val methodId: UInt)

    private companion object {
        val EMPTY = ByteArray(0)
    }
}
