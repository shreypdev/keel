package dev.undra.runtime

import java.nio.ByteBuffer

/**
 * The JNI facade over the native core (`undra-ffi`, feature `jni`; SPEC section 6.1).
 *
 * Every method is a `static native` of class `dev.undra.runtime.UndraNative` with exactly the signature
 * the shim registers through `JNI_OnLoad` / `RegisterNatives`:
 *
 * ```
 * static native int    abiVersion();                                       // ()I
 * static native long   schemaHash();                                       // ()J
 * static native byte[] schemaJson();                                       // ()[B
 * static native int    init(byte[] cfg, UndraNative.Callbacks cb);          // ([BLdev/undra/runtime/UndraNative$Callbacks;)I
 * static native int    call(byte[] payload);                               // ([B)I
 * static native byte[] callSync(byte[] payload);                           // ([B)[B
 * static native void   cancel(int callId);                                 // (I)V
 * static native void   streamCredit(int callId, int credit);               // (II)V
 * static native void   observe(long handle, int signalId, boolean on);     // (JIZ)V
 * static native void   release(long handle);                               // (J)V
 * static native void   portReply(byte[] payload);                          // ([B)V
 * static native void   event(int portId, int methodId, byte[] payload);    // (II[B)V
 * static native void   timerFired(int timerId);                            // (I)V
 * static native byte[] snapshot();                                         // ()[B
 * static native int    restore(byte[] snapshot);                           // ([B)I
 * static native String statsJson();                                        // ()Ljava/lang/String;
 * ```
 *
 * `Callbacks` is `dev.undra.runtime.UndraNative$Callbacks` with the methods `onReply(ILjava/nio/ByteBuffer;)V`,
 * `onChangeSet(Ljava/nio/ByteBuffer;)V`, `onStream(ILjava/nio/ByteBuffer;)V`,
 * `onPortCall(IIILjava/nio/ByteBuffer;)I` and `portSyncReply()[B`.
 *
 * Nothing here is meant to be called directly by applications: [UndraCore.load] drives it. The natives
 * are only usable when [isAvailable] is `true`.
 *
 * ### Loading the library
 *
 * The library is loaded once, when this object is first touched, with `System.loadLibrary(name)`. The
 * name is the system property `undra.native.name` (default `undra_core`, so `libundra_core.so`,
 * `libundra_core.dylib` or `undra_core.dll`). The system property `undra.native.path`, when set, is an
 * absolute path to the library file and takes precedence (`System.load`); it is meant for tests and
 * development builds. A missing library does not throw: [isAvailable] is `false` and
 * [unavailableReason] holds the `UnsatisfiedLinkError`, so a JVM without the native core can still
 * use `Mode.REMOTE`.
 *
 * ### Threading contract
 *
 * The callbacks are invoked from the core thread, a blocking-pool thread or the calling thread, possibly
 * while the core lock is held, and the [ByteBuffer]s they receive are **direct buffers over core memory
 * that are valid only until the callback returns**. An implementation must copy what it needs and must
 * not call any native method from inside a callback (the core would deadlock or answer `E_REENTRANT`).
 */
public object UndraNative {

    /** System property naming the native library (without the platform prefix and suffix). */
    public const val NAME_PROPERTY: String = "undra.native.name"

    /** System property holding the absolute path of the native library file; wins over [NAME_PROPERTY]. */
    public const val PATH_PROPERTY: String = "undra.native.path"

    /** The library name used when [NAME_PROPERTY] is not set. */
    public const val DEFAULT_NAME: String = "undra_core"

    private val loadFailure: Throwable? = tryLoad()

    /** `true` when the native core library was loaded and the natives below may be called. */
    public val isAvailable: Boolean get() = loadFailure == null

    /** Why the native library could not be loaded, or `null` when [isAvailable]. */
    public val unavailableReason: Throwable? get() = loadFailure

    /**
     * What the native core calls back with. See the class documentation for the threading contract.
     * Implementations must not throw: an exception crossing back into native code is undefined
     * behaviour for the shim.
     */
    public interface Callbacks {
        /** A call finished. [reply] is a whole `Reply` payload (SPEC 3.4: `call_id u32, status u8, body`). */
        public fun onReply(callId: Int, reply: ByteBuffer)

