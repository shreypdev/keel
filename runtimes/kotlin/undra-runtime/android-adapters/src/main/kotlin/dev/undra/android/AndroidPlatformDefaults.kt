package dev.undra.android

import android.content.Context
import android.util.Log
import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraEmbeddingApi
import dev.undra.runtime.adapters.ClientWebSocketAdapter
import dev.undra.runtime.adapters.ClockAdapter
import dev.undra.runtime.adapters.DbPortAdapter
import dev.undra.runtime.adapters.RngAdapter
import dev.undra.runtime.adapters.SsePortAdapter
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.TimerAdapter
import dev.undra.runtime.adapters.UrlConnectionSseAdapter
import dev.undra.runtime.adapters.WebSocketPortAdapter
import java.util.WeakHashMap

/**
 * Every standard port of SPEC section 8 on an Android core: the adapters [AndroidPlatformDefaults.install] made, and
 * the handle that stops the event sources.
 *
 * @property http the `Http` adapter.
 * @property kv the `Kv` adapter.
 * @property secureStore the `SecureStore` adapter.
 * @property fs the `Fs` adapter.
 * @property log the `Log` adapter.
 * @property connectivity the `Connectivity` event source.
 * @property lifecycle the `Lifecycle` event source.
 * @property webSocket the binding of the opt-in `WebSocket` port (ADR-047), over the runtime's [ClientWebSocketAdapter].
 * @property sse the binding of the opt-in `Sse` port (ADR-047), over [UrlConnectionSseAdapter] (`HttpURLConnection`).
 * @property db the binding of the opt-in `Db` port (ADR-048), over [AndroidDbAdapter].
 */
public class AndroidPlatform internal constructor(
    public val http: AndroidHttpAdapter,
    public val kv: AndroidKvAdapter,
    public val secureStore: AndroidSecureStoreAdapter,
    public val fs: AndroidFsAdapter,
    public val log: AndroidLogAdapter,
    public val connectivity: AndroidConnectivityAdapter,
    public val lifecycle: AndroidLifecycleAdapter,
    private val timer: TimerAdapter,
    public val webSocket: WebSocketPortAdapter,
    public val sse: SsePortAdapter,
    public val db: DbPortAdapter,
) : AutoCloseable {
    /**
     * Stops reporting `Connectivity` and `Lifecycle` events and cancels pending timers. The ports stay registered (a
     * timer the core sets afterwards is answered `unavailable`, since its executor is gone); an app that never closes
     * the core never needs this. Tests that reuse a core call [AndroidPlatformDefaults.install] again instead.
     */
    override fun close() {
        stopEventSources()
        timer.close()
    }

    /** Stops the `Connectivity` and `Lifecycle` reports only; timers the core already armed keep running. */
    internal fun stopEventSources() {
        connectivity.close()
        lifecycle.close()
    }
}

