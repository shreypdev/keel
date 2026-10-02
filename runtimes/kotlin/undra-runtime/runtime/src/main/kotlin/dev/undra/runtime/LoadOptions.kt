package dev.undra.runtime

import dev.undra.runtime.adapters.UndraPanicReport
import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds

/** Where the core runs relative to the app. */
public enum class Mode {
    /**
     * In this process, through the JNI natives of the core's generated `UndraCoreNative` ([NativeApi]). The
     * production mode. Load an in-process core through its generated entry, `Undra<Namespace>.load()`.
     */
    INPROC,

    /**
     * In another process (`undra dev`), over a WebSocket. **A development mode**: every call is a network
     * round trip, and `callSync` and `construct` block the calling thread until the reply arrives, so
     * do not use it to ship an app.
     */
    REMOTE,
}

/**
 * How a core is started: by its generated entry, `Undra<Namespace>.load(options)` (which fills in
 * [expectedSchemaHash]), or by [UndraCore.load].
 *
 * ```kotlin
 * UndraPlaygroundCore.load()                                                              // in this process
 * UndraPlaygroundCore.load(LoadOptions(Mode.REMOTE, remoteUrl = "ws://10.0.2.2:7878"))   // `undra dev`
 * ```
 *
 * @property mode [Mode.INPROC] (default) or [Mode.REMOTE].
 * @property remoteUrl the `ws://` or `wss://` URL of the `undra dev` server; required for [Mode.REMOTE]
 *   and rejected for [Mode.INPROC].
 * @property adapters platform implementations of ports, by port id (`UndraIds.Ports.<Trait>.PORT_ID`),
 *   made with the generated `<trait>PortImpl(...)` functions. They take precedence over the default
 *   adapters.
 * @property expectedSchemaHash the schema hash of the bindings (`UndraIds.SCHEMA_HASH`); loading fails
 *   with [UndraSchemaMismatchException] if the core reports another one. `null` (the default) lets the generated
 *   `Undra<Namespace>.load` fill it in; [UndraCore.load] refuses options without it.
 * @property defaultAdapters install the JVM default adapters (`dev.undra.runtime.adapters.JvmAdapters`) for
 *   every standard port not in [adapters]. On Android only the portable ones (Clock, Rng, Log, Timer) are
 *   installed; Http, Kv, SecureStore, Fs, Connectivity and Lifecycle come from the `android-adapters` module, which
 *   installs all ten with `AndroidPlatformDefaults.install(core, context)` after the core is loaded.
 * @property namespace the core's namespace (`UndraIds.NAMESPACE`), which the default `Kv`, `SecureStore`, `Fs` and `Db`
 *   adapters keep their data under (`<dataDir>/<namespace>/...`, `<filesDir>/undra/<namespace>/...`, ADR-044 amendment A):
 *   two cores of one app never share a store. `null` (the default) lets the generated `Undra<Namespace>.load` fill it
 *   in, and an in-process core takes its natives'; a core loaded without either is [UndraCore.UNNAMED_NAMESPACE]. An
 *   adapter given its own directory or alias ignores it.
 * @property remoteTimeout how long a blocking call (`callSync`, `construct`) and the connection
 *   handshake wait for the remote core before giving up.
 * @property mirror how change-sets are delivered to stores: the frame pacer and the backlog bounds.
 * @property reconnect how a [Mode.REMOTE] core reconnects by itself when its connection drops (ADR-051):
 *   `null` turns it off, and a drop then closes the core. The default is on, with [ReconnectPolicy]'s defaults.
 * @property onConnectionChange called with every change of [UndraCore.connectionState], starting with
 *   [ConnectionState.Connecting], on the thread that changed it (a thread of the runtime's own for a reconnect). It
 *   must return quickly and must not call into the core.
 * @property onError called with every failure that has no caller to throw to (ADR-032, amendment A): a generated
 *   command (a synchronous method that returns nothing and has no error type) that failed, a store change that
 *   could not be applied, a malformed change-set, a port implementation that failed. The failure is also logged at
 *   error level, whether or not a handler is set. The handler runs synchronously on the thread that made the call
 *   (the main thread for a store's `apply` and for a command called from a click handler); keep it short and do not
 *   call into Undra from it: a failure reported while a handler runs on the same thread is only logged. An
 *   `Exception` it throws is logged and dropped; an `Error` propagates, so a debug build can crash on purpose with
 *   `onError = { throw AssertionError(it) }`. Failures that originate in a core callback (a malformed change-set, a
 *   failed port) are delivered from the runtime's delivery thread instead.
 * @property onDevNotice **Development only, and inert unless the core is a [Mode.REMOTE] one served by `undra dev`.** Called
 *   with a one-line message the dev server says about itself, such as `Reloaded, state kept` after it rebuilt the core
 *   (ADR-053): show it in a status bar for a few seconds. `undra dev` tells every client that attaches soon after a
 *   rebuild, once; an in-process or production core never produces one, so the callback never fires there. The message
 *   is also written to the log. It runs on a thread of the runtime's own (the delivery thread), never on the transport's:
 *   keep it short, and hop to the main thread before touching UI. An exception it throws is logged and dropped.
 * @property onPanic called with one [UndraPanicReport] for every panic the core contained (ADR-046): the message, `file:line:column`,
 *   the operation that was running, the thread, the stack frames (addresses, and symbols when the build has them) and the identity of
 *   the core and its image, which is what a crash reporter needs (Crashlytics, Sentry, Play Console). The call that panicked still fails
 *   as before (`UndraCallError.Panicked`) and the core keeps working; this is the report on top. It runs on the runtime's main
 *   dispatcher (the main thread on Android), once per report, in the order the panics happened, after the core's own FATAL log record;
 *   the core is never blocked on it. An `Exception` it throws is logged and reported to [onError] (operation `onPanic`), and the next
 *   panic is reported all the same. When it is `null` the runtime logs each report at error level instead (one line: the operation, the
 *   message and the location), so a panic is never silent. It is delivered through the `Diagnostics` port, which the runtime registers
 *   for every core it loads, also when [defaultAdapters] is `false`; an adapter of your own for that port in [adapters] replaces it.
 */
