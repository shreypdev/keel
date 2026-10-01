package @@APP_ID@@

import android.app.Application
import @@KOTLIN_PACKAGE@@.UndraIds
import dev.undra.runtime.UndraCore
import dev.undra.runtime.LoadOptions

/**
 * Attaches the app to its Rust core once per process, before any store is created. The core is
 * `libundra_core.so`, which `undra build --platform android` writes to `build/android/jniLibs`.
 */
class UndraApp : Application() {
    override fun onCreate() {
        super.onCreate()
        UndraCore.load(LoadOptions(expectedSchemaHash = UndraIds.SCHEMA_HASH))
    }
}
