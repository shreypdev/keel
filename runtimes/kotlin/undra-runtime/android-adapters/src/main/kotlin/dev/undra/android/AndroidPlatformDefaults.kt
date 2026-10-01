package dev.undra.android

import android.content.Context
import android.util.Log
import dev.undra.runtime.UndraCore
import dev.undra.runtime.adapters.ClockAdapter
import dev.undra.runtime.adapters.RngAdapter
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.TimerAdapter
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
 * | `Kv` | [AndroidKvAdapter] | one file per key under `filesDir` |
 * | `SecureStore` | [AndroidSecureStoreAdapter] | AES-256-GCM under an Android Keystore key, files under `noBackupFilesDir` |
 * | `Fs` | [AndroidFsAdapter] | `filesDir`, confined to its root |
 * | `Connectivity` | [AndroidConnectivityAdapter] | `ConnectivityManager.registerDefaultNetworkCallback` |
 * | `Lifecycle` | [AndroidLifecycleAdapter] | `Application.ActivityLifecycleCallbacks` |
 * | `Log` | [AndroidLogAdapter] | `android.util.Log` |
 * | `Clock`, `Rng`, `Timer` | the runtime's own | `System`, `SecureRandom`, a scheduled executor |
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
     * Registers the adapters of all ten standard ports with [core] and starts reporting `Connectivity` and `Lifecycle`.
     * Installing again on the same core stops the earlier event sources first.
     *
     * @param core the core its load (`Undra<Namespace>.load`) returned.
     * @param context any context of the app; only its application context is kept.
     * @param http the `Http` adapter, to change its timeouts or size limit.
     * @param requireValidatedNetwork whether a network must pass Android's own reachability check to count as online;
     *   see [AndroidConnectivityAdapter].
     * @return the adapters, and a handle that stops the event sources.
     */
    public fun install(
        core: UndraCore,
        context: Context,
        http: AndroidHttpAdapter = AndroidHttpAdapter(),
        requireValidatedNetwork: Boolean = false,
    ): AndroidPlatform {
        val app = context.applicationContext
        val kv = AndroidKvAdapter(app)
        val secureStore = AndroidSecureStoreAdapter(app)
        val fs = AndroidFsAdapter(app)
        val log = AndroidLogAdapter()
        val timer = TimerAdapter(core::timerFired)
        val connectivity = AndroidConnectivityAdapter(app, requireValidated = requireValidatedNetwork)
        val lifecycle = AndroidLifecycleAdapter(app)
        val platform = AndroidPlatform(http, kv, secureStore, fs, log, connectivity, lifecycle, timer)

        // Installing again replaces the event sources; timers the core armed through the earlier adapter still fire.
        synchronized(installed) { installed.put(core, platform) }?.stopEventSources()

        // Kv first: the query client hydrates its cache and queue from it while the core starts.
        core.registerPort(StandardPorts.Kv.PORT_ID, kv.portImpl())
        core.registerPort(StandardPorts.SecureStore.PORT_ID, secureStore.portImpl())
        core.registerPort(StandardPorts.Fs.PORT_ID, fs.portImpl())
        core.registerPort(StandardPorts.Http.PORT_ID, http.portImpl())
        core.registerPort(StandardPorts.Clock.PORT_ID, ClockAdapter().portImpl())
        core.registerPort(StandardPorts.Rng.PORT_ID, RngAdapter().portImpl())
        core.registerPort(StandardPorts.Log.PORT_ID, log.portImpl())
        core.registerPort(StandardPorts.Timer.PORT_ID, timer.portImpl())
        connectivity.attach(core)
        lifecycle.attach(core)
        Log.i(TAG, "AndroidPlatformDefaults: registered Kv, SecureStore, Fs, Http, Clock, Rng, Log and Timer; reporting Connectivity and Lifecycle")
        return platform
    }
}
