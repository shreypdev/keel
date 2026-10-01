package dev.undra.runtime

import dev.undra.runtime.wire.Payloads.CallTarget
import java.net.URI
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.flow.Flow

/**
 * A running Undra core, seen from Kotlin: everything generated code calls (SPEC section 17.2).
 *
 * Get one with [load] (once, at startup), then let generated classes use [shared]:
 *
 * ```kotlin
 * UndraCore.load(LoadOptions(expectedSchemaHash = UndraIds.SCHEMA_HASH, adapters = mapOf(...)))
 * val todos = TodoStore()                 // uses UndraCore.shared
 * ```
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
 * (or, for [mirror] and [stats], answers with an inert value). Only [load] returns a working core.
 */
public open class UndraCore protected constructor() : AutoCloseable {

    /** Loading a core and the process-wide shared one. */
    public companion object {
        private val current = AtomicReference<UndraCore?>(null)

        /**
         * The core the first successful [load] returned. Generated constructors default to it.
         *
         * @throws UndraException if no core has been loaded (or the first one was closed).
         */
        public val shared: UndraCore
            get() = current.get()
                ?: throw UndraException("no UndraCore has been loaded: call UndraCore.load(LoadOptions(...)) before using generated bindings")

        /**
         * Starts a core as described by [options] and checks that it was built from the same schema as
         * the bindings ([LoadOptions.expectedSchemaHash]). The first successful call also becomes [shared].
         *
         * In [Mode.INPROC] this loads the native library (see [UndraNative]), initializes the core and
         * registers the ports; only one in-process core can exist per process, and it cannot be
         * unloaded, so a second `load(INPROC)` fails. In [Mode.REMOTE] it connects to `undra dev` and
         * performs the `Hello` handshake; see [Mode.REMOTE] for its limits.
         *
         * @throws UndraSchemaMismatchException if the core's schema hash differs.
         * @throws UndraModeException if [options] contradict each other or the mode is unavailable here.
         * @throws UndraException if the core cannot be started or reached.
         */
        public fun load(options: LoadOptions): UndraCore {
            val transport = createTransport(options)
            return attach(transport, options, makeShared = true)
        }

        /**
         * Connects a core over [transport] and runs the handshake: registers the ports of [options]
         * (explicit adapters first, then the defaults for the rest), asks the transport for the core's
         * schema hash and compares it with [LoadOptions.expectedSchemaHash].
         */
        internal fun attach(transport: Transport, options: LoadOptions, makeShared: Boolean): UndraCore {
            val core = ConnectedCore(transport, options.remoteTimeout, mirrorOptions = options.mirror)
            try {
                core.installPorts(options)
                val got = transport.connect(core, options.expectedSchemaHash)
                if (got != options.expectedSchemaHash) {
                    throw UndraSchemaMismatchException(options.expectedSchemaHash, got)
                }
            } catch (e: UndraException) {
                core.abandon()
                throw e
            } catch (e: Exception) {
                core.abandon()
                throw UndraException("could not start the Undra core: ${e.message}", e)
            }
            if (makeShared) current.compareAndSet(null, core)
            return core
        }

        /** Forgets [core] as the shared core, if it is. */
        internal fun forget(core: UndraCore) {
            current.compareAndSet(core, null)
        }

        private fun createTransport(options: LoadOptions): Transport =
            when (options.mode) {
                Mode.INPROC -> {
                    if (options.remoteUrl != null) {
                        throw UndraModeException("Mode.INPROC does not use remoteUrl (${options.remoteUrl}); did you mean Mode.REMOTE?")
                    }
                    InprocTransport()
                }
                Mode.REMOTE -> {
                    val url = options.remoteUrl ?: throw UndraModeException("Mode.REMOTE needs LoadOptions.remoteUrl (for example ws://localhost:7350)")
                    val uri = try {
                        URI(url)
                    } catch (e: java.net.URISyntaxException) {
                        throw UndraModeException("remoteUrl is not a valid URL: $url")
                    }
                    if (uri.scheme != "ws" && uri.scheme != "wss") {
                        throw UndraModeException("remoteUrl must start with ws:// or wss://, got: $url")
                    }
                    try {
                        RemoteTransport(uri, options.remoteTimeout)
                    } catch (e: LinkageError) {
                        throw UndraModeException("Mode.REMOTE needs java.net.http, which this platform does not provide (${e.message})")
                    }
                }
            }
    }

    /** The mode this core runs in. */
    public open val mode: Mode get() = throw unsupported("mode")

    /**
     * Calls a synchronous method or function and returns the reply body. In [Mode.INPROC] this is a
     * direct call into the core. In [Mode.REMOTE] it blocks the calling thread for a network round
     * trip (up to [LoadOptions.remoteTimeout]); that is acceptable for development only. Called on the
     * main thread, it returns after the change-sets the call produced have been applied to the stores.
     *
     * [methodId] must equal the id inside [target] (a [CallTarget.LazyListPage] carries none).
     *
     * @throws UndraReplyException if the core answers with anything but success.
     * @throws UndraException if the core is closed or unreachable.
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
     * @throws UndraException if the core is closed or unreachable.
     */
    public open suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray =
        throw unsupported("call")

    /**
     * Opens a stream and returns its items as a cold [Flow]: every collection starts a new call. Items
     * are delivered with back-pressure: the collector grants the core 16 items when the stream opens
     * and 8 more each time fewer than 8 remain granted, so a slow collector slows the core down.
     * Cancelling the collection cancels the stream in the core. A stream that fails ends the flow with
     * [UndraReplyException] (`status == ERROR`, body the encoded error).
     */
    public open fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray> =
        throw unsupported("stream")

    /**
     * Runs a constructor and returns the new object's handle. Synchronous: in [Mode.INPROC] a direct
     * call; in [Mode.REMOTE] it blocks (development only). Constructors that are `async` or fallible
     * are called with [call] and a [CallTarget.Constructor] instead.
     *
     * @throws UndraReplyException if the core answers with anything but success.
     */
    public open fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long =
        throw unsupported("construct")

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
     * replace earlier ones. Ports can also be supplied up front through [LoadOptions.adapters], which
     * is the better choice when the core calls ports while starting (for example to hydrate caches).
     */
    public open fun registerPort(portId: UInt, impl: PortImpl): Unit = throw unsupported("registerPort")

    /** Counters of the core and of this host; see [UndraStats]. */
    public open fun stats(): UndraStats = UndraStats(liveHandles = UndraStats.UNKNOWN)

    /**
     * Serializes every store (SPEC 5.9), for restoring after a hot reload.
     *
     * @throws UndraModeException over a remote transport.
     */
    public open fun snapshot(): ByteArray = throw unsupported("snapshot")

    /**
     * Rebuilds the stores from [snapshot]; the handles the app holds stay valid.
     *
     * @throws UndraModeException over a remote transport.
     * @throws UndraException if the core rejects the snapshot.
     */
    public open fun restore(snapshot: ByteArray): Unit = throw unsupported("restore")

    /**
     * Detaches this host from the core: pending calls fail with [UndraException], streams end with it,
     * port work is cancelled and the link is closed. An in-process core keeps running (the native library
     * cannot be unloaded) and cannot be loaded again in this process. Idempotent.
     */
    override fun close() {}

    private fun unsupported(member: String): UnsupportedOperationException =
        UnsupportedOperationException(
            "UndraCore.$member is not implemented by this UndraCore; use UndraCore.load(...) or override it in your test double",
        )
}
