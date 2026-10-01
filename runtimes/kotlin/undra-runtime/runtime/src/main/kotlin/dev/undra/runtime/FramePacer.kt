package dev.undra.runtime

import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

/**
 * When the [Mirror] applies what the core produced on its own: once per display frame (ADR-031,
 * SPEC section 11).
 *
 * The mirror asks for a frame when the first change-set arrives after a drain, and applies
 * everything that arrived meanwhile, merged, when the frame runs. Replies, `callSync` on the main
 * thread and `observe` never wait for it.
 *
 * Android apps install `dev.undra.android.ChoreographerFramePacer` (module `android-adapters`) through
 * [MirrorOptions.framePacer]; without one the runtime paces itself on a 60 Hz grid.
 *
 * ```kotlin
 * UndraPlaygroundCore.load(LoadOptions(mirror = MirrorOptions(framePacer = ChoreographerFramePacer()))) // the generated entry of your core
 * ```
 */
public fun interface FramePacer {
    /** Runs [frame] once, on the main thread, at the next display frame. Called from any thread; the mirror keeps at most one request outstanding. */
    public fun requestFrame(frame: Runnable)
}

/**
 * The pacer the runtime uses when the app installs none: a single daemon thread named `undra-frame`
 * waits for the next tick of a fixed grid ([intervalNanos] apart, counted from this pacer's
 * creation with `System.nanoTime`) and posts the frame to [main].
 */
internal class PacedFramePacer(
    private val main: MainThread,
    private val intervalNanos: Long = DEFAULT_INTERVAL_NANOS,
) : FramePacer {
    private val origin = System.nanoTime()

    init {
        require(intervalNanos > 0) { "the frame interval must be positive, got $intervalNanos ns" }
    }

    override fun requestFrame(frame: Runnable) {
        val elapsed = System.nanoTime() - origin
        val delay = intervalNanos - Math.floorMod(elapsed, intervalNanos)
        ticker.schedule({ main.post(frame) }, delay, TimeUnit.NANOSECONDS)
    }

    companion object {
        /** One frame at 60 Hz. */
        const val DEFAULT_INTERVAL_NANOS: Long = 16_666_667L

        /** Shared by every paced pacer: one thread for the whole process, however many cores are attached. */
        private val ticker: ScheduledExecutorService by lazy {
            Executors.newSingleThreadScheduledExecutor(NamedDaemonThreads("undra-frame"))
        }
    }
}
