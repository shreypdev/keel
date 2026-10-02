package dev.undra.runtime

import dev.undra.runtime.wire.Payloads.CallTarget
import java.net.URI
import java.security.SecureRandom
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * A running Undra core, seen from Kotlin: everything generated code calls (SPEC section 17.2).
 *
 * Get one from the generated entry of the core's bindings, `Undra<Namespace>` (once, at startup); the generated
 * classes of those bindings use it unless they are given another (ADR-044):
 *
 * ```kotlin
 * UndraPlaygroundCore.load(LoadOptions(adapters = mapOf(...)))
 * val todos = TodoStore()                 // uses UndraPlaygroundCore.core
 * ```
 *
 * Several cores (each with its own namespace) can be loaded in one process; each generated entry knows its own.
 *
 * ### Threading
 *
 *  - [callSync] and [construct] run on the calling thread and return when the core is done.
 *  - [call] suspends; the reply resumes the caller on its own dispatcher, never on a core thread.
 *  - Change-sets are applied to stores on the main thread ([UndraDispatchers.main]) through [mirror],
 *    merged and once per display frame ([LoadOptions.mirror]); a reply, a [callSync] on the main thread and
 *    [observe] apply what is queued at once, so code on the main thread reads its own writes.
 *  - Port implementations run off the core's threads (see [PortImpl]).
 *  - The runtime never calls into the core from one of the core's own callbacks.
 *
 * ### Test doubles
 *
 * The class is `open` with a protected constructor so that tests can subclass it and override the
 * members generated code uses; every member of the base class throws [UnsupportedOperationException]
 * (or, for [mirror] and [stats], answers with an inert value). Only a load returns a working core.
 */
public open class UndraCore protected constructor() : AutoCloseable {

