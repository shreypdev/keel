package dev.undra.runtime

/**
 * The JNI natives of one native core (SPEC section 6.1, ADR-044).
 *
 * It is implemented by the generated `UndraCoreNative` object of each core's bindings: an `object` whose
 * members are `override external fun`s, loaded with [NativeLibrary.load] of the core's [namespace]. The core's
 * `JNI_OnLoad` registers its natives on that class by name and descriptor, so two cores in one process each
 * bind their own class and never one another's:
 *
 * ```
 * abiVersion   ()I                      schemaHash ()J          schemaJson ()[B
 * init         ([BLdev/undra/runtime/NativeCallbacks;)I
 * call         ([B)I                    callSync ([B)[B         cancel (I)V
 * streamCredit (II)V                    observe (JIZ)V          release (J)V
 * portReply    ([B)V                    event (II[B)V           timerFired (I)V
 * snapshot     ()[B                     restore ([B)I           statsJson ()Ljava/lang/String;
 * shutdown     ()V
 * ```
 *
 * Apps do not implement or call it: the generated `Undra<Namespace>.load()` hands it to
 * [UndraCore.load] (through [CoreEntry]), and the in-process transport drives it. The natives may only be
 * called while [isAvailable] is `true`.
 */
public interface NativeApi {
    /**
     * The core's namespace (`[core] namespace` in `undra.toml`): the name of its library (`lib<namespace>.so`)
     * and the key of its in-process claim, so two cores with one namespace cannot both be loaded.
     */
    public val namespace: String

    /** `true` when the core's library was loaded and the natives below may be called. */
    public val isAvailable: Boolean

    /** Why the core's library could not be loaded, or `null` when [isAvailable]. */
    public val unavailableReason: Throwable?

    /** The version of the core's native ABI (SPEC 6); this runtime speaks `2`. */
    public fun abiVersion(): Int

    /** The schema hash (`u64`, as a signed `long`) the core was built with. */
    public fun schemaHash(): Long

    /** The core's schema as JSON, doc comments included, UTF-8 encoded. */
    public fun schemaJson(): ByteArray

    /**
     * Starts the core with [cfg] (an encoded `RuntimeConfig`) and registers [cb]; returns `0` on success.
     * Idempotent per core until [shutdown].
     */
    public fun init(cfg: ByteArray, cb: NativeCallbacks): Int

    /** Submits a `Call` payload (SPEC 3.3); `0` accepted, `5` bad request. The reply arrives through [NativeCallbacks.onReply]. */
    public fun call(payload: ByteArray): Int

    /** Runs a synchronous method and returns its whole `Reply` payload (SPEC 3.4). */
    public fun callSync(payload: ByteArray): ByteArray

    /** Cancels the in-flight call or stream [callId]. */
    public fun cancel(callId: Int)

    /** Grants the stream [callId] [credit] more items. */
    public fun streamCredit(callId: Int, credit: Int)

    /**
     * Starts (`on`) or stops observing [signalId] of the store [handle]; `-1` means every signal. Starting
     * delivers the current values through [NativeCallbacks.onChangeSet] before it returns.
     */
    public fun observe(handle: Long, signalId: Int, on: Boolean)

    /** Releases the object [handle]. */
    public fun release(handle: Long)

    /** Answers a port call that [NativeCallbacks.onPortCall] deferred; [payload] is a whole `PortReply` (SPEC 3.6). */
    public fun portReply(payload: ByteArray)

    /** Delivers a host-to-core event of an event port. */
    public fun event(portId: Int, methodId: Int, payload: ByteArray)

    /** Tells the core that the timer [timerId] came due. */
    public fun timerFired(timerId: Int)

    /** Serializes every store (SPEC 5.9). */
    public fun snapshot(): ByteArray

    /** Rebuilds the stores from [snapshot]; `0` on success. */
    public fun restore(snapshot: ByteArray): Int

    /** Runtime statistics as a JSON document. */
    public fun statsJson(): String

    /**
     * Ends the core's work (ADR-034): every call in flight is answered as cancelled and every open stream
     * ends, the core's threads stop, the port registrations go and the [NativeCallbacks] object is released.
     * A later [init] starts a new core. Idempotent. Must not be called from inside a callback (it would wait
     * for the thread it runs on).
     */
    public fun shutdown()
}
