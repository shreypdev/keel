package dev.undra.work

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.Operation
import androidx.work.WorkManager
import dev.undra.runtime.UndraCore
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * Background runs of the core on Android (ADR-046): registers how a process WorkManager starts loads the core, and asks WorkManager for a
 * window when the core has work to drain. The job itself is [UndraWorker].
 *
 * ```kotlin
 * class MyApp : Application() {
 *     override fun onCreate() {
 *         super.onCreate()
 *         // Once, before anything can schedule: how to get the core in a process no activity opened. Idempotent: it returns
 *         // the core that is already loaded, or loads it (the generated `Undra<Namespace>.load` throws when it is loaded twice).
 *         UndraWork.configure(loader = { context -> (context.applicationContext as MyApp).core() })
 *     }
 * }
 *
 * // At app start: when the app goes to the background with work pending, ask for a window.
 * AndroidPlatformDefaults.install(core, app, onBackgroundWorkPending = { UndraWork.schedule(app) })
 * ```
 *
 * [schedule] enqueues **unique** work (one job however many times it is called) that waits for a network, so a window is only spent when
 * the offline queue can be replayed. WorkManager decides when it runs; the core never assumes a window and keeps its progress per item.
 */
public object UndraWork {
    /** The name of the unique work [schedule] enqueues ([androidx.work.WorkManager.getWorkInfosForUniqueWork] finds it by this). */
    public const val UNIQUE_WORK_NAME: String = "dev.undra.background"

    private val registered = AtomicReference<((Context) -> UndraCore)?>(null)

    /** The registered loader, or `null` before [configure]. */
    internal val loader: ((Context) -> UndraCore)? get() = registered.get()

    /**
     * Registers how [UndraWorker] gets the core. WorkManager may start the app's process just to run the job, with no activity and
     * no earlier load, so the app says how to load its core here, in `Application.onCreate`. Calling it again replaces the loader.
     *
     * The loader runs on a background thread of WorkManager's, once per run. It must be idempotent: **return the core that is already
     * loaded** when the process is warm (loading it twice is an error), and load it (with its platform adapters, `AndroidPlatformDefaults.install`)
     * when it is not. A loader that throws makes the run end with `Result.retry()`.
     *
     * @param loader gets the core for the application context it is given.
     */
    public fun configure(loader: (Context) -> UndraCore) {
        registered.set(loader)
    }

    /**
     * Asks WorkManager to run the core's background tasks when the device has a network: enqueues [UndraWorker] as **unique** work named
     * [UNIQUE_WORK_NAME] with `NetworkType.CONNECTED` and the `KEEP` policy, so calling it again while a job is waiting or running adds
     * nothing. A job that returned `retry` is run again by WorkManager with exponential back-off from 30 seconds.
     *
     * Call it when the app goes to the background with pending work ([dev.undra.runtime.UndraStats.background]; `AndroidPlatformDefaults.install`'s
     * `onBackgroundWorkPending` does) or after a mutation was queued offline ([scheduleIfPending]).
     *
     * @throws IllegalStateException if WorkManager is not initialized (an app that disabled its default initializer must initialize it first).
     */
    public fun schedule(context: Context): Operation {
        val request = OneTimeWorkRequestBuilder<UndraWorker>()
            .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, BACKOFF_SECONDS, TimeUnit.SECONDS)
            .build()
        return WorkManager.getInstance(context.applicationContext).enqueueUniqueWork(UNIQUE_WORK_NAME, ExistingWorkPolicy.KEEP, request)
    }

    /**
     * [schedule]s the job when [core] says a background window has work to drain (`stats().background.pending > 0`: queued offline
     * mutations, stale persisted queries, unflushed persistence) and does nothing otherwise. For an app that knows it just queued a
     * mutation while offline, which will have to outlive its process. Cheap enough for the main thread.
     *
     * @param core the core to ask; the shared one by default.
     * @return whether a job was requested.
     */
    public fun scheduleIfPending(context: Context, core: UndraCore = UndraCore.shared): Boolean {
        if (!core.stats().background.hasPendingWork) return false
        schedule(context)
        return true
    }

    /** Cancels the job [schedule] enqueued, if one is waiting or running. */
    public fun cancel(context: Context): Operation = WorkManager.getInstance(context.applicationContext).cancelUniqueWork(UNIQUE_WORK_NAME)

    /** Forgets the loader (tests). */
    internal fun reset() {
        registered.set(null)
    }

    private const val BACKOFF_SECONDS: Long = 30L
}
