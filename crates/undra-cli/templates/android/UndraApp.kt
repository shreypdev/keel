package @@APP_ID@@

import android.app.Application
import android.os.Handler
import android.os.Looper
import android.util.Log
import @@KOTLIN_PACKAGE@@.UndraIds
import dev.undra.runtime.ClosedReason
import dev.undra.runtime.ConnectionState
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.Mode
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraSchemaMismatchException
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * Attaches the app to its Rust core once per process, before any store is created ([start], from the activity). The
 * core is `libundra_core.so`, which `undra build --platform android` writes to `build/android/jniLibs`.
 *
 * **Against `undra dev`** (debug builds, see [DevServer]) the core is the one `undra dev` serves, over a WebSocket:
 * edit the Rust, save, and the app is on the new core within a second, with no rebuild of the app. A dropped
 * connection is reconnected by the runtime; [connection] says what it is doing. When the dev server comes back with
 * a new core (a rebuild) the old core's objects are gone: the runtime reports `Closed(SESSION_LOST)`, this class loads
 * the new core and bumps [epoch], and the activity starts over on it.
 */
class UndraApp : Application() {
    /** The dev server this process uses, or `null` for the in-process core. */
    var devUrl: String? = null
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
     * Called again it does nothing; the answer of a running process cannot change.
     */
    @Synchronized
    fun start(requested: String?) {
        if (started) {
            if (requested != devUrl) Log.w(TAG, "already running with ${devUrl ?: "the in-process core"}; force-stop the app to switch")
            return
        }
        started = true
        devUrl = requested
        load()
    }

    /** Tries again after a failed start. */
    fun retry() {
        if (load()) _epoch.value += 1
    }

    /** Loads the core, in process or from [devUrl]; `false` (with [failure] set) when the dev server cannot be reached. */
    private fun load(): Boolean {
        val url = devUrl
        return try {
            UndraCore.load(
                LoadOptions(
                    mode = if (url == null) Mode.INPROC else Mode.REMOTE,
                    remoteUrl = url,
                    expectedSchemaHash = UndraIds.SCHEMA_HASH,
                    remoteTimeout = 5.seconds,
                    onConnectionChange = ::onConnection,
                ),
            )
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
