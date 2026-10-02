package dev.undra.work

import android.util.Log
import dev.undra.runtime.UndraCore
import kotlin.coroutines.cancellation.CancellationException

/** How a background window ended, in WorkManager's terms: [SUCCESS] when the core drained everything, [RETRY] when something is left or went wrong. */
internal enum class WindowOutcome {
    /** Every background task finished before the deadline. */
    SUCCESS,

    /** Work is still pending, or the run failed: WorkManager runs the job again after its back-off. */
    RETRY,
}

/**
 * What a worker does with its window, kept apart from WorkManager so that JVM unit tests can drive it (ADR-046, decision 3.4):
 * how long the core may take, and how the core's answer maps onto a result.
 */
internal object BackgroundWindow {
    /** WorkManager stops a worker after ten minutes. */
    const val WORKER_LIMIT_MS: Long = 10 * 60_000L

    /** The window the worker asks the core for: nine of those ten minutes, the last one is the OS's and the app's. */
    const val WINDOW_MS: Long = 9 * 60_000L

    /** Kept back from [WINDOW_MS] for loading the core, handing back the result and the clock's slack. */
    const val MARGIN_MS: Long = 15_000L

    /** The shortest deadline a run is given, even when loading the core used up the window (the core returns half a second before it). */
    const val MIN_DEADLINE_MS: Long = 1_000L

    /**
     * The deadline, in milliseconds from now, to give `runInBackground` when [elapsedMs] of the worker's time went before it:
     * [WINDOW_MS] less [MARGIN_MS] less what was spent, at least [MIN_DEADLINE_MS].
     */
    fun deadlineMs(elapsedMs: Long, windowMs: Long = WINDOW_MS, marginMs: Long = MARGIN_MS): Long =
        (windowMs - marginMs - elapsedMs.coerceAtLeast(0L)).coerceAtLeast(MIN_DEADLINE_MS)

    /**
     * Runs the core's background tasks for at most [deadlineMs] and maps what it says: [WindowOutcome.SUCCESS] when it finished,
     * [WindowOutcome.RETRY] when it did not or the run failed with anything but a cancellation (an `UndraCallError`: the core is closed,
     * refused or cancelled the call). A cancellation (WorkManager stopped the worker) propagates, after the coroutine has cancelled
     * the call in the core: what was done is kept, and the job is run again by WorkManager. It never throws anything else, so
     * nothing escapes into an OS callback.
     */
    suspend fun run(core: UndraCore, deadlineMs: Long): WindowOutcome =
        try {
            val report = core.runInBackground(deadlineMs)
            Log.i(TAG, "background run: finished=${report.finished}, replayed=${report.replayed}, refetched=${report.refetched}, stillPending=${report.stillPending}")
            if (report.finished) WindowOutcome.SUCCESS else WindowOutcome.RETRY
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "the background run failed; WorkManager will retry it", e)
            WindowOutcome.RETRY
        }

    internal const val TAG: String = "UndraWork"
}