    /** Loading a core and the process-wide shared one. */
    public companion object {
        private val slot = AtomicReference<UndraCore?>(null)

        /** The placeholder [shared] returns while no core is loaded. */
        private val unloaded: UndraCore by lazy { UnloadedCore(null) }

        /** Whether the placeholder's "load a core" message has been logged. */
        private val unloadedWarned = AtomicBoolean(false)

        /**
         * The core the first successful load returned (of any core: a generated `Undra<Namespace>.load` or [load]),
         * for app code that uses one core. Generated code never reads it: its classes and functions default to
         * their own core, `Undra<Namespace>.core`.
         *
         * Using it before a successful load, or after the shared core was closed, is a programming error
         * but not a crash (ADR-032, amendment A): it returns a permanently closed placeholder whose calls
         * fail with [UndraCallError.Unavailable] (reason [UndraTransportException.Reason.CLOSED]), whose
         * commands only log, and whose first use logs what to do. [current] still returns `null` in that
         * state, so check it, not this, to learn whether a core is loaded.
         */
        public val shared: UndraCore
            get() {
                slot.get()?.let { return it }
                if (unloadedWarned.compareAndSet(false, true)) {
                    UndraLog.error(
                        "UndraCore.shared was used while no core is loaded (before a load succeeds, or after the shared " +
                            "core was closed); calls on it fail with UndraCallError.Unavailable. Load the core at app " +
                            "startup with its generated entry (Undra<Namespace>.load()), before creating any Undra object.",
                    )
                }
                return unloaded
            }

        /** The core the first successful load returned and that is not closed, or `null`. While it is `null`, [shared] is the closed placeholder. */
        public val current: UndraCore? get() = slot.get()

        /**
         * Connects to a core over `undra dev` ([Mode.REMOTE]) as described by [options] and checks that it was built
         * from the same schema as the bindings ([LoadOptions.expectedSchemaHash], required here). The first successful
         * load also becomes [shared].
         *
         * An in-process core ([Mode.INPROC]) is not loaded here: its library and natives belong to its bindings, so
         * load it through its generated entry, `Undra<Namespace>.load(...)`, which also fills in the schema hash.
         *
         * @throws UndraModeException for [Mode.INPROC] (use the generated entry), when [LoadOptions.expectedSchemaHash]
         *   is not set, or when [options] contradict each other.
         * @throws UndraSchemaMismatchException if the core's schema hash differs.
         * @throws UndraException if the core cannot be reached.
         */
        public fun load(options: LoadOptions): UndraCore {
            if (options.mode == Mode.INPROC) {
                checkModeOptions(options)
                throw UndraModeException(
                    "UndraCore.load(options) cannot load an in-process core: it does not know the core's library. " +
                        "Load it through the generated entry of its bindings, Undra<Namespace>.load(...) " +
                        "(for example UndraPlaygroundCore.load()), or use Mode.REMOTE",
                )
            }
            return start(options, native = null)
        }

        /**
         * Starts the core whose JNI natives are [native] (its generated `UndraCoreNative`) as described by [options],
         * and checks that it was built from the same schema as the bindings ([LoadOptions.expectedSchemaHash], required
         * here). The first successful load also becomes [shared]. This is what the generated `Undra<Namespace>.load`
         * does (through [CoreEntry], which fills in the hash); apps call that instead.
         *
         * In [Mode.INPROC] this checks the core's ABI version and schema hash, initializes it and registers the ports.
         * Only one in-process core per namespace ([NativeApi.namespace]) can be loaded at a time, so a second load of
         * the same core fails until the first one is [close]d (which ends its work, ADR-034); cores with different
         * namespaces run side by side. In [Mode.REMOTE] it connects to `undra dev` and performs the `Hello`
         * handshake, and [native] is not used; see [Mode.REMOTE] for its limits.
         *
         * @throws UndraSchemaMismatchException if the core's schema hash differs.
         * @throws UndraModeException if [LoadOptions.expectedSchemaHash] is not set, or [options] contradict each other.
         * @throws UndraException if the core cannot be started or reached (its library is missing, it speaks another
         *   ABI version, or a core with its namespace is already loaded).
         */
        public fun load(options: LoadOptions, native: NativeApi): UndraCore = start(options) { native }

        /**
         * Starts a core: in process over the natives [native] returns (called only for [Mode.INPROC]), or over
         * `undra dev`.
         */
        internal fun start(options: LoadOptions, native: (() -> NativeApi)?): UndraCore {
            checkModeOptions(options)
            if (options.expectedSchemaHash == null) throw missingSchemaHash()
            val transport = createTransport(options, native)
            return attach(transport, options, makeShared = true)
        }

        /**
         * Connects a core over [transport] and runs the handshake: registers the ports of [options]
         * (explicit adapters first, then the defaults for the rest), asks the transport for the core's
         * schema hash and compares it with [LoadOptions.expectedSchemaHash].
         */
        internal fun attach(transport: Transport, options: LoadOptions, makeShared: Boolean): UndraCore {
            val core = ConnectedCore(
                transport,
                options.remoteTimeout,
                mirrorOptions = options.mirror,
                onConnectionChange = options.onConnectionChange,
                onError = options.onError,
                onDevNotice = options.onDevNotice,
            )
            try {
                val expected = options.expectedSchemaHash ?: throw missingSchemaHash()
                core.installPorts(options)
                val got = transport.connect(core, expected)
                if (got != expected) {
                    throw UndraSchemaMismatchException(expected, got)
                }
                core.markConnected()
            } catch (e: UndraException) {
                core.abandon()
                throw e
            } catch (e: Exception) {
                core.abandon()
                throw UndraException("could not start the Undra core: ${e.message}", e)
            }
            if (makeShared) slot.compareAndSet(null, core)
            return core
        }

        /** Forgets [core] as the shared core, if it is. */
        internal fun forget(core: UndraCore) {
            slot.compareAndSet(core, null)
        }

        private fun missingSchemaHash(): UndraModeException =
            UndraModeException(
                "LoadOptions.expectedSchemaHash is not set: load the core through the generated entry of its bindings, " +
                    "Undra<Namespace>.load(...), which sets it, or pass UndraIds.SCHEMA_HASH",
            )

        /** Refuses [options] that contradict each other: a URL for [Mode.INPROC], or a missing or malformed one for [Mode.REMOTE]. */
        private fun checkModeOptions(options: LoadOptions) {
            when (options.mode) {
                Mode.INPROC -> if (options.remoteUrl != null) {
                    throw UndraModeException("Mode.INPROC does not use remoteUrl (${options.remoteUrl}); did you mean Mode.REMOTE?")
                }
                Mode.REMOTE -> remoteUri(options)
            }
        }

        private fun remoteUri(options: LoadOptions): URI {
            val url = options.remoteUrl ?: throw UndraModeException("Mode.REMOTE needs LoadOptions.remoteUrl (for example ws://localhost:7350)")
            val uri = try {
                URI(url)
            } catch (e: java.net.URISyntaxException) {
                throw UndraModeException("remoteUrl is not a valid URL: $url")
            }
            if (uri.scheme != "ws" && uri.scheme != "wss") {
                throw UndraModeException("remoteUrl must start with ws:// or wss://, got: $url")
            }
            return uri
        }

        private fun createTransport(options: LoadOptions, native: (() -> NativeApi)?): Transport =
            when (options.mode) {
                Mode.INPROC -> {
                    val api = native ?: throw UndraModeException("an in-process core is loaded through the generated entry of its bindings, Undra<Namespace>.load(...)")
                    InprocTransport(api())
                }
                Mode.REMOTE -> RemoteTransport(remoteUri(options), options.remoteTimeout, options.reconnect, session = newSessionToken())
            }

        /** A random token for the dev server to recognise this core's connections by (ADR-051). */
        private fun newSessionToken(): String {
            val bytes = ByteArray(16).also { SecureRandom().nextBytes(it) }
            return bytes.joinToString("") { "%02x".format(it) }
        }
    }

