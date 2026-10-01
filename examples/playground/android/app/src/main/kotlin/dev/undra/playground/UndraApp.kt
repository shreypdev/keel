package dev.undra.playground

import android.app.Application
import dev.undra.android.ChoreographerFramePacer
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.RemoteConfig
import dev.undra.playground.core.configureRemote
import dev.undra.playground.remote.DemoServer
import dev.undra.playground.remote.InMemoryKv
import dev.undra.runtime.UndraCore
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.MirrorOptions
import dev.undra.runtime.adapters.ConnectivityEvents
import dev.undra.runtime.adapters.StandardPorts

/**
 * Attaches the app to its Rust core once per process, before any store is created.
 *
 * The core is `libundra_core.so`, which `undra build --platform android` writes to `build/android/jniLibs`;
 * `UndraCore.load` checks that it was built from the same schema as the bindings (`UndraIds.SCHEMA_HASH`).
 * On Android the runtime installs the adapters that need nothing from the platform (Clock, Rng, Log,
 * Timer); the two this app needs beyond those are supplied here:
 *
 *  - `Http`: a demo server that lives in this process ([DemoServer]), so the Remote tab works without a
 *    network and can be taken offline on demand. A real app would pass an OkHttp-backed port here.
 *  - `Kv`: a map in memory ([InMemoryKv]). The demo server forgets everything when the app restarts,
 *    so a cache that outlived it would only be wrong.
 *
 * The mirror applies what the core produces on its own (the 10k list's streamed updates) once per display
 * frame, through the [ChoreographerFramePacer] of `android-adapters` (ADR-031).
 */
class UndraApp : Application() {
    /** The server behind the `Http` port; the Remote tab switches it offline. */
    val server = DemoServer()

    /** Sends `Connectivity.changed` to the core; the Remote tab calls it with the Offline switch. */
    val connectivity: ConnectivityEvents by lazy { ConnectivityEvents(UndraCore.shared) }

    /**
     * How long `UndraCore.load` took in this process, in nanoseconds: the first load, which loads `libundra_core.so`,
     * starts the core and checks the schema. The device benchmark (`bench/BenchRunner`) reports it as the cold start.
     */
    var coreLoadNanos: Long = 0L
        private set

    override fun onCreate() {
        super.onCreate()
        val loadStarted = System.nanoTime()
        UndraCore.load(
            LoadOptions(
                expectedSchemaHash = UndraIds.SCHEMA_HASH,
                adapters = mapOf(
                    StandardPorts.Http.PORT_ID to server.portImpl(),
                    StandardPorts.Kv.PORT_ID to InMemoryKv().portImpl(),
                ),
                mirror = MirrorOptions(framePacer = ChoreographerFramePacer()),
            ),
        )
        coreLoadNanos = System.nanoTime() - loadStarted
        // Where the remote lists live: every request of the core goes to this address through the Http port.
        configureRemote(RemoteConfig(baseUrl = DemoServer.BASE_URL))
    }
}
