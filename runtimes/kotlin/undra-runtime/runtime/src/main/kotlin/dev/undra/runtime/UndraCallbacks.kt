package dev.undra.runtime

import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraReader
import java.util.IdentityHashMap
import kotlin.coroutines.cancellation.CancellationException

/**
 * The app objects a core holds as callback instances (ADR-041): [UndraCore.callbacks].
 *
 * When generated code passes an implementation of a callback interface (`Reporter`, `UploadListener`) to the core,
 * it [lend]s it here and sends the **instance handle** this returns: a non-zero number from a per-core counter,
 * never reused. Every crossing is one reference the core now owns, and the registry holds the implementation
 * **strongly** until the core has given every reference back ([release], what the core's `__release` does), so a
 * listener created inline keeps working. Lending the same object again (by identity, `===`) returns the same handle
 * and counts one more reference.
 *
 * A call that the core refused (status 5) or that never reached it transferred nothing: generated code takes its
 * reference back with [giveBackIfRefused]. A disconnect from `undra dev` and closing the core drop every entry.
 *
 * Thread-safe. App code reads it for diagnostics and tests ([liveCount], [count]); generated code calls the rest.
 */
public class UndraCallbacks internal constructor() {
    private class Entry(val implementation: Any, var count: Int)

    private val lock = Any()
    private val entries = HashMap<Long, Entry>()
    private val instances = IdentityHashMap<Any, Long>()
    private var next = 1L

    /** Instances below this were dropped by [clear]: a late release of one is expected, not an error. */
    private var clearedBelow = 1L

    /** Told when an instance's last reference is gone (the host drops what it kept for it). */
    @Volatile
    internal var onGone: ((ULong) -> Unit)? = null

    /**
     * Lends [implementation] to the core for one crossing and returns its instance handle: the handle it already
     * has if it is lent (one more reference), or a new one.
     */
    public fun lend(implementation: Any): ULong = synchronized(lock) {
        val existing = instances[implementation]
        if (existing != null) {
            val entry = entries.getValue(existing)
            if (entry.count == Int.MAX_VALUE) {
                UndraLog.error("callback instance ${existing.toULong()} was lent ${Int.MAX_VALUE} times; the count saturates")
            } else {
                entry.count++
            }
            existing.toULong()
        } else {
            val instance = next++
            entries[instance] = Entry(implementation, 1)
            instances[implementation] = instance
            instance.toULong()
        }
    }

    /** Takes back one reference that a call did not hand to the core (it was refused, or never sent). */
    public fun giveBack(instance: ULong) {
        drop(instance, "given back")
    }

    /**
     * [giveBack]s [instance] when [error], what a call that carried it failed with, shows that the core did not
     * take the reference: the core refused the call (status 5), or it never reached the core (a closed core, a lost
     * connection, a failure before sending). A failure that the core produced after it read the arguments (its own
     * error, a panic, a cancellation, a timeout) leaves the reference with the core. `null` does nothing.
     */
    public fun giveBackIfRefused(error: Throwable, instance: ULong?) {
        if (instance != null && !coreTookArguments(error)) giveBack(instance)
    }

    /** Gives back one reference the core held: what the core's `__release` does. */
    public fun release(instance: ULong) {
        drop(instance, "released")
    }

    /** How many implementations the core holds references to. */
    public val liveCount: Int get() = synchronized(lock) { entries.size }

    /** How many references the core holds to [implementation]; `0` when it holds none. */
    public fun count(of: Any): Int = synchronized(lock) {
        instances[of]?.let { entries[it]?.count } ?: 0
    }

    /** The instance handle [implementation] is lent as, or `null`. */
    public fun instanceOf(implementation: Any): ULong? = synchronized(lock) { instances[implementation]?.toULong() }

    /** The implementation lent as [instance], or `null` when the core holds no reference to it. */
    internal fun implementation(instance: ULong): Any? = synchronized(lock) { entries[instance.toLong()]?.implementation }

    /** Drops every entry: the core that held them is gone (closed). A lost connection to `undra dev` keeps them, for the session's return. */
    internal fun clear() {
        val gone = synchronized(lock) {
            val out = entries.keys.toList()
            entries.clear()
            instances.clear()
            clearedBelow = next
            out
        }
        for (instance in gone) onGone?.invoke(instance.toULong())
    }

