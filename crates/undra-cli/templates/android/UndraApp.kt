package @@APP_ID@@

import android.app.Application
import dev.undra.android.AndroidPlatformDefaults
import dev.undra.android.ChoreographerFramePacer
import @@KOTLIN_PACKAGE@@.UndraIds
import dev.undra.runtime.UndraCore
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.MirrorOptions

/**
 * Attaches the app to its Rust core once per process, before any store is created. The core is
 * `libundra_core.so`, which `undra build --platform android` writes to `build/android/jniLibs`.
 *
 * `AndroidPlatformDefaults.install` gives the core every platform capability in one call: `Http` over
 * `HttpURLConnection`, `Kv` and `Fs` in the app's files, `SecureStore` under an Android Keystore key, and the
 * `Connectivity` and `Lifecycle` events. They need the `INTERNET` and `ACCESS_NETWORK_STATE` permissions of the manifest.
 */
class UndraApp : Application() {
    override fun onCreate() {
        super.onCreate()
        val core = UndraCore.load(
            LoadOptions(
                expectedSchemaHash = UndraIds.SCHEMA_HASH,
                // Apply what the core produces on its own once per display frame.
                mirror = MirrorOptions(framePacer = ChoreographerFramePacer()),
            ),
        )
        AndroidPlatformDefaults.install(core, this)
    }
}
