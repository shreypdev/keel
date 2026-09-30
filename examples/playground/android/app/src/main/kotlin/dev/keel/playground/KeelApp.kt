package dev.keel.playground

import android.app.Application
import dev.keel.playground.core.KeelIds
import dev.keel.playground.core.RemoteConfig
import dev.keel.playground.core.configureRemote
import dev.keel.playground.remote.DemoServer
import dev.keel.playground.remote.InMemoryKv
import dev.keel.runtime.KeelCore
import dev.keel.runtime.LoadOptions
import dev.keel.runtime.adapters.ConnectivityEvents
import dev.keel.runtime.adapters.StandardPorts

/**
 * Attaches the app to its Rust core once per process, before any store is created.
 *
 * The core is `libkeel_core.so`, which `keel build --platform android` writes to `build/android/jniLibs`;
 * `KeelCore.load` checks that it was built from the same schema as the bindings (`KeelIds.SCHEMA_HASH`).
 * On Android the runtime installs the adapters that need nothing from the platform (Clock, Rng, Log,
 * Timer); the two this app needs beyond those are supplied here:
 *
 *  - `Http`: a demo server that lives in this process ([DemoServer]), so the Remote tab works without a
 *    network and can be taken offline on demand. A real app would pass an OkHttp-backed port here.
 *  - `Kv`: a map in memory ([InMemoryKv]). The demo server forgets everything when the app restarts,
 *    so a cache that outlived it would only be wrong.
 */
class KeelApp : Application() {
    /** The server behind the `Http` port; the Remote tab switches it offline. */
    val server = DemoServer()

    /** Sends `Connectivity.changed` to the core; the Remote tab calls it with the Offline switch. */
    val connectivity: ConnectivityEvents by lazy { ConnectivityEvents(KeelCore.shared) }

    override fun onCreate() {
        super.onCreate()
        KeelCore.load(
            LoadOptions(
                expectedSchemaHash = KeelIds.SCHEMA_HASH,
                adapters = mapOf(
                    StandardPorts.Http.PORT_ID to server.portImpl(),
                    StandardPorts.Kv.PORT_ID to InMemoryKv().portImpl(),
                ),
            ),
        )
        // Where the remote lists live: every request of the core goes to this address through the Http port.
        configureRemote(RemoteConfig(baseUrl = DemoServer.BASE_URL))
    }
}
