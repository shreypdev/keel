package dev.undra.contract

import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.UndraPlaygroundCore
import dev.undra.playground.core.RemoteConfig
import dev.undra.playground.core.configureRemote
import dev.undra.runtime.UndraCore
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraUnhandledError
import dev.undra.runtime.adapters.ConnectivityEvents
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.StorageError
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicLong

/**
 * What a scenario runs against: the one core of this process and the adapters it was loaded with.
 * The adapters are the harness of scenarios.md: a manual clock, an in-memory server, an in-memory
 * `Kv` that records its operations and fails on demand, and a `Log` that captures. `Rng` and `Timer` are the runtime's
 * JVM defaults.
 *
 * @property core the core, which is also `UndraCore.shared`.
 * @property clock the `Clock` port; the test moves it.
 * @property server the `Http` port: the routes and the requests it saw.
 * @property kv the `Kv` port: what the core stored and every operation it asked for.
 * @property log the `Log` port: the records the core emitted.
 * @property portCalls how many calls the four adapters above received (S17.7).
 * @property options the options [core] was loaded with; S17.7 loads a fresh core with them after the shutdown.
 * @property panics what `LoadOptions.onPanic` received (S29).
 * @property unhandled what `LoadOptions.onError` received: the failures of commands and of changes that could not be
 *   applied (ADR-032, amendment A). A scenario that causes one asserts it and then calls [takeUnhandled].
 */
class World(
    val core: UndraCore,
    val clock: ManualClock,
    val server: FakeServer,
    val kv: MemoryKv,
    val log: CapturingLog,
    val portCalls: PortCallCounter,
    val options: LoadOptions,
    val unhandled: CopyOnWriteArrayList<UndraUnhandledError> = CopyOnWriteArrayList(),
    val panics: PanicLog = PanicLog(),
) {
    /** What `onError` received since the last call, oldest first; the list is empty afterwards. */
    fun takeUnhandled(): List<UndraUnhandledError> = unhandled.toList().also { unhandled.clear() }

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
 * Makes the adapters and loads the core the scenarios share. A load claims the native library until that core
 * is closed (ADR-034), so S16 attempts its failing load before it calls [load]; S17.7 loads a second core with
 * [World.options] after closing the first.
 */
class Bootstrap {
    /** The manual `Clock` port the core is loaded with. */
    val clock = ManualClock()

    /** The in-memory `Http` server the core is loaded with. */
    val server = FakeServer()

    /**
     * The in-memory `Kv` port the core is loaded with. Its first `get` of the offline queue fails `Locked`, as the
     * platform's store answers an app launched before the device's first unlock (scenarios.md, "Adapters"): S14 waits
     * until the core read the queue again, S20 step 4 checks what it did meanwhile.
     */
    val kv = MemoryKv().also { it.fail(MemoryKv.Kind.GET, key = Persisted.QUEUE_KEY, error = StorageError.Locked, times = 1) }

    /** The capturing `Log` port the core is loaded with. */
    val log = CapturingLog()

    /** Counts the calls the four adapters above receive. */
    val portCalls = PortCallCounter()

    /** What `LoadOptions.onError` received. */
    val unhandled = CopyOnWriteArrayList<UndraUnhandledError>()

    /** What `LoadOptions.onPanic` received. */
    val panics = PanicLog()

    /** The loaded core, or `null` while S16 has not loaded it (yet, or successfully). */
    var world: World? = null
        private set

    /** The adapters of the harness, by port id (`LoadOptions.adapters`), each counting its calls in [portCalls]. */
    private fun adapters(): Map<UInt, PortImpl> = mapOf(
        StandardPorts.Clock.PORT_ID to portCalls.counting("Clock", clock.portImpl()),
        StandardPorts.Http.PORT_ID to portCalls.counting("Http", server.portImpl()),
        StandardPorts.Kv.PORT_ID to portCalls.counting("Kv", kv.portImpl()),
        StandardPorts.Log.PORT_ID to portCalls.counting("Log", log.portImpl()),
    )

    /** Loads the core through the bindings' entry (which brings the schema hash) with the harness adapters. */
    fun load(): World {
        val options = LoadOptions(adapters = adapters(), onError = { unhandled.add(it) }, onPanic = panics::record)
        val core = UndraPlaygroundCore.load(options)
        return World(core, clock, server, kv, log, portCalls, options, unhandled, panics).also { world = it }
    }
}

/**
 * Counts the calls the core makes into the harness adapters, by port name. S17.7 uses it to see that a core that
 * was shut down calls none of them any more.
 */
class PortCallCounter {
    private val counts = ConcurrentHashMap<String, AtomicLong>()

    /** [impl] with every method counting a call to [port] before it runs. */
    fun counting(port: String, impl: PortImpl): PortImpl {
        val count = counts.computeIfAbsent(port) { AtomicLong() }
        val methods = LinkedHashMap<UInt, suspend (ByteArray) -> ByteArray>()
        for ((id, method) in impl.methods) {
            methods[id] = { args ->
                count.incrementAndGet()
                method(args)
            }
        }
        return PortImpl(impl.sync, methods)
    }

    /** The calls [port] has received so far. */
    fun count(port: String): Long = counts[port]?.get() ?: 0L

    /** The calls every counted port has received so far, by port name. */
    fun all(): Map<String, Long> = counts.entries.associate { it.key to it.value.get() }.toSortedMap()

    /** The calls all counted ports have received so far. */
    val total: Long get() = counts.values.sumOf { it.get() }
}
