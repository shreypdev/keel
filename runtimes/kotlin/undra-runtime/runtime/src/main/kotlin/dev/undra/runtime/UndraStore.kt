package dev.undra.runtime

import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.Payloads.ChangeOp
import java.lang.ref.WeakReference
import kotlinx.coroutines.flow.MutableStateFlow

/**
 * Base class of every generated store: an object whose signals the core mirrors into `StateFlow`s.
 *
 * Generated subclasses declare one `StateFlow` per signal, created with [signal] and updated by their
 * [apply], which the runtime calls **on the main thread** ([UndraDispatchers.main]) for the changes the
 * core reports, after the subclass has called `core.observe(handle, ...)`. Changes that arrive between two
 * display frames are merged first (see [Mirror]): a signal set many times is applied once with its last
 * value, and its keyed patches are applied as one patch. The constructor registers the store with
 * `core.mirror`; [close] (or the cleaner backstop, see [UndraObject]) unregisters it.
 *
 * The mirror holds the store weakly: keep a reference to the store for as long as you use its flows. A
 * flow on its own does not keep the store, and with it the core object, alive.
 *
 * @param noCoalesce the ids of the store's signals declared `#[undra(no_coalesce)]`: the mirror applies
 *   every value of those, in order, instead of only the last one per frame. A `StateFlow` conflates by
 *   design, so a collector may still miss values set in quick succession; read the flow's `value` (or use
 *   a callback) where every step matters.
 */
public abstract class UndraStore(core: UndraCore, handle: Long, noCoalesce: Set<UInt> = emptySet()) : UndraObject(core, handle) {

    /**
     * Applies one change to the signal [signalId]. Called on the main thread. [reader] is limited to
     * the change's bytes and valid only during the call; decode it and update the signal's flow. For
     * [ChangeOp.FULL] the reader holds the whole value, for [ChangeOp.PATCH] a keyed patch (SPEC 3.8) and
     * for [ChangeOp.INVALIDATED] nothing. Signal ids a subclass does not know must be ignored.
     */
    protected abstract fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader)

    /**
     * Starts observing every signal, so the core reports their current values (applied before this returns when
     * the core is in process). Generated stores call it from `init`. If the core cannot be reached the store is
     * closed (no handle leaks) and the failure is thrown as an [UndraCallError].
     *
     * @throws UndraCallError if the core is closed or unreachable.
     */
    protected fun observeAll() {
        try {
            core.observe(handle, UInt.MAX_VALUE, true)
        } catch (e: Exception) {
            close()
            throw UndraCallError.mapped(e)
        }
    }

    /** Creates the [MutableStateFlow] backing one signal, holding [initial] until the core reports the real value. */
    protected fun <T> signal(initial: T): MutableStateFlow<T> = MutableStateFlow(initial)

    init {
        val self = WeakReference(this)
        val route: (UInt, ChangeOp, UndraReader) -> Unit = { signalId, op, reader -> self.get()?.apply(signalId, op, reader) }
        // Without no_coalesce signals the two-argument registration, the one a test double's mirror overrides.
        if (noCoalesce.isEmpty()) core.mirror.register(handle, route) else core.mirror.register(handle, noCoalesce, route)
    }
}