public class LoadOptions(
    public val mode: Mode = Mode.INPROC,
    public val remoteUrl: String? = null,
    public val adapters: Map<UInt, PortImpl> = emptyMap(),
    public val expectedSchemaHash: ULong? = null,
    public val defaultAdapters: Boolean = true,
    public val remoteTimeout: Duration = 30.seconds,
    public val mirror: MirrorOptions = MirrorOptions(),
    public val reconnect: ReconnectPolicy? = ReconnectPolicy(),
    public val onConnectionChange: ((ConnectionState) -> Unit)? = null,
    public val onError: ((UndraUnhandledError) -> Unit)? = null,
    public val onDevNotice: ((String) -> Unit)? = null,
    public val namespace: String? = null,
    public val onPanic: ((UndraPanicReport) -> Unit)? = null,
) {
    override fun toString(): String =
        "LoadOptions(mode=$mode, remoteUrl=$remoteUrl, adapters=${adapters.keys.sorted()}, " +
            "expectedSchemaHash=${expectedSchemaHash?.let { "0x" + it.toString(16) } ?: "unset"}, defaultAdapters=$defaultAdapters, remoteTimeout=$remoteTimeout, " +
            "mirror=$mirror, reconnect=$reconnect, onError=${if (onError == null) "none" else "set"}, " +
            "namespace=${namespace ?: "unset"}, onPanic=${if (onPanic == null) "none" else "set"})"

    /** These options with [expectedSchemaHash] set to [hash] when it is not set already. */
    internal fun withSchemaHashDefault(hash: ULong): LoadOptions =
        if (expectedSchemaHash != null) this else copyWith(expectedSchemaHash = hash, namespace = namespace)

    /** These options with [namespace] set to [value] when it is not set already. */
    internal fun withNamespaceDefault(value: String): LoadOptions =
        if (namespace != null) this else copyWith(expectedSchemaHash = expectedSchemaHash, namespace = value)

    private fun copyWith(expectedSchemaHash: ULong?, namespace: String?): LoadOptions =
        LoadOptions(
            mode = mode,
            remoteUrl = remoteUrl,
            adapters = adapters,
            expectedSchemaHash = expectedSchemaHash,
            defaultAdapters = defaultAdapters,
            remoteTimeout = remoteTimeout,
            mirror = mirror,
            reconnect = reconnect,
            onConnectionChange = onConnectionChange,
            onError = onError,
            onDevNotice = onDevNotice,
            namespace = namespace,
            onPanic = onPanic,
        )
}

/**
 * How the [Mirror] of a loaded core delivers change-sets (ADR-031, SPEC section 11).
 *
 * ```kotlin
 * // Android: drain at the display's own frames (module android-adapters).
 * UndraPlaygroundCore.load(LoadOptions(mirror = MirrorOptions(framePacer = ChoreographerFramePacer())))
 * ```
 *
 * @property framePacer when the change-sets the core produced on its own are applied. `null` (the default)
 *   uses the runtime's own pacer: a daemon thread named `undra-frame` that posts a drain to the main thread
 *   on a 60 Hz grid. Replies, `callSync` on the main thread and `observe` never wait for a frame.
 * @property maxPendingEntries when more entries than this wait for a drain, the backlog is folded in place.
 * @property maxPendingBytes when the waiting entries hold more bytes than this (values plus 17 bytes per
 *   entry), the backlog is folded in place.
 * @throws IllegalArgumentException if a bound is not positive.
 */
public class MirrorOptions(
    public val framePacer: FramePacer? = null,
    public val maxPendingEntries: Int = 65_536,
    public val maxPendingBytes: Long = 16L * 1024 * 1024,
) {
    init {
        require(maxPendingEntries > 0) { "maxPendingEntries must be positive, got $maxPendingEntries" }
        require(maxPendingBytes > 0) { "maxPendingBytes must be positive, got $maxPendingBytes" }
    }

    override fun toString(): String =
        "MirrorOptions(framePacer=${framePacer?.javaClass?.name ?: "default"}, maxPendingEntries=$maxPendingEntries, " +
            "maxPendingBytes=$maxPendingBytes)"
}
