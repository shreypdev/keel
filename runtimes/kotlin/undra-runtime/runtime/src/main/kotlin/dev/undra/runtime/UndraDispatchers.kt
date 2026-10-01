package dev.undra.runtime

import java.util.concurrent.Executor
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.ThreadFactory
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.asCoroutineDispatcher
import kotlin.coroutines.EmptyCoroutineContext

/**
 * Where the runtime delivers things to the application (SPEC section 11).
 *
 * [main] is the dispatcher change-sets are applied on: on Android (detected by reflection, so this
 * module does not depend on the Android SDK) it is `Dispatchers.Main.immediate`; on a plain JVM it is a
 * single daemon thread named `undra-main`, standing in for a UI thread.
 */
public object UndraDispatchers {
    private val loop: MainThread by lazy { createMainThread() }

    /** The main-thread dispatcher: `Dispatchers.Main.immediate` on Android, a single thread named `undra-main` elsewhere. */
    public val main: CoroutineDispatcher get() = loop.dispatcher

    /** `true` when the calling thread is the one [main] runs on. */
    public fun isMainThread(): Boolean = loop.isCurrent()

    /** The [MainThread] the default [Mirror] applies change-sets on. */
    internal fun mainThread(): MainThread = loop

    /**
     * A single daemon thread named `undra-delivery`. Core callbacks hand everything that could resume
     * application code (stream items, blocking waiters) to it, so that such code never runs on a core
     * thread while the core lock is held.
     */
    internal val delivery: Executor by lazy {
        Executors.newSingleThreadExecutor(NamedDaemonThreads("undra-delivery"))
    }

    private fun createMainThread(): MainThread {
        val android = AndroidMainThread.create()
        return android ?: ExecutorMainThread()
    }
}

/** A thread on which the mirror applies change-sets. */
internal interface MainThread {
    /** The dispatcher form, for `UndraDispatchers.main`. */
    val dispatcher: CoroutineDispatcher

    /** Runs [task] later on the main thread; never inline, even when already on it. */
    fun post(task: Runnable)

    /** `true` when called on the main thread. */
    fun isCurrent(): Boolean
}

/** A daemon-thread factory with fixed names, so thread dumps show which is which. */
internal class NamedDaemonThreads(private val name: String) : ThreadFactory {
    override fun newThread(r: Runnable): Thread = Thread(r, name).also { it.isDaemon = true }
}

/** The thread of an [ExecutorMainThread]; it remembers its owner so that `isCurrent` survives a replaced worker. */
private class MainWorker(val owner: Any, r: Runnable) : Thread(r, "undra-main") {
    init {
        isDaemon = true
    }
}

/** The JVM fallback: one daemon thread named `undra-main`. */
internal class ExecutorMainThread : MainThread {
    private val executor: ExecutorService = Executors.newSingleThreadExecutor { MainWorker(this, it) }

    override val dispatcher: CoroutineDispatcher = executor.asCoroutineDispatcher()

    override fun post(task: Runnable) {
        executor.execute {
            try {
                task.run()
            } catch (e: Throwable) {
                // A UI thread survives a bad task; so does this stand-in.
                UndraLog.warn("a task on the undra-main thread failed", e)
            }
        }
    }

    override fun isCurrent(): Boolean = (Thread.currentThread() as? MainWorker)?.owner === this
}

/**
 * Android's main thread, reached without a compile-time dependency: `Dispatchers.Main.immediate`
 * (needs `kotlinx-coroutines-android`, which every Android app using coroutines has) plus reflection on
 * `android.os.Looper`, once, to find the main looper's thread for the thread check (which runs on every
 * `callSync`, so it must not reflect).
 */
internal class AndroidMainThread private constructor(
    override val dispatcher: CoroutineDispatcher,
    private val thread: Thread,
) : MainThread {
    override fun post(task: Runnable) {
        dispatcher.dispatch(EmptyCoroutineContext, task)
    }

    override fun isCurrent(): Boolean = Thread.currentThread() === thread

    companion object {
        /** The Android main thread, or `null` when not on Android or when `Dispatchers.Main` is unusable. */
        fun create(): AndroidMainThread? {
            if (!Platform.isAndroid) return null
            return try {
                val looper = Class.forName("android.os.Looper")
                val mainLooper = looper.getMethod("getMainLooper").invoke(null) ?: return null
                val thread = looper.getMethod("getThread").invoke(mainLooper) as? Thread ?: return null
                val immediate = Dispatchers.Main.immediate
                immediate.isDispatchNeeded(EmptyCoroutineContext) // throws when no Main dispatcher is installed
                AndroidMainThread(immediate, thread)
            } catch (e: Exception) {
                UndraLog.warn("Dispatchers.Main is not usable; falling back to a background thread for change-sets", e)
                null
            }
        }
    }
}
