package dev.undra.runtime

import java.util.concurrent.atomic.AtomicBoolean

/**
 * Base class of every generated object: a Kotlin handle to an object that lives in the core.
 *
 * Closing it (`close()`, or `use { }`) releases the core object. As a backstop, a handle whose
 * `UndraObject` is garbage collected without being closed is released by a cleaner
 * (`java.lang.ref.Cleaner`, or an equivalent where that is not available), so a forgotten `close()`
 * costs memory for a while, not forever. Closing twice, or closing and then being collected, releases
 * once.
 *
 * @property core the core the object lives in.
 * @property handle the object's handle in the core's object table (SPEC section 1.2); `0` is the null
 *   handle and is never released.
 */
public abstract class UndraObject(public val core: UndraCore, public val handle: Long) : AutoCloseable {
    private val closed = AtomicBoolean(false)
    private val cleanable: HandleCleaner.Cleanable? =
        if (handle == 0L) null else HandleCleaner.register(this, ReleaseHandle(core, handle))

    /** `true` once [close] has been called. */
    public val isClosed: Boolean get() = closed.get()

    /** Releases the core object. Idempotent. */
    override fun close() {
        if (closed.compareAndSet(false, true)) cleanable?.clean()
    }

    /**
     * The cleanup action: holds the core and the handle but never the object, so it does not keep the
     * object reachable.
     */
    private class ReleaseHandle(private val core: UndraCore, private val handle: Long) : Runnable {
        override fun run() {
            try {
                core.release(handle)
            } catch (e: Exception) {
                UndraLog.warn("releasing the leaked handle $handle failed", e)
            }
        }
    }
}
