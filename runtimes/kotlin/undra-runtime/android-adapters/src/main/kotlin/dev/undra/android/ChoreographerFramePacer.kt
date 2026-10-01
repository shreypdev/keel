package dev.undra.android

import android.os.Handler
import android.os.Looper
import android.view.Choreographer
import dev.undra.runtime.FramePacer

/**
 * Paces the mirror's drains by the display (ADR-031): each requested frame runs from a
 * [Choreographer] frame callback on the main thread, at the next vsync, before the frame is drawn.
 *
 * Install it when the core is loaded:
 *
 * ```kotlin
 * UndraCore.load(
 *     LoadOptions(
 *         expectedSchemaHash = UndraIds.SCHEMA_HASH,
 *         mirror = MirrorOptions(framePacer = ChoreographerFramePacer()),
 *     ),
 * )
 * ```
 *
 * Without it the runtime posts its drains on a 60 Hz grid of its own, which is not aligned with the
 * display (and wrong for a 90 or 120 Hz one). Replies, `callSync` on the main thread and `observe` never
 * wait for a frame either way.
 *
 * [requestFrame] may be called from any thread (the core delivers change-sets on its own threads): off
 * the main thread the callback is posted from the main looper, since a [Choreographer] belongs to the
 * thread that obtained it.
 */
public class ChoreographerFramePacer : FramePacer {
    private val mainLooper: Looper = Looper.getMainLooper()
    private val mainHandler = Handler(mainLooper)

    override fun requestFrame(frame: Runnable) {
        if (Looper.myLooper() === mainLooper) {
            postFrameCallback(frame)
        } else {
            mainHandler.post { postFrameCallback(frame) }
        }
    }

    private fun postFrameCallback(frame: Runnable) {
        Choreographer.getInstance().postFrameCallback { frame.run() }
    }
}