/**
 * The one call an Android app makes to give its core every platform capability (SPEC sections 8 and 11), mirroring the
 * Swift `Adapters.platformDefault`:
 *
 * ```kotlin
 * class MyApp : Application() {
 *     override fun onCreate() {
 *         super.onCreate()
 *         // The generated entry of the core's bindings, Undra<Namespace> (ADR-044).
 *         val core = UndraPlaygroundCore.load(LoadOptions(mirror = MirrorOptions(framePacer = ChoreographerFramePacer())))
 *         AndroidPlatformDefaults.install(core, this)
 *     }
 * }
 * ```
 *
 * | Port | Adapter | Android API |
 * |---|---|---|
 * | `Http` | [AndroidHttpAdapter] | `HttpURLConnection` on `Dispatchers.IO`, aborted on cancellation |
 * | `Kv` | [AndroidKvAdapter] | one file per key under `filesDir/undra/<namespace>/kv` |
 * | `SecureStore` | [AndroidSecureStoreAdapter] | AES-256-GCM under the Android Keystore key `<namespace>.dev.undra.securestore`, files under `noBackupFilesDir/undra/<namespace>/secure` |
 * | `Fs` | [AndroidFsAdapter] | `filesDir/undra/<namespace>/fs`, confined to its root |
 * | `Connectivity` | [AndroidConnectivityAdapter] | `ConnectivityManager.registerDefaultNetworkCallback` |
 * | `Lifecycle` | [AndroidLifecycleAdapter] | `Application.ActivityLifecycleCallbacks` (`reportLifecycle = false` opts out) |
 * | `Log` | [AndroidLogAdapter] | `android.util.Log` |
 * | `Clock`, `Rng`, `Timer` | the runtime's own | `System`, `SecureRandom`, a scheduled executor |
 * | `WebSocket` (opt-in, ADR-047) | [WebSocketPortAdapter] over the runtime's [ClientWebSocketAdapter] | `java.net.Socket` (RFC 6455 client of the runtime) |
 * | `Sse` (opt-in, ADR-047) | [SsePortAdapter] over [UrlConnectionSseAdapter] | `HttpURLConnection` |
 * | `Db` (opt-in, ADR-048) | [DbPortAdapter] over [AndroidDbAdapter] | `android.database.sqlite`, `getDatabasePath("undra-<namespace>-<name>.sqlite")` |
 *
 * **Every default store is per core namespace** (ADR-044, amendment A): the namespace is the one in [UndraCore.namespace]
 * (the generated entry's, `UndraIds.NAMESPACE`), so two cores of one app that both use these defaults never read or
 * overwrite each other's keys, secrets, files or databases. An adapter you construct yourself and register afterwards
 * (`AndroidKvAdapter(directory)`, `AndroidSecureStoreAdapter(directory, keyAlias)`, ...) keeps the location you gave it.
 *
 * The three opt-in ports are registered whatever the core enables (cargo features `websocket`, `sse`, `db`): a core that
 * does not declare one never calls it. When the core closes, their connections and databases are closed.
 *
 * Call it once, right after the core is loaded (`Undra<Namespace>.load`) and before any store is created. The core reads its persisted query cache
 * and offline queue through `Kv` while it starts and waits up to five seconds for the adapter to appear, which is why
 * installing after `load` is enough.
 *
 * Every failure of the storage adapters reaches the core as a typed error, never as a crash or a silent miss
 * (ADR-049): `StorageError` (`Full`, `Corrupt`, `Locked`, `Unavailable`, `Io`) for `Kv` and `SecureStore`, `FsError`
 * (with `Full` for a full disk) for `Fs`, and `HttpError` for `Http`. To change one port, register another implementation afterwards:
 * `core.registerPort(StandardPorts.Http.PORT_ID, impl)` replaces what this installed.
 *
 * Needs the permissions `INTERNET` and `ACCESS_NETWORK_STATE`; this library's manifest declares both, so they merge into the
 * app's. Cleartext (`http://`) requests are blocked by Android unless the app's network security config allows the host.
 */
public object AndroidPlatformDefaults {
    private const val TAG = "Undra"
    private val installed = WeakHashMap<UndraCore, AndroidPlatform>()

    /**
     * Registers the adapters of all ten standard ports and of the three opt-in ones (WebSocket, Sse, Db) with [core] and
     * starts reporting `Connectivity` and `Lifecycle`.
     * Installing again on the same core stops the earlier event sources first.
     *
     * @param core the core its load (`Undra<Namespace>.load`) returned.
     * @param context any context of the app; only its application context is kept.
     * @param http the `Http` adapter, to change its timeouts or size limit.
     * @param requireValidatedNetwork whether a network must pass Android's own reachability check to count as online;
     *   see [AndroidConnectivityAdapter].
     * @param reportLifecycle whether the app's activities are reported to the core as `Lifecycle.changed` (ADR-046: on by default, so
     *   the core refetches on `Active` and flushes persistence on `Background` without the app forwarding anything). `false` leaves
     *   the `Lifecycle` port to the app (`LifecycleEvents`); [AndroidPlatform.lifecycle] is then an adapter that is not started.
     * @param onBackgroundWorkPending called on the main thread when the app moved to the background and the core has background work
     *   to drain (`stats().background.pending > 0`); ask the OS for a window here, with the `android-work` module:
     *   `onBackgroundWorkPending = { UndraWork.schedule(app) }`. Ignored when [reportLifecycle] is `false`; see
     *   [AndroidLifecycleAdapter.attach].
     * @return the adapters, and a handle that stops the event sources.
     */
    public fun install(
        core: UndraCore,
        context: Context,
        http: AndroidHttpAdapter = AndroidHttpAdapter(),
        requireValidatedNetwork: Boolean = false,
        reportLifecycle: Boolean = true,
        onBackgroundWorkPending: (() -> Unit)? = null,
    ): AndroidPlatform = install(
        core,
        context,
        http,
        http.portImpl(),
        WebSocketPortAdapter(ClientWebSocketAdapter()),
        SsePortAdapter(UrlConnectionSseAdapter()),
        requireValidatedNetwork,
        reportLifecycle,
        onBackgroundWorkPending,
    )

