package dev.undra.playground

import android.app.Application
import android.os.Handler
import android.os.Looper
import android.util.Log
import dev.undra.android.AndroidPlatform
import dev.undra.android.AndroidPlatformDefaults
import dev.undra.android.ChoreographerFramePacer
import dev.undra.playground.core.UndraPlaygroundCore
import dev.undra.playground.core.RemoteConfig
import dev.undra.playground.core.configureRemote
import dev.undra.playground.remote.DemoServer
import dev.undra.runtime.ClosedReason
import dev.undra.runtime.ConnectionState
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.UndraCore
import dev.undra.runtime.MirrorOptions
import dev.undra.runtime.Mode
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraSchemaMismatchException
import dev.undra.runtime.adapters.ConnectivityEvents
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * Attaches the app to its Rust core once per process, before any store is created ([start], from the activity).
 *
 * The core is `libplayground_core.so`, which `undra build --platform android` writes to `build/android/jniLibs`;
 * the bindings' entry, `UndraPlaygroundCore.load`, loads it and checks that it was built from their schema.
 * `AndroidPlatformDefaults.install` then gives it every platform port, none of them faked: `Http` over
 * `HttpURLConnection`, `Kv` and `Fs` in the app's files, `SecureStore` under an Android Keystore key, `Connectivity`
 * from `ConnectivityManager`, `Lifecycle` from the app's activities. The persisted query cache and the offline queue
 * therefore survive the process being killed.
 *
 * The Remote tab needs a server, and the playground has none to ship: [DemoServer] is a small HTTP server on the
 * device's loopback interface, so the core's requests still travel through the real `Http` adapter and a real socket.
 *
 * The mirror applies what the core produces on its own (the 10k list's streamed updates) once per display
 * frame, through the [ChoreographerFramePacer] of `android-adapters` (ADR-031).
 *
 * **Against `undra dev`** (debug builds, see [DevServer]) the core is the one `undra dev` serves, over a WebSocket:
 * edit the Rust, save, and the app is on the new core within a second, with no rebuild of the app, and with its state:
 * `undra dev` snapshots the old core and restores it into the new one (ADR-053), so the runtime just reconnects and the
 * screens converge on the values they had. A dropped connection (the laptop slept, `adb` restarted) is reconnected by the
 * runtime; [connection] says what it is doing and [devNotice] what the dev server said about the reload. When the state
 * could not be carried (a schema change, a state over the limit) the old core's objects are gone: the runtime reports
 * `Closed(SESSION_LOST)`, this class loads the new core and bumps [epoch], and the activity starts over on it. The
 * platform adapters stay on the device either way: each core gets its own `install`, and the previous one stops
 * reporting.
 *
 * A *command* (`store.toggle(...)`, `query.refetch()`) never throws into a click handler (ADR-032): the runtime logs a
 * failure at error level and hands it to `onError`, which is where an app would send it to its crash reporter. A
 * command tapped while the dev server is away is not such a failure: [connection] already says so, and the runtime only
 * logs it.
 */
class UndraApp : Application() {
    /** The server behind the Remote tab; the Offline switch makes it drop connections. */
    val server by lazy { DemoServer(this) }

    /** Sends `Connectivity.changed` to the core; the Remote tab's Offline switch calls it to simulate losing the network. */
    val connectivity: ConnectivityEvents get() = ConnectivityEvents(UndraCore.shared)

    /** The dev server this process uses, or `null` for the in-process core. */
    var devUrl: String? = null
        private set

    /**
     * How long `UndraPlaygroundCore.load` took in this process, in nanoseconds: the first load, which loads `libplayground_core.so`,
     * starts the core and checks the schema; `0` until a load has succeeded. Later loads (a dev server that restarted its
     * core) do not change it. The device benchmark (`bench/BenchRunner`) reports it as the cold start. The platform
     * adapters are installed after the clock stops, so the number stays the core's own start.
     */
    var coreLoadNanos: Long = 0L
        private set

    private val _connection = MutableStateFlow<ConnectionState>(ConnectionState.Connected)

    /** What the connection to `undra dev` is doing; always `Connected` for the in-process core. */
    val connection: StateFlow<ConnectionState> get() = _connection

    private val _epoch = MutableStateFlow(0)

    /** Counts the cores this process has loaded after the first: it changes when a new core replaced a lost one. */
    val epoch: StateFlow<Int> get() = _epoch

    private val _devNotice = MutableStateFlow<String?>(null)

    /** What `undra dev` said about its last reload ("Reloaded, state kept", ADR-053), for a few seconds; `null` after. */
    val devNotice: StateFlow<String?> get() = _devNotice

    private val _failure = MutableStateFlow<String?>(null)

    /** Why there is no core, when there is none: the dev server could not be reached, or it is another build. */
    val failure: StateFlow<String?> get() = _failure

    private val main = Handler(Looper.getMainLooper())
    private var started = false
    private var platform: AndroidPlatform? = null

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
            val core = UndraPlaygroundCore.load(
                LoadOptions(
                    mode = if (url == null) Mode.INPROC else Mode.REMOTE,
                    remoteUrl = url,
                    mirror = MirrorOptions(framePacer = ChoreographerFramePacer()),
                    remoteTimeout = 5.seconds,
                    onConnectionChange = ::onConnection,
                    onDevNotice = ::onDevNotice,
                    onError = { unhandled -> Log.w(TAG, "${unhandled.operation} failed: ${unhandled.error.message}") },
                ),
            )
            if (coreLoadNanos == 0L) coreLoadNanos = System.nanoTime() - loadStarted
            // Every platform port, none of them faked. A core the dev server replaced is gone: its event sources stop
            // before the new core gets its own.
            platform?.close()
            platform = AndroidPlatformDefaults.install(core, this)
            // Where the remote lists live: every request of the core goes to this address through the Http port.
            configureRemote(RemoteConfig(baseUrl = server.start()))
            _failure.value = null
            true
        } catch (e: UndraSchemaMismatchException) {
            val whose = if (url == null) "The core in this app is" else "The dev server's core is"
            _failure.value = "$whose built from another schema than this app's bindings. Run `undra bindgen`, then rebuild and reinstall the app."
            false
        } catch (e: UndraException) {
            _failure.value = if (url == null) "The core did not load: ${e.message}" else "Cannot reach the dev server at $url: ${e.message}"
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

    /** Heard on a thread of the runtime's: what the dev server says about a reload. Shown for [NOTICE_MILLIS]. */
    private fun onDevNotice(message: String) {
        Log.i(TAG, "dev server: $message")
        _devNotice.value = message
        main.postDelayed({ if (_devNotice.value == message) _devNotice.value = null }, NOTICE_MILLIS)
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
        const val NOTICE_MILLIS = 4_000L
    }
}
