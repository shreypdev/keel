package @@APP_ID@@

import android.app.Application
import android.os.Handler
import android.os.Looper
import android.util.Log
import @@KOTLIN_PACKAGE@@.@@CORE_ENTRY@@
import dev.undra.android.AndroidPlatform
import dev.undra.android.AndroidPlatformDefaults
import dev.undra.android.ChoreographerFramePacer
import dev.undra.runtime.ClosedReason
import dev.undra.runtime.ConnectionState
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.MirrorOptions
import dev.undra.runtime.Mode
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraSchemaMismatchException
import dev.undra.runtime.adapters.UndraPanicReport
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * Attaches the app to its Rust core once per process, before any store is created ([start], from the activity). The
 * core is `lib@@NAMESPACE@@.so`, which `undra build --platform android` writes to `build/android/jniLibs`; the bindings'
 * entry, [@@CORE_ENTRY@@], loads it and checks it was built from their schema.
 *
 * `AndroidPlatformDefaults.install` gives the core every platform capability in one call: `Http` over
 * `HttpURLConnection`, `Kv` and `Fs` in the app's files, `SecureStore` under an Android Keystore key, and the
 * `Connectivity` and `Lifecycle` events. They need the `INTERNET` and `ACCESS_NETWORK_STATE` permissions of the manifest.
 *
 * What the core produces on its own (timers, streams, port completions) is applied to the stores once per
 * display frame, at the display's own frames: [ChoreographerFramePacer] (the `android-adapters` module) hands the
 * mirror each vsync. Without it the runtime drains on a 60 Hz grid of its own, which is not aligned with the display
 * (and wrong for a 90 or 120 Hz one). Replies and `callSync` on the main thread never wait for a frame either way.
 *
 * **Against `undra dev`** (debug builds, see [DevServer]) the core is the one `undra dev` serves, over a WebSocket:
 * edit the Rust, save, and the app is on the new core within a second, with no rebuild of the app, and with its state:
 * `undra dev` snapshots the old core and restores it into the new one (ADR-053), so the runtime just reconnects and the
 * screens converge on the values they had. A dropped connection is reconnected by the runtime; [connection] says what it
 * is doing and [devNotice] what the dev server said about the reload. When the state could not be carried (a schema
 * change, a state over the limit) the old core's objects are gone: the runtime reports `Closed(SESSION_LOST)`, this
 * class loads the new core and bumps [epoch], and the activity starts over on it.
 *
 * **When the core panics** the call that panicked fails (`UndraCallError.Panicked`) and the core keeps working; [onPanic]
 * also hears one structured report per panic (message, `file:line:column`, the operation, stack frames) on the main thread:
 * log it, and forward it to your crash reporter there (ADR-046). Release builds keep line tables, so the frames symbolicate with
 * the files `undra build --release` writes next to the app (`build/symbols`).
 *
 * **Background drains** (opt-in, ADR-046): the offline queue can be replayed while the app is in the background. Add
 * `implementation("dev.undra:android-work:@@KOTLIN_RUNTIME_VERSION@@")` to `app/build.gradle.kts`, then uncomment the two lines
 * marked `android-work` below: [UndraWork.configure] says how a process WorkManager starts on its own loads the core, and
 * `onBackgroundWorkPending` asks WorkManager for a window, with a network constraint, when the app goes to the background
 * with work pending.
 */
class UndraApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // android-work (opt-in): how a process that WorkManager starts without an activity loads the core. `start` is idempotent:
        // it loads the in-process core the first time and does nothing after, so the loader also serves a warm process.
        // UndraWork.configure(loader = { context -> (context.applicationContext as UndraApp).start(null); @@CORE_ENTRY@@.core })
    }

    /** The dev server this process uses, or `null` for the in-process core. */
    var devUrl: String? = null
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
            val core = @@CORE_ENTRY@@.load(
                LoadOptions(
                    mode = if (url == null) Mode.INPROC else Mode.REMOTE,
                    remoteUrl = url,
                    mirror = MirrorOptions(framePacer = ChoreographerFramePacer()),
                    remoteTimeout = 5.seconds,
                    onConnectionChange = ::onConnection,
                    onDevNotice = ::onDevNotice,
                    onPanic = ::onPanic,
                ),
            )
            // A core the dev server replaced is gone: stop reporting to it before the new one gets its own ports.
            platform?.close()
            platform = AndroidPlatformDefaults.install(
                core,
                this,
                // android-work (opt-in): ask WorkManager for a background window when the app goes to the background with work pending.
                // onBackgroundWorkPending = { UndraWork.schedule(this) },
            )
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

    /**
     * Heard on the main thread: one report for every panic the core contained (ADR-046). It carries the message, the location
     * (`file:line:column`), the operation that was running, the stack frames and the identity of the core and its image. Log it
     * and hand it to the crash reporter: with Firebase Crashlytics that is
     * `FirebaseCrashlytics.getInstance().recordException(RuntimeException(report.summary))`, with Sentry
     * `Sentry.captureException(RuntimeException(report.summary))`. Without an `onPanic` the runtime logs each report at error level.
     */
    private fun onPanic(report: UndraPanicReport) {
        Log.e(TAG, "the core panicked: ${report.summary} on ${report.thread} (${report.namespace} ${report.coreVersion}, image ${report.imageId.ifEmpty { "unknown" }})")
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