    /**
     * [install] with other implementations of the three network ports, registered in place of the platform's: [http] for `Http`,
     * [webSocket] for `WebSocket`, [sse] for `Sse`. The platform's network adapters are never registered, so no request can go
     * through them, not even one the core makes while this runs (it replays its offline queue as soon as `Kv` answers, before the
     * call returns). It is the call of a module that puts another HTTP stack behind the ports (`okhttp-adapters`'
     * `installWithOkHttp`, ADR-060); an app calls that, or [install].
     *
     * The returned platform's `webSocket` and `sse` are [webSocket] and [sse]; its `http` is an [AndroidHttpAdapter] that is not
     * registered.
     *
     * @param core the core its load (`Undra<Namespace>.load`) returned.
     * @param context any context of the app; only its application context is kept.
     * @param http the implementation of the `Http` port.
     * @param webSocket the binding of the opt-in `WebSocket` port.
     * @param sse the binding of the opt-in `Sse` port.
     * @param requireValidatedNetwork as in [install].
     * @param reportLifecycle as in [install].
     * @param onBackgroundWorkPending as in [install].
     * @return the adapters, and a handle that stops the event sources.
     */
    @UndraEmbeddingApi
    public fun installWithNetworkPorts(
        core: UndraCore,
        context: Context,
        http: PortImpl,
        webSocket: WebSocketPortAdapter,
        sse: SsePortAdapter,
        requireValidatedNetwork: Boolean = false,
        reportLifecycle: Boolean = true,
        onBackgroundWorkPending: (() -> Unit)? = null,
    ): AndroidPlatform =
        install(core, context, AndroidHttpAdapter(), http, webSocket, sse, requireValidatedNetwork, reportLifecycle, onBackgroundWorkPending)

    private fun install(
        core: UndraCore,
        context: Context,
        http: AndroidHttpAdapter,
        httpPort: PortImpl,
        webSocket: WebSocketPortAdapter,
        sse: SsePortAdapter,
        requireValidatedNetwork: Boolean,
        reportLifecycle: Boolean,
        onBackgroundWorkPending: (() -> Unit)?,
    ): AndroidPlatform {
        val app = context.applicationContext
        val namespace = core.namespace
        val kv = AndroidKvAdapter(app, namespace)
        val secureStore = AndroidSecureStoreAdapter(app, namespace)
        val fs = AndroidFsAdapter(app, namespace)
        val log = AndroidLogAdapter()
        val timer = TimerAdapter(core::timerFired)
        val connectivity = AndroidConnectivityAdapter(app, requireValidated = requireValidatedNetwork)
        val lifecycle = AndroidLifecycleAdapter(app)
        val db = DbPortAdapter(AndroidDbAdapter(app, namespace))
        val platform = AndroidPlatform(http, kv, secureStore, fs, log, connectivity, lifecycle, timer, webSocket, sse, db)

        // Installing again replaces the event sources; timers the core armed through the earlier adapter still fire.
        synchronized(installed) { installed.put(core, platform) }?.stopEventSources()

        // The network first: the core may make a request as soon as Kv answers (it replays its offline queue then), so the ports a
        // request goes through are in place before it. Then Kv: the query client hydrates its cache and queue from it while the core starts.
        core.registerPort(StandardPorts.Http.PORT_ID, httpPort)
        core.registerPort(StandardPorts.WebSocket.PORT_ID, webSocket.portImpl())
        core.registerPort(StandardPorts.Sse.PORT_ID, sse.portImpl())
        core.registerPort(StandardPorts.Kv.PORT_ID, kv.portImpl())
        core.registerPort(StandardPorts.SecureStore.PORT_ID, secureStore.portImpl())
        core.registerPort(StandardPorts.Fs.PORT_ID, fs.portImpl())
        core.registerPort(StandardPorts.Clock.PORT_ID, ClockAdapter().portImpl())
        core.registerPort(StandardPorts.Rng.PORT_ID, RngAdapter().portImpl())
        core.registerPort(StandardPorts.Log.PORT_ID, log.portImpl())
        core.registerPort(StandardPorts.Timer.PORT_ID, timer.portImpl())
        core.registerPort(StandardPorts.Db.PORT_ID, db.portImpl())
        connectivity.attach(core)
        if (reportLifecycle) lifecycle.attach(core, onBackgroundWorkPending)
        Log.i(
            TAG,
            "AndroidPlatformDefaults: registered Kv, SecureStore, Fs, Http, Clock, Rng, Log, Timer, WebSocket, Sse and Db; " +
                "reporting Connectivity" + if (reportLifecycle) " and Lifecycle" else " (Lifecycle is left to the app)",
        )
        return platform
    }
}
