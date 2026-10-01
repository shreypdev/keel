package dev.undra.runtime.support

import dev.undra.runtime.MainThread
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicInteger

/**
 * A main thread that only runs when the test says so: `post` queues, [runPending] drains on the calling
 * thread, which is then "the main thread" for [isCurrent]. It makes batching and ordering deterministic.
 */
internal class ManualMainThread : MainThread {
    private val queue = ConcurrentLinkedQueue<Runnable>()

    @Volatile var mainThread: Thread? = null
    val posted = AtomicInteger()

    override val dispatcher: CoroutineDispatcher get() = Dispatchers.Unconfined

    override fun post(task: Runnable) {
        posted.incrementAndGet()
        queue.add(task)
    }

    override fun isCurrent(): Boolean = Thread.currentThread() === mainThread

    /** Number of tasks waiting. */
    val pending: Int get() = queue.size

    /** Runs the queued tasks (and any they queue) on the calling thread, which becomes the main thread. */
    fun runPending() {
        mainThread = Thread.currentThread()
        try {
            while (true) (queue.poll() ?: break).run()
        } finally {
            mainThread = null
        }
    }
}
