package @@APP_ID@@

import android.app.Application
import @@KOTLIN_PACKAGE@@.UndraIds
import dev.undra.android.ChoreographerFramePacer
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.MirrorOptions
import dev.undra.runtime.UndraCore

/**
 * Attaches the app to its Rust core once per process, before any store is created. The core is
 * `libundra_core.so`, which `undra build --platform android` writes to `build/android/jniLibs`.
 *
 * What the core produces on its own (timers, streams, port completions) is applied to the stores once per
 * display frame, at the display's own frames: [ChoreographerFramePacer] (the `android-adapters` module) hands the
 * mirror each vsync. Without it the runtime drains on a 60 Hz grid of its own, which is not aligned with the display
 * (and wrong for a 90 or 120 Hz one). Replies and `callSync` on the main thread never wait for a frame either way.
 */
class UndraApp : Application() {
    override fun onCreate() {
        super.onCreate()
        UndraCore.load(
            LoadOptions(
                expectedSchemaHash = UndraIds.SCHEMA_HASH,
                mirror = MirrorOptions(framePacer = ChoreographerFramePacer()),
            ),
        )
    }
}