    /** The mode this core runs in. */
    public open val mode: Mode get() = throw unsupported("mode")

    private val inertConnection: StateFlow<ConnectionState> by lazy { MutableStateFlow(ConnectionState.Connected) }

    /**
     * What the connection to the core is doing: [ConnectionState.Connected] from [load] until the core is closed, and,
     * for a [Mode.REMOTE] core, [ConnectionState.Reconnecting] while `undra dev` is unreachable (ADR-051). While it is,
     * calls and [observe] fail at once with [UndraTransportException] (reason [UndraTransportException.Reason.CONNECTION_LOST]; a
     * generated call throws [UndraCallError.Unavailable]), and what was in flight when the connection dropped failed
     * with it; when it is [ConnectionState.Connected] again every store the app observes has been observed
     * again, so the mirrors converge on the core's current values by themselves. A state that is
     * [ConnectionState.Closed] is final.
     *
     * A `StateFlow` conflates: a quick drop and recovery can be seen as no change. [LoadOptions.onConnectionChange]
     * hears every one.
     */
    public open val connectionState: StateFlow<ConnectionState> get() = inertConnection

    /**
     * Calls a synchronous method or function and returns the reply body. In [Mode.INPROC] this is a
     * direct call into the core. In [Mode.REMOTE] it blocks the calling thread for a network round
     * trip (up to [LoadOptions.remoteTimeout]); that is acceptable for development only. Called on the
     * main thread, it returns after the change-sets the call produced have been applied to the stores
     * (from inside a store's `apply`, the running drain applies them in its next round instead).
     *
     * [methodId] must equal the id inside [target] (a [CallTarget.LazyListPage] carries none).
     *
     * This is the raw API, for what generated bindings do not expose: it throws the runtime's own exceptions.
     * Generated code maps them onto [UndraCallError] ([UndraCallError.mapped]).
     *
     * @throws UndraReplyException if the core answers with anything but success.
     * @throws UndraTransportException if the core is closed or unreachable.
     * @throws UndraProtocolException if the core's reply is malformed.
     */
    public open fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray =
        throw unsupported("callSync")