    private fun drop(instance: ULong, what: String) {
        var gone = false
        synchronized(lock) {
            val key = instance.toLong()
            val entry = entries[key]
            if (entry == null) {
                if (key in 1 until clearedBelow) {
                    UndraLog.debug("callback instance $instance $what after its core went away; nothing to do")
                } else {
                    UndraLog.error("callback instance $instance $what more often than it was lent; ignored (an over-release never frees early)")
                }
                return
            }
            entry.count--
            if (entry.count == 0) {
                entries.remove(key)
                instances.remove(entry.implementation)
                gone = true
            }
        }
        if (gone) onGone?.invoke(instance)
    }

    private companion object {
        /** Whether a call that failed with [error] got as far as the core reading its arguments. */
        fun coreTookArguments(error: Throwable): Boolean = when (error) {
            is CancellationException -> true
            is UndraReplyException -> error.status != ReplyStatus.BAD_REQUEST
            is UndraProtocolException -> true
            is UndraTransportException ->
                error.reason == UndraTransportException.Reason.TIMEOUT || error.reason == UndraTransportException.Reason.INTERRUPTED
            is UndraCallError.Refused -> false
            is UndraCallError.Unavailable -> coreTookArguments(error.transport)
            is UndraCallError -> true
            else -> false
        }
    }
}

/**
 * How the runtime runs the core's calls into one callback interface (ADR-041). Generated code declares one object
 * per `#[undra::callback]` trait (`ReporterBridge`) and its core entry registers them at load, before the core
 * starts; apps never use this.
 *
 * @property name the interface's name, as reports name it (`Reporter.confirm`).
 * @property portId the interface's port id.
 * @property releaseInstance the id of its reserved `__release` method.
 * @property cancelCall the id of its reserved `__cancel` method.
 * @property background `true` for a `#[undra::callback(background)]` interface: its methods run on a serial
 *   executor per instance instead of the main thread's drain.
 * @property coalesced the methods marked `#[undra(coalesce)]`: of their pending calls only the newest per instance
 *   runs in a drain.
 */
public abstract class UndraCallbackBridge<T : Any>(
    public val name: String,
    public val portId: UInt,
    public val releaseInstance: UInt,
    public val cancelCall: UInt,
    public val background: Boolean = false,
    public val coalesced: Set<UInt> = emptySet(),
) {
    /**
     * Decodes the arguments of a call of [methodId] from [args] (positioned after the instance handle) and returns
     * what to run with the implementation, or `null` for a method the interface does not have.
     *
     * @throws dev.undra.runtime.wire.WireException if the arguments do not decode.
     */
    public abstract fun invocation(methodId: UInt, args: UndraReader): UndraCallbackInvocation<T>?

    override fun toString(): String = "UndraCallbackBridge($name, port 0x${portId.toString(16)})"
}

/**
 * One call of the core into an implementation, decoded and ready to run: what an [UndraCallbackBridge] returns.
 *
 * @property method the method's Kotlin name, for reports (`Reporter.note`).
 */
public sealed class UndraCallbackInvocation<in T : Any>(public val method: String) {
    /** A fire-and-forget method: [run] calls it; what it throws is reported and goes nowhere else. */
    public class Notify<in T : Any>(method: String, public val run: (T) -> Unit) : UndraCallbackInvocation<T>(method)

    /**
     * An `async` method: [run] calls it and returns the encoded `Ok` value. The method's own error is thrown as an
     * [UndraPortException] carrying the encoded error (answered with status 1); anything else is reported and
     * answered unavailable (status 2), as is [UndraCallbackGoneException].
     */
    public class Ask<in T : Any>(method: String, public val run: suspend (T) -> ByteArray) : UndraCallbackInvocation<T>(method)
}

/**
 * Thrown by a weak callback wrapper (`Reporter.weak(target)`) whose target was collected: the core is answered
 * unavailable, and nothing is reported (a weak listener that went away is not a failure).
 */
public class UndraCallbackGoneException(interfaceName: String) :
    UndraException("the $interfaceName this weak wrapper forwarded to is gone")