        /** A transaction committed. [changes] is a whole `ChangeSet` payload (SPEC 3.5). */
        public fun onChangeSet(changes: ByteBuffer)

        /** A stream produced something. [item] is a whole `StreamItem` payload (SPEC 3.7). */
        public fun onStream(callId: Int, item: ByteBuffer)

        /**
         * The core calls a platform port (SPEC 6.3). [args] are the encoded parameters only.
         *
         * @return `0` when the call was answered synchronously (the core then reads the whole `PortReply`
         *   payload from [portSyncReply] on the same thread), `1` when the answer will come later through
         *   [portReply], `2` when the port is unavailable.
         */
        public fun onPortCall(portId: Int, methodId: Int, portCallId: Int, args: ByteBuffer): Int

        /**
         * The `PortReply` payload (SPEC 3.6) of the port call that [onPortCall] just answered with `0`.
         *
         * The core calls it on the same thread, right after [onPortCall] returned `0`, while callbacks run
         * concurrently on other threads: hand the reply over through thread-local state (as `InprocTransport`
         * does), never through a field that all threads share.
         */
        public fun portSyncReply(): ByteArray
    }

    /** The ABI version of the library; this runtime speaks `1`. */
    @JvmStatic
    public external fun abiVersion(): Int

    /** The schema hash (`u64`, as a signed `long`) the core was built with. */
    @JvmStatic
    public external fun schemaHash(): Long

    /** The core's schema as JSON, doc comments included, UTF-8 encoded. */
    @JvmStatic
    public external fun schemaJson(): ByteArray

    /**
     * Starts the core with [cfg] (an encoded `RuntimeConfig`) and registers [cb]; returns `0` on success.
     * Idempotent per process.
     */
    @JvmStatic
    public external fun init(cfg: ByteArray, cb: Callbacks): Int

    /** Submits a `Call` payload (SPEC 3.3); `0` accepted, `5` bad request. The reply arrives through [Callbacks.onReply]. */
    @JvmStatic
    public external fun call(payload: ByteArray): Int

    /** Runs a synchronous method and returns its whole `Reply` payload (SPEC 3.4). */
    @JvmStatic
    public external fun callSync(payload: ByteArray): ByteArray

    /** Cancels the in-flight call or stream [callId]. */
    @JvmStatic
    public external fun cancel(callId: Int)

    /** Grants the stream [callId] [credit] more items. */
    @JvmStatic
    public external fun streamCredit(callId: Int, credit: Int)

    /** Starts (`on`) or stops observing [signalId] of the store [handle]; `-1` means every signal. Starting delivers the current values through [Callbacks.onChangeSet] before it returns. */
    @JvmStatic
    public external fun observe(handle: Long, signalId: Int, on: Boolean)

    /** Releases the object [handle]. */
    @JvmStatic
    public external fun release(handle: Long)

    /** Answers a port call that [Callbacks.onPortCall] deferred; [payload] is a whole `PortReply` (SPEC 3.6). */
    @JvmStatic
    public external fun portReply(payload: ByteArray)

    /** Delivers a host-to-core event of an event port. */
    @JvmStatic
    public external fun event(portId: Int, methodId: Int, payload: ByteArray)

    /** Tells the core that the timer [timerId] came due. */
    @JvmStatic
    public external fun timerFired(timerId: Int)

    /** Serializes every store (SPEC 5.9). */
    @JvmStatic
    public external fun snapshot(): ByteArray

    /** Rebuilds the stores from [snapshot]; `0` on success. */
    @JvmStatic
    public external fun restore(snapshot: ByteArray): Int

    /** Runtime statistics as a JSON document. */
    @JvmStatic
    public external fun statsJson(): String

    private fun tryLoad(): Throwable? =
        try {
            val path = System.getProperty(PATH_PROPERTY)
            if (path != null && path.isNotEmpty()) {
                System.load(path)
            } else {
                System.loadLibrary(System.getProperty(NAME_PROPERTY, DEFAULT_NAME))
            }
            null
        } catch (e: UnsatisfiedLinkError) {
            e
        } catch (e: SecurityException) {
            e
        }
}
