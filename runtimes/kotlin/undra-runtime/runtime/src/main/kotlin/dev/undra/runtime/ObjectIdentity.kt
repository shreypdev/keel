package dev.undra.runtime

import dev.undra.runtime.wire.Handle
import java.lang.ref.Reference
import java.lang.ref.WeakReference

/**
 * One wrapper per handle (ADR-040 decision 7): the identity map behind [UndraCore.adopt].
 *
 * Every handle the core hands out is one reference the host owns. The map remembers, weakly, the wrapper that
 * owns each handle: a handle that comes back while its wrapper lives gives its extra reference back at once and
 * returns that wrapper, so `workshop.shelf("a") === workshop.shelf("a")` and a store returned twice is mirrored
 * once. An entry whose wrapper was closed or collected is replaced by the next adoption; the wrapper's cleaner
 * ([HandleCleaner]) removes its own entry.
 *
 * Wrappers are made under the map's lock (making one is cheap: a store's first observation happens after, in
 * [UndraStore.start]), so two threads that adopt the same handle at once get the same wrapper.
 */
internal class IdentityMap {
    private val lock = Any()
    private val wrappers = HashMap<Long, WeakReference<UndraObject>>()

    fun <T : UndraObject> adopt(core: UndraCore, handle: Long, make: (UndraCore, Long) -> T): T {
        if (handle == 0L) throw UndraProtocolException("the core returned the null handle for an object")
        var made: T? = null
        val live: UndraObject? = synchronized(lock) {
            val existing = wrappers[handle]?.get()
            if (existing != null && !existing.isClosed) {
                existing
            } else {
                val wrapper = try {
                    make(core, handle)
                } catch (e: Throwable) {
                    giveBackQuietly(core, handle)
                    throw e
                }
                wrappers[handle] = wrapper.self
                made = wrapper
                null
            }
        }
        if (live != null) {
            // The core counted one more reference for this crossing; the live wrapper owns one already.
            giveBackQuietly(core, handle)
            @Suppress("UNCHECKED_CAST") // a handle names one object of one type
            return live as T
        }
        val wrapper = made!!
        core.adopted(handle)
        if (wrapper is UndraStore) wrapper.start()
        return wrapper
    }

    /** Removes the entry of [handle] if it is still [ref]'s: what a wrapper's cleaner does before it releases. */
    fun forget(handle: Long, ref: WeakReference<UndraObject>) {
        synchronized(lock) {
            if (wrappers[handle] === ref) wrappers.remove(handle)
        }
    }

    /** Runs [block] (under the map's lock) unless a wrapper that is open still owns [handle]. */
    fun unlessLive(handle: Long, block: () -> Unit) {
        synchronized(lock) {
            val existing = wrappers[handle]?.get()
            if (existing == null || existing.isClosed) block()
        }
    }

    /** The open wrapper of [handle], if there is one. */
    fun live(handle: Long): UndraObject? = synchronized(lock) { wrappers[handle]?.get()?.takeUnless { it.isClosed } }

    /** How many handles have an entry (live or not yet cleaned). */
    val size: Int get() = synchronized(lock) { wrappers.size }

    private fun giveBackQuietly(core: UndraCore, handle: Long) {
        try {
            core.release(handle)
        } catch (e: Exception) {
            UndraLog.warn("giving back the extra reference to ${Handle(handle)} failed", e)
        }
    }
}

/**
 * Keeps [value] reachable up to this point, so that a wrapper whose handle generated code has just sent is not
 * collected, and its handle released, before the call reaches the core (ADR-040 decision 4). The same as
 * `java.lang.ref.Reference.reachabilityFence`, which Android has only from API 28; older releases get an
 * equivalent volatile store.
 */
public fun reachabilityFence(value: Any?) {
    if (Reachability.hasFence) {
        Reference.reachabilityFence(value)
    } else {
        Reachability.sink = value
        Reachability.sink = null
    }
}

private object Reachability {
    val hasFence: Boolean = try {
        Reference::class.java.getMethod("reachabilityFence", Any::class.java)
        true
    } catch (e: NoSuchMethodException) {
        false
    } catch (e: SecurityException) {
        false
    }

    @Volatile
    @JvmField
    var sink: Any? = null
}
