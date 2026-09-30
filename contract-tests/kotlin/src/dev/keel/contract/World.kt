package dev.keel.contract

import dev.keel.playground.core.KeelIds
import dev.keel.playground.core.RemoteConfig
import dev.keel.playground.core.configureRemote
import dev.keel.runtime.KeelCore
import dev.keel.runtime.LoadOptions
import dev.keel.runtime.PortImpl
import dev.keel.runtime.adapters.ConnectivityEvents
import dev.keel.runtime.adapters.StandardPorts

/**
 * What a scenario runs against: the one core of this process and the adapters it was loaded with.
 * The adapters are the harness of scenarios.md: a manual clock, an in-memory server, an in-memory
 * `Kv` that records its writes and a `Log` that captures. `Rng` and `Timer` are the runtime's
 * JVM defaults.
 *
 * @property core the core, which is also `KeelCore.shared`.
 * @property clock the `Clock` port; the test moves it.
 * @property server the `Http` port: the routes and the requests it saw.
 * @property kv the `Kv` port: the writes the core made.
 * @property log the `Log` port: the records the core emitted.
 */
class World(
    val core: KeelCore,
    val clock: ManualClock,
    val server: FakeServer,
    val kv: MemoryKv,
    val log: CapturingLog,
) {
    /** The port the test emits `Connectivity.changed` through. */
    val connectivity = ConnectivityEvents(core)

    /** Reads the core's statistics now. */
    fun stats(): Stats = core.readStats()

    /**
     * Tells the core where the server is, once per process (`configure_remote`), however many scenarios
     * ask; the query scenarios S12 to S14 each call it first.
     */
    fun configureRemoteOnce() {
        remoteConfigured
    }

    private val remoteConfigured: Unit by lazy { configureRemote(RemoteConfig(BASE_URL), core) }

    /** The server address `configure_remote` is given (scenarios.md, "Server fixtures"). */
    companion object {
        /** `https://playground.test`. */
        const val BASE_URL: String = "https://playground.test"
    }
}

/**
 * Makes the adapters and loads the core once. The first load of a process claims the native library for
 * good (`KeelCore.load` cannot be undone), so S16 attempts its failing load before it calls [load].
 */
class Bootstrap {
    /** The manual `Clock` port the core is loaded with. */
    val clock = ManualClock()

    /** The in-memory `Http` server the core is loaded with. */
    val server = FakeServer()

    /** The in-memory `Kv` port the core is loaded with. */
    val kv = MemoryKv()

    /** The capturing `Log` port the core is loaded with. */
    val log = CapturingLog()

    /** The loaded core, or `null` while S16 has not loaded it (yet, or successfully). */
    var world: World? = null
        private set

    /** The adapters of the harness, by port id (`LoadOptions.adapters`). */
    private fun adapters(): Map<UInt, PortImpl> = mapOf(
        StandardPorts.Clock.PORT_ID to clock.portImpl(),
        StandardPorts.Http.PORT_ID to server.portImpl(),
        StandardPorts.Kv.PORT_ID to kv.portImpl(),
        StandardPorts.Log.PORT_ID to log.portImpl(),
    )

    /** Loads the core with the bindings' schema hash and the harness adapters. */
    fun load(): World {
        val core = KeelCore.load(LoadOptions(expectedSchemaHash = KeelIds.SCHEMA_HASH, adapters = adapters()))
        return World(core, clock, server, kv, log).also { world = it }
    }
}
