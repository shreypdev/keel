package dev.undra.contract

import dev.undra.runtime.UndraDispatchers
import dev.undra.runtime.adapters.UndraPanicReport
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicBoolean

/**
 * What `LoadOptions.onPanic` received (S29): every [UndraPanicReport] with the thread it was delivered on, oldest first. A scenario that
 * counts takes [size] before and compares after, since the core is shared and S17 panics too.
 */
class PanicLog {
    /**
     * One delivery: the report, the name of the thread `onPanic` ran on and whether that was the runtime's main thread
     * ([UndraDispatchers.isMainThread]: the single-thread "main" executor of this runner).
     */
    class Delivery(val report: UndraPanicReport, val thread: String, val onMainThread: Boolean)

    private val deliveries = CopyOnWriteArrayList<Delivery>()
    private val failNext = AtomicBoolean(false)

    /** How many reports have been delivered. */
    val size: Int get() = deliveries.size

    /** The deliveries from the [from]th on. */
    fun since(from: Int): List<Delivery> = deliveries.drop(from)

    /** Makes the next delivery throw after it was recorded, as a crash-reporter bridge that fails would (S29.4). */
    fun throwOnNext() {
        failNext.set(true)
    }

    /** `LoadOptions.onPanic`. */
    fun record(report: UndraPanicReport) {
        deliveries.add(Delivery(report, Thread.currentThread().name, UndraDispatchers.isMainThread()))
        if (failNext.compareAndSet(true, false)) throw IllegalStateException("the crash reporter is down")
    }
}
