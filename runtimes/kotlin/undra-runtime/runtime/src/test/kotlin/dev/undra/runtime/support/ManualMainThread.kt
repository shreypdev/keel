package dev.undra.runtime.support

import dev.undra.runtime.FramePacer
import dev.undra.runtime.MainThread
import dev.undra.runtime.Mirror
import dev.undra.runtime.MirrorOptions
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

/**
 * A frame pacer that only produces a frame when the test says so: [requestFrame] holds the frame,
 * [frame] hands every held frame to [main] and runs the main thread. A test that never calls [frame]
 * is a display that never refreshes (the frame-aligned drain never comes).
 */
internal class ManualFramePacer(private val main: ManualMainThread) : FramePacer {
    private val held = ConcurrentLinkedQueue<Runnable>()

    /** Frames requested so far. */
    val requests = AtomicInteger()

    override fun requestFrame(frame: Runnable) {
        requests.incrementAndGet()
        held.add(frame)
    }

    /** Frames requested and not yet run. */
    val pending: Int get() = held.size

    /** Runs the held frames (and anything else queued) on the calling thread, which becomes the main thread. */
    fun frame() {
        while (true) main.post(held.poll() ?: break)
        main.runPending()
    }
}

/** A mirror on [main] whose frames come from [pacer], as the runtime builds it (resyncs go to [resync]). */
internal fun manualMirror(
    main: ManualMainThread,
    pacer: FramePacer = ManualFramePacer(main),
    maxPendingEntries: Int = 65_536,
    maxPendingBytes: Long = 16L * 1024 * 1024,
    resync: ((Long, UInt) -> Unit)? = null,
): Mirror = Mirror(main, MirrorOptions(pacer, maxPendingEntries, maxPendingBytes), resync)

/** Runs `flush()` on the calling thread, which stands in for [main] during the call: a drain, now. */
internal fun Mirror.flushOnThisThread(main: ManualMainThread) {
    main.mainThread = Thread.currentThread()
    try {
        flush()
    } finally {
        main.mainThread = null
    }
}
