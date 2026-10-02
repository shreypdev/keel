package dev.undra.work

import android.content.Context
import android.os.SystemClock
import android.util.Log
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters

/**
 * The WorkManager job that gives the core a background window (ADR-046): it loads the core with the application context, runs its
 * background tasks (replay the offline queue, refetch stale persisted queries, flush persistence) for up to nine minutes less a margin
 * (WorkManager stops a worker at ten), and reports whether they all finished.
 *
 * Apps do not create it: [UndraWork.schedule] enqueues it, and WorkManager instantiates it by class name, possibly in a process that no
 * activity has opened, which is why the app registers how to load its core with [UndraWork.configure] first (in `Application.onCreate`).
 *
 * | The core... | The worker returns |
 * |---|---|
 * | finished every task before the deadline | `Result.success()` |
 * | did not finish (the deadline passed, or a task is still waiting for the network) | `Result.retry()`: WorkManager runs it again after its back-off, with the network constraint |
 * | could not be loaded, or the run failed (the core was closed, refused the call) | `Result.retry()` |
 * | has no loader registered ([UndraWork.configure] was not called) | `Result.failure()`: retrying cannot help |
 *
 * When WorkManager stops the worker (the ten minutes passed, the constraints no longer hold, the OS needs the process) the coroutine
 * is cancelled and so is the call into the core: what the run already did is kept (the offline queue persists per item), and
 * the job runs again. Nothing here throws into WorkManager.
 */
public class UndraWorker(appContext: Context, params: WorkerParameters) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        val started = SystemClock.elapsedRealtime()
        val loader = UndraWork.loader
        if (loader == null) {
            Log.e(
                TAG,
                "UndraWorker ran but no core loader is registered: call UndraWork.configure(loader = { context -> ... }) in " +
                    "Application.onCreate, so that a process WorkManager starts on its own can load the core",
            )
            return Result.failure()
        }
        val core = try {
            loader(applicationContext)
        } catch (e: Exception) {
            Log.w(TAG, "the core could not be loaded for the background run; WorkManager will retry it", e)
            return Result.retry()
        }
        val deadlineMs = BackgroundWindow.deadlineMs(SystemClock.elapsedRealtime() - started)
        return when (BackgroundWindow.run(core, deadlineMs)) {
            WindowOutcome.SUCCESS -> Result.success()
            WindowOutcome.RETRY -> Result.retry()
        }
    }

    private companion object {
        const val TAG = BackgroundWindow.TAG
    }
}
