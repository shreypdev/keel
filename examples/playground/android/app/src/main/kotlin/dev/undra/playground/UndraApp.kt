package dev.undra.playground

import android.app.Application
import android.os.Handler
import android.os.Looper
import android.util.Log
import dev.undra.android.ChoreographerFramePacer
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.RemoteConfig
import dev.undra.playground.core.configureRemote
import dev.undra.playground.remote.DemoServer
import dev.undra.playground.remote.InMemoryKv
import dev.undra.runtime.ClosedReason
import dev.undra.runtime.ConnectionState
import dev.undra.runtime.UndraCore
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.MirrorOptions
import dev.undra.runtime.Mode
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraSchemaMismatchException
import dev.undra.runtime.adapters.ConnectivityEvents
import dev.undra.runtime.adapters.StandardPorts
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * Attaches the app to its Rust core once per process, before any store is created ([start], from the activity).
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
 *
 * **Against `undra dev`** (debug builds, see [DevServer]) the core is the one `undra dev` serves, over a WebSocket:
 * edit the Rust, save, and the app is on the new core within a second, with no rebuild of the app. A dropped
 * connection (the laptop slept, `adb` restarted) is reconnected by the runtime; [connection] says what it is doing.
 * When the dev server comes back with a new core (a rebuild) the old core's objects are gone: the runtime reports
 * `Closed(SESSION_LOST)`, this class loads the new core and bumps [epoch], and the activity starts over on it.
 */
class UndraApp : Application() {
    /** The server behind the `Http` port; the Remote tab switches it offline. */
    val server = DemoServer()

    /** Sends `Connectivity.changed` to the core; the Remote tab calls it with the Offline switch. */
    val connectivity: ConnectivityEvents get() = ConnectivityEvents(UndraCore.shared)

    /** The dev server this process uses, or `null` for the in-process core. */
    var devUrl: String? = null
        private set

    /**
     * How long `UndraCore.load` took in this process, in nanoseconds: the first load, which loads `libundra_core.so`,
     * starts the core and checks the schema; `0` until a load has succeeded. Later loads (a dev server that restarted its
     * core) do not change it. The device benchmark (`bench/BenchRunner`) reports it as the cold start.
     */
    var coreLoadNanos: Long = 0L
        private set

    private val _connection = MutableStateFlow<ConnectionState>(ConnectionState.Connected)

    /** What the connection to `undra dev` is doing; always `Connected` for the in-process core. */
    val connection: StateFlow<ConnectionState> get() = _connection

    private val _epoch = MutableStateFlow(0)

    /** Counts the cores this process has loaded after the first: it changes when a new core replaced a lost one. */
    val epoch: StateFlow<Int> get() = _epoch

    private val _failure = MutableStateFlow<String?>(null)

    /** Why there is no core, when there is none: the dev server could not be reached, or it is another build. */
    val failure: StateFlow<String?> get() = _failure

    private val main = Handler(Looper.getMainLooper())
    private var started = false

    /**
     * Loads the core, once per process: in process, or from the dev server [requested] names (see [DevServer]).
     * Called with the same answer again it does nothing; the answer of a running process cannot change.
     */
    @Synchronized
    fun start(requested: String?) {
        if (started) {
            if (requested != devUrl) Log.w(TAG, "already running with ${devUrl ?: "the in-process core"}; force-stop the app to switch to ${requested ?: "the in-process core"}")
            return
        }
        started = true
        devUrl = requested
        if (load()) return
        // Not reachable (yet): the activity shows why, with a Retry that comes back here.
    }

    /** Tries again after a failed start. */
    fun retry() {
        if (load()) _epoch.value += 1
    }

    /** Loads the core, in process or from [devUrl]; `false` (with [failure] set) when the dev server cannot be reached. */
    private fun load(): Boolean {
        val url = devUrl
        return try {
            val loadStarted = System.nanoTime()
            UndraCore.load(
                LoadOptions(
                    mode = if (url == null) Mode.INPROC else Mode.REMOTE,
                    remoteUrl = url,
                    expectedSchemaHash = UndraIds.SCHEMA_HASH,
                    adapters = mapOf(
                        StandardPorts.Http.PORT_ID to server.portImpl(),
                        StandardPorts.Kv.PORT_ID to InMemoryKv().portImpl(),
                    ),
                    mirror = MirrorOptions(framePacer = ChoreographerFramePacer()),
                    remoteTimeout = 5.seconds,
                    onConnectionChange = ::onConnection,
                ),
            )
            if (coreLoadNanos == 0L) coreLoadNanos = System.nanoTime() - loadStarted
            // Where the remote lists live: every request of the core goes to this address through the Http port.
            configureRemote(RemoteConfig(baseUrl = DemoServer.BASE_URL))
            _failure.value = null
            true
        } catch (e: UndraSchemaMismatchException) {
            _failure.value = "The dev server runs a core built from another schema than this app's bindings. Run `undra bindgen`, then rebuild and reinstall the app."
            false
        } catch (e: UndraException) {
            _failure.value = "Cannot reach the dev server at $url: ${e.message}"
            false
        }
    }

    /** Heard on a thread of the runtime's: every change of the core's connection. */
    private fun onConnection(state: ConnectionState) {
        Log.i(TAG, "connection: $state")
        _connection.value = state
        if (state is ConnectionState.Closed && state.reason == ClosedReason.SESSION_LOST) {
            // `undra dev` restarted the core: its objects are gone. Load the new one, then start over on it.
            main.post { reloadWhenReachable() }
        }
    }

    private fun reloadWhenReachable() {
        if (load()) {
            _epoch.value += 1
        } else if (_failure.value?.startsWith("Cannot reach") == true) {
            main.postDelayed(::reloadWhenReachable, RELOAD_RETRY_MILLIS)
        }
    }

    private companion object {
        const val TAG = "UndraApp"
        const val RELOAD_RETRY_MILLIS = 500L
    }
}
