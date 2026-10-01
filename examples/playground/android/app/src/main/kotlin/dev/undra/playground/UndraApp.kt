package dev.undra.playground

import android.app.Application
import dev.undra.android.AndroidPlatformDefaults
import dev.undra.android.ChoreographerFramePacer
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.RemoteConfig
import dev.undra.playground.core.configureRemote
import dev.undra.playground.remote.DemoServer
import dev.undra.runtime.UndraCore
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.MirrorOptions
import dev.undra.runtime.adapters.ConnectivityEvents

/**
 * Attaches the app to its Rust core once per process, before any store is created.
 *
 * The core is `libundra_core.so`, which `undra build --platform android` writes to `build/android/jniLibs`;
 * `UndraCore.load` checks that it was built from the same schema as the bindings (`UndraIds.SCHEMA_HASH`).
 * `AndroidPlatformDefaults.install` then gives it every platform port, none of them faked: `Http` over
 * `HttpURLConnection`, `Kv` and `Fs` in the app's files, `SecureStore` under an Android Keystore key, `Connectivity`
 * from `ConnectivityManager`, `Lifecycle` from the app's activities. The persisted query cache and the offline queue
 * therefore survive the process being killed.
 *
 * The Remote tab needs a server, and the playground has none to ship: [DemoServer] is a small HTTP server on the
 * device's loopback interface, so the core's requests still travel through the real `Http` adapter and a real socket.
 *
 * The mirror applies what the core produces on its own (the 10k list's streamed updates) once per display frame,
 * through the [ChoreographerFramePacer] of `android-adapters` (ADR-031).
 */
class UndraApp : Application() {
    /** The server behind the Remote tab; the Offline switch makes it drop connections. */
    val server by lazy { DemoServer(this) }

    /** Sends `Connectivity.changed` to the core; the Remote tab's Offline switch calls it to simulate losing the network. */
    val connectivity: ConnectivityEvents by lazy { ConnectivityEvents(UndraCore.shared) }

    override fun onCreate() {
        super.onCreate()
        val core = UndraCore.load(
            LoadOptions(
                expectedSchemaHash = UndraIds.SCHEMA_HASH,
                mirror = MirrorOptions(framePacer = ChoreographerFramePacer()),
            ),
        )
        AndroidPlatformDefaults.install(core, this)
        // Where the remote lists live: every request of the core goes to this address through the Http port.
        configureRemote(RemoteConfig(baseUrl = server.start()))
    }
}