    /**
     * Calls a method or function and suspends for the reply body. Cancelling the coroutine cancels the
     * call in the core (the caller sees [kotlinx.coroutines.CancellationException] at once; the core is
     * told to drop the task). The caller resumes on its own dispatcher; on the main thread, after the
     * change-sets that arrived before the reply have been applied to the stores.
     *
     * @throws UndraReplyException if the core answers with anything but success.
     * @throws UndraTransportException if the core is closed or unreachable.
     */
    public open suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray =
        throw unsupported("call")

    /**
     * Opens a stream and returns its items as a cold [Flow]: every collection starts a new call. Items
     * are delivered with back-pressure: the collector grants the core 16 items when the stream opens
     * and 8 more each time fewer than 8 remain granted, so a slow collector slows the core down.
     * Cancelling the collection cancels the stream in the core. A stream that fails ends the flow in the vocabulary of a
     * failed reply (ADR-036): with [UndraReplyException] of status `ERROR` and the encoded error as its body when the
     * stream ends with its own typed error (flag 2), or with the failure's own status and SPEC 3.4 body when the core
     * ended the stream itself (flag 3: `PANIC`, `CANCELLED` by a restore or a shutdown, `BAD_REQUEST` when it refused
     * it); with [UndraProtocolException] for an item or a failure body the runtime cannot read; and with
     * [UndraTransportException] when the core goes away. Generated code maps them with [UndraCallError.mappedStream].
     */
    public open fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray> =
        throw unsupported("stream")

    /**
     * Runs a constructor and returns the new object's handle. Synchronous: in [Mode.INPROC] a direct
     * call; in [Mode.REMOTE] it blocks (development only). Constructors that are `async` or fallible
     * are called with [call] and a [CallTarget.Constructor] instead.
     *
     * @throws UndraReplyException if the core answers with anything but success.
     * @throws UndraTransportException if the core is closed or unreachable.
     * @throws UndraProtocolException if the core answers with the null handle or a malformed handle.
     */
    public open fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long =
        throw unsupported("construct")

    /**
     * [construct] for generated code: what it throws is mapped onto the closed set ([UndraCallError.mapped]). A
     * secondary constructor cannot hold a `try`, so the generated `constructor(ctx: UndraCore = Undra<Namespace>.core)`
     * delegates through this.
     *
     * @throws UndraCallError whatever [construct] throws.
     */
    public fun constructObject(typeId: UInt, methodId: UInt, args: ByteArray): Long =
        try {
            construct(typeId, methodId, args)
        } catch (e: Exception) {
            throw UndraCallError.mapped(e)
        }

    /**
     * Starts or stops observing a signal of the store [handle] ([signalId] `UInt.MAX_VALUE` means all
     * of them). Starting makes the core report the current values. In [Mode.INPROC] they have been applied
     * to the store when this returns (on the main thread, immediately; from another thread, after
     * waiting for the main thread), so a store never shows its placeholder values to the UI.
     */
    public open fun observe(handle: Long, signalId: UInt, on: Boolean): Unit = throw unsupported("observe")

    /** Releases the object [handle] in the core and stops routing its change-sets. Called by [UndraObject.close]. */
    public open fun release(handle: Long): Unit = throw unsupported("release")

    /** Sends a host-to-core event of an event port (`Connectivity.changed`, `Lifecycle.changed`, ...). */
    public open fun event(portId: UInt, methodId: UInt, payload: ByteArray): Unit = throw unsupported("event")

    /** Tells the core that the timer [timerId] set through the `Timer` port came due. Timer adapters call it; after [close] it does nothing. */
    public open fun timerFired(timerId: UInt): Unit = throw unsupported("timerFired")

    private val inertMirror: Mirror by lazy { Mirror() }

    /** Routes the core's change-sets to stores; see [Mirror]. */
    public open val mirror: Mirror get() = inertMirror

