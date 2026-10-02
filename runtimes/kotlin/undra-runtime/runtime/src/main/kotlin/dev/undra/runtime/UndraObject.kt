package dev.undra.runtime

import java.lang.ref.WeakReference
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Base class of every generated object: a Kotlin handle to an object that lives in the core.
 *
 * A core hands out **one wrapper per handle** ([UndraCore.adopt], ADR-040): a method that returns an object the
 * app already holds returns that same wrapper, so wrappers compare by identity (`===`, which `equals` is).
 *
 * Closing it (`close()`, or `use { }`) gives back the one reference to the core object it owns. As a backstop, a
 * handle whose `UndraObject` is garbage collected without being closed is released by a cleaner
 * (`java.lang.ref.Cleaner`, or an equivalent where that is not available), so a forgotten `close()` costs memory
 * for a while, not forever. Closing twice, or closing and then being collected, releases once.
 *
 * @property core the core the object lives in.
 * @property handle the object's handle in the core's object table (SPEC section 1.2); `0` is the null
 *   handle and is never released. A handle means something only to [core] (ADR-044).
 */
public abstract class UndraObject(public val core: UndraCore, public val handle: Long) : AutoCloseable {
    private val closed = AtomicBoolean(false)

    /** What the core's identity map holds for this wrapper (and its cleaner compares). */
    internal val self: WeakReference<UndraObject> = WeakReference(this)

    private val cleanable: HandleCleaner.Cleanable? =
        if (handle == 0L) null else HandleCleaner.register(this, ReleaseHandle(core, handle, self))

    /** `true` once [close] has been called. */
    public val isClosed: Boolean get() = closed.get()

    /** Releases the core object. Idempotent. */
    override fun close() {
        if (closed.compareAndSet(false, true)) cleanable?.clean()
    }

    /**
     * The cleanup action: holds the core, the handle and the wrapper's weak reference but never the object, so it
     * does not keep the object reachable. Runs at most once.
     */
    private class ReleaseHandle(
        private val core: UndraCore,
        private val handle: Long,
        private val self: WeakReference<UndraObject>,
    ) : Runnable {
        override fun run() {
            try {
                core.identity.forget(handle, self)
                core.release(handle)
            } catch (e: Exception) {
                UndraLog.warn("releasing the leaked handle $handle failed", e)
            }
        }
    }
}
