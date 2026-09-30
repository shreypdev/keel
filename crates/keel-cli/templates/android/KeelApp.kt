package @@APP_ID@@

import android.app.Application
import @@KOTLIN_PACKAGE@@.KeelIds
import dev.keel.runtime.KeelCore
import dev.keel.runtime.LoadOptions

/**
 * Attaches the app to its Rust core once per process, before any store is created. The core is
 * `libkeel_core.so`, which `keel build --platform android` writes to `build/android/jniLibs`.
 */
class KeelApp : Application() {
    override fun onCreate() {
        super.onCreate()
        KeelCore.load(LoadOptions(expectedSchemaHash = KeelIds.SCHEMA_HASH))
    }
}
