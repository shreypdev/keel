package dev.undra.runtime.adapters

import dev.undra.runtime.PortImpl
import dev.undra.runtime.Platform
import java.nio.file.Path
import java.nio.file.Paths

/**
 * The default port implementations for a JVM host (SPEC section 11), keyed by port id so they can be
 * passed to `LoadOptions.adapters`:
 *
 * | Port | Implementation |
 * |---|---|
 * | `Http` | [HttpAdapter] over `java.net.http.HttpClient` |
 * | `Kv`, `SecureStore` | [FileKv] over files in `<dataDir>/kv` and `<dataDir>/secure`; failures are [StorageError]s |
 * | `Fs` | [FsAdapter] over `<dataDir>/fs`; failures are [FsError]s |
 * | `Clock`, `Rng`, `Log` | [ClockAdapter], [RngAdapter] (`SecureRandom`), [LogAdapter] (`java.util.logging`) |
 * | `Timer` | [TimerAdapter] over a scheduled executor |
 * | `Connectivity`, `Lifecycle` | none: they are event ports; see [ConnectivityEvents] and [LifecycleEvents] |
 * | `WebSocket` (opt-in, ADR-047) | [WebSocketPortAdapter] over [ClientWebSocketAdapter], the runtime's own RFC 6455 client |
 * | `Sse` (opt-in, ADR-047) | [SsePortAdapter] over [JdkHttpSseAdapter] (`java.net.http`) |
 * | `Db` (opt-in, ADR-048) | [DbPortAdapter] over [JdbcDbAdapter] in `<dataDir>/db` (needs `org.xerial:sqlite-jdbc` on the class path) |
 *
 * The three opt-in ports are registered whether or not the core enables them (cargo features `websocket`, `sse`, `db`); a
 * core that does not declare a port never calls it.
 *
 * `UndraCore.load` installs these itself unless `LoadOptions.defaultAdapters` is `false`; use this object to
 * pick another data directory or to mix them with your own.
 *
 * On Android `UndraCore.load` installs only [portable] (SPEC section 11): the other six come from the
 * `android-adapters` module (`AndroidPlatformDefaults.install(core, context)`, right after the load). Until then a
 * storage call of the core is unanswered by any adapter, which the core reads as `StorageError::Unavailable`, never a
 * crash (ADR-049); the query layer's hydration keeps asking for a few seconds, which is why installing after `load`
 * is enough. Nothing is registered in their place on purpose: a stand-in answering `Unavailable` as a typed error
 * would end that wait at once.
 */
public object JvmAdapters {
    /** System property naming the directory of the file-backed adapters. */
    public const val DATA_DIR_PROPERTY: String = "undra.data.dir"

    /** The data directory: the system property `undra.data.dir`, or `.undra/data` in the user's home directory. */
    public fun defaultDataDir(): Path {
        val configured = System.getProperty(DATA_DIR_PROPERTY)
        if (!configured.isNullOrEmpty()) return Paths.get(configured)
        return Paths.get(System.getProperty("user.home") ?: ".", ".undra", "data")
    }

    /**
     * Every adapter of the table above, with file-backed ones under [dataDir].
     *
     * @param timerFired what the timer adapter calls when a timer is due; pass `core::timerFired`.
     */
    public fun standard(dataDir: Path = defaultDataDir(), timerFired: (UInt) -> Unit): Map<UInt, PortImpl> {
        val all = LinkedHashMap<UInt, PortImpl>(portable(timerFired))
        all[StandardPorts.Http.PORT_ID] = HttpAdapter().portImpl()
        all[StandardPorts.Kv.PORT_ID] = FileKv(dataDir.resolve("kv")).portImpl("Kv")
        all[StandardPorts.SecureStore.PORT_ID] = FileKv(dataDir.resolve("secure")).portImpl("SecureStore")
        all[StandardPorts.Fs.PORT_ID] = FsAdapter(dataDir.resolve("fs")).portImpl()
        all[StandardPorts.WebSocket.PORT_ID] = webSocketPort(ClientWebSocketAdapter())
        all[StandardPorts.Sse.PORT_ID] = ssePort(JdkHttpSseAdapter())
        all[StandardPorts.Db.PORT_ID] = dbPort(JdbcDbAdapter(dataDir.resolve("db")))
        return all
    }

    /**
     * Only the adapters that need nothing from the platform beyond the JDK's core classes: Clock, Rng, Log
     * and Timer. Used where the rest must come from elsewhere (Android, where `android-adapters` installs them).
     */
    public fun portable(timerFired: (UInt) -> Unit): Map<UInt, PortImpl> =
        linkedMapOf(
            StandardPorts.Clock.PORT_ID to ClockAdapter().portImpl(),
            StandardPorts.Rng.PORT_ID to RngAdapter().portImpl(),
            StandardPorts.Log.PORT_ID to LogAdapter().portImpl(),
            StandardPorts.Timer.PORT_ID to TimerAdapter(timerFired).portImpl(),
        )

    /** What `UndraCore.load` installs by default: [standard] on a JVM, [portable] on Android. */
    internal fun defaults(timerFired: (UInt) -> Unit): Map<UInt, PortImpl> =
        if (Platform.isAndroid) portable(timerFired) else standard(timerFired = timerFired)
}
