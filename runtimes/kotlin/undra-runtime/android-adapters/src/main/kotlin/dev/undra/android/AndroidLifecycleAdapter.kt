package dev.undra.android

import android.app.Activity
import android.app.ActivityManager
import android.app.Application
import android.content.Context
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import dev.undra.runtime.UndraCore
import dev.undra.runtime.adapters.AppState
import dev.undra.runtime.adapters.LifecycleEvents

/**
 * The state machine behind [AndroidLifecycleAdapter], kept apart from Android so that JVM unit tests can drive it.
 *
 * It counts the app's started and resumed activities. With one resumed the app is [AppState.ACTIVE]; with none resumed
 * but one started (a dialog, a permission prompt or another window has the focus) it is [AppState.INACTIVE]; with none
 * started it is [AppState.BACKGROUND]. Becoming active is reported at once; any other change is reported only if it
 * lasts for [settleMs], so that a rotation, one activity giving way to another, or the `onStart` that comes a moment
 * before `onResume` does not look like the app leaving and coming back (`androidx.lifecycle.ProcessLifecycleOwner`
 * waits 700 ms for the same reason).
 */
internal class ProcessStateMachine(
    private val settleMs: Long,
    private val schedule: (delayMs: Long, task: Runnable) -> Cancel,
    initial: AppState,
    private val emit: (AppState) -> Unit,
) {
    /** Cancels a scheduled task. */
    fun interface Cancel {
        /** Prevents the task from running if it has not run yet. */
        fun cancel()
    }

    private var started = 0
    private var resumed = 0
    private var pending: Cancel? = null
    private var disposed = false

    /** The state last reported (or the initial one). */
    var state: AppState = initial
        private set

    fun onStarted() {
        started++
        reevaluate()
    }

    fun onResumed() {
        resumed++
        reevaluate()
    }

    fun onPaused() {
        if (resumed > 0) resumed--
        reevaluate()
    }

    fun onStopped() {
        if (started > 0) started--
        reevaluate()
    }

    /** Drops a scheduled report and stops reporting. */
    fun dispose() {
        pending?.cancel()
        pending = null
        disposed = true
    }

    private fun target(): AppState =
        when {
            resumed > 0 -> AppState.ACTIVE
            started > 0 -> AppState.INACTIVE
            else -> AppState.BACKGROUND
        }

    private fun reevaluate() {
        pending?.cancel()
        pending = null
        val wanted = target()
        if (wanted == state) return
        if (wanted == AppState.ACTIVE) {
            apply(wanted)
        } else {
            pending = schedule(settleMs) {
                pending = null
                val now = target() // what holds after the settling time, not what was true when it started
                if (now != state) apply(now)
            }
        }
    }

    private fun apply(next: AppState) {
        if (disposed) return
        state = next
        emit(next)
    }
}

/**
 * The `Lifecycle` event port over the app's activities: tells the core when the app is in the foreground and receiving
 * input ([AppState.ACTIVE]), in the foreground without the focus ([AppState.INACTIVE]) or not visible
 * ([AppState.BACKGROUND]). The core refetches the observed queries that went stale while the app was away when it
 * becomes active (SPEC section 9).
 *
 * It listens with `Application.ActivityLifecycleCallbacks` rather than `ProcessLifecycleOwner`, which would add
 * `androidx.lifecycle:lifecycle-process` to every app for a dozen lines of counting. It reports the state when it is
 * started (the process importance says whether the app is in the foreground, so an app started by a notification or a
 * `WorkManager` job is background), then every change, on the main thread. A change to a less active state is reported
 * after 700 ms if it lasted, so rotating the device or opening another activity of the app is not a trip to the
 * background. There is no "terminate" state: `AppState` has none, and Android ends a process without telling it; the
 * offline queue and the query cache are persisted as they change (`Kv`), so nothing has to be saved at exit.
 *
 * ```kotlin
 * val lifecycle = AndroidLifecycleAdapter(context)
 * lifecycle.attach(UndraCore.shared)
 * ```
 *
 * [AndroidPlatformDefaults.install] does this for you.
 *
 * @param context any context of the app.
 * @param settleMs how long a move to a less active state must last before it is reported.
 */
public class AndroidLifecycleAdapter(context: Context, private val settleMs: Long = DEFAULT_SETTLE_MS) : AutoCloseable {
    private val application: Application = context.applicationContext as Application
    private val main = Handler(Looper.getMainLooper())
    private var machine: ProcessStateMachine? = null
    private var callbacks: Application.ActivityLifecycleCallbacks? = null

    init {
        require(settleMs >= 0) { "settleMs must not be negative, got $settleMs" }
    }

    /** The state now: the last one reported, or what the process importance says if the adapter is not started. */
    public val state: AppState
        get() = machine?.state ?: initialState()

    /**
     * Starts reporting to [listener] on the main thread: the current state first, then every change. Calling it again
     * replaces the previous listener.
     */
    public fun start(listener: (AppState) -> Unit) {
        close()
        val initial = initialState()
        val fresh = ProcessStateMachine(
            settleMs = settleMs,
            schedule = { delay, task ->
                main.postDelayed(task, delay)
                ProcessStateMachine.Cancel { main.removeCallbacks(task) }
            },
            initial = initial,
            emit = { state ->
                try {
                    listener(state)
                } catch (e: Exception) {
                    Log.w(TAG, "a Lifecycle listener failed", e)
                }
            },
        )
        val registered = object : Application.ActivityLifecycleCallbacks {
            override fun onActivityStarted(activity: Activity) = fresh.onStarted()

            override fun onActivityResumed(activity: Activity) = fresh.onResumed()

            override fun onActivityPaused(activity: Activity) = fresh.onPaused()

            override fun onActivityStopped(activity: Activity) = fresh.onStopped()

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit

            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit

            override fun onActivityDestroyed(activity: Activity) = Unit
        }
        machine = fresh
        callbacks = registered
        application.registerActivityLifecycleCallbacks(registered)
        listener(initial)
    }

    /** Starts reporting to [core] as `Lifecycle.changed(state)` events; see [start]. */
    public fun attach(core: UndraCore) {
        val events = LifecycleEvents(core)
        start { state ->
            try {
                events.changed(state)
            } catch (e: Exception) {
                Log.w(TAG, "could not report the lifecycle state to the core", e)
            }
        }
    }

    /** Stops reporting. Idempotent. */
    override fun close() {
        callbacks?.let(application::unregisterActivityLifecycleCallbacks)
        callbacks = null
        machine?.dispose()
        machine = null
    }

    /** Foreground when the process is, as far as Android's importance ranking says; background otherwise. */
    private fun initialState(): AppState {
        val info = ActivityManager.RunningAppProcessInfo()
        ActivityManager.getMyMemoryState(info)
        return if (info.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND) AppState.ACTIVE else AppState.BACKGROUND
    }

    /** Defaults. */
    public companion object {
        /** How long a move to a less active state must last before it is reported: 700 ms. */
        public const val DEFAULT_SETTLE_MS: Long = 700L

        private const val TAG = "Undra"
    }
}