    /**
     * Registers the platform's implementation of a port; later registrations for the same [portId]
     * replace earlier ones (the replaced one is detached: [PortImpl.detach]). Ports can also be supplied up front
     * through [LoadOptions.adapters], which is the better choice when the core calls ports while starting (for example
     * to hydrate caches). When the core closes, every registered implementation is detached.
     */
    public open fun registerPort(portId: UInt, impl: PortImpl): Unit = throw unsupported("registerPort")

    /** Counters of the core and of this host; see [UndraStats]. */
    public open fun stats(): UndraStats = UndraStats(liveHandles = UndraStats.UNKNOWN)

    /**
     * Reports a failure that no caller can see (ADR-032, amendment A): logs it at error level and passes it to
     * [LoadOptions.onError]. Generated commands and store `apply` call it; it never throws (an `Exception` from the
     * handler is logged) and never stops the process.
     *
     * [error] is mapped the way a throwing call's error is ([UndraCallError.mapped]), so the handler always
     * receives an [UndraCallError] inside the [UndraUnhandledError]. The handler runs synchronously on the calling
     * thread. A report made while the handler runs on the same thread (a handler that calls a failing command) is
     * only logged.
     *
     * The base class only logs; a loaded core also calls the handler.
     *
     * @param error what the call threw.
     * @param operation what failed, as Kotlin spells it, for example `"TodoStore.toggle"`.
     */
    public open fun report(error: Throwable, operation: String) {
        val unhandled = UndraUnhandledError(operation, UndraCallError.asCallError(error))
        UndraLog.error(unhandled.message.orEmpty(), error)
    }

    /**
     * Serializes every store (SPEC 5.9), for restoring after a hot reload.
     *
     * @throws UndraModeException over a remote transport.
     * @throws UndraTransportException if this core is closed.
     */
    public open fun snapshot(): ByteArray = throw unsupported("snapshot")

    /**
     * Rebuilds the stores from [snapshot]; the handles the app holds stay valid. Called on the main thread, it
     * applies the restored values to the stores before it returns, like any synchronous call.
     *
     * A snapshot taken by another build restores when its stores' values migrate to this build's types (by name,
     * ADR-037); one that does not is refused as a whole with [UndraRestoreException.INCOMPATIBLE]. A store type this
     * build no longer has is left out (its handle is stale) instead of failing the restore.
     *
     * @throws UndraModeException over a remote transport.
     * @throws UndraRestoreException if the core rejects the snapshot (the core is unchanged); its
     *   [UndraRestoreException.code] says why.
     * @throws UndraTransportException if this core is closed.
     */
    public open fun restore(snapshot: ByteArray): Unit = throw unsupported("restore")

    /**
     * Closes this core: pending calls fail with [UndraTransportException] (reason `CLOSED`), streams end with it, port work is
     * cancelled and the link is closed. **Closing ends the core's work** (ADR-034): an in-process core is
     * shut down (its tasks, timers and port calls stop), and a later load of it in the same process starts a
     * fresh one with fresh handles. Idempotent.
     *
     * **For an in-process core, `close()` waits for the native shutdown**: it returns only after the core's
     * own thread, its timer thread and its blocking pool have been joined and every port callback running
     * on another thread has returned. So do not call it while holding a lock (or waiting on a latch, or
     * inside a `runBlocking`) that a synchronous port implementation needs: the close would wait for the
     * callback and the callback for the close, and a port callback that never returns keeps `close()` from
     * returning. Over a remote transport `close()` only closes the connection.
     *
     * @throws UndraReplyException (status `BAD_REQUEST`, reason `E_REENTRANT`) when called from inside a core
     *   callback (a synchronous port implementation), where the shutdown would wait for the very thread it runs
     *   on; the core is left open.
     */
    override fun close() {}

    private fun unsupported(member: String): UnsupportedOperationException =
        UnsupportedOperationException(
            "UndraCore.$member is not implemented by this UndraCore; load a core (Undra<Namespace>.load(...)) or override it in your test double",
        )
}
