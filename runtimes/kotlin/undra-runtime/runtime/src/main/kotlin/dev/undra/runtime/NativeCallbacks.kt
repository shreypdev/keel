package dev.undra.runtime

import java.nio.ByteBuffer

/**
 * What a native core calls back into (SPEC section 6.1): the object the runtime passes to [NativeApi.init].
 *
 * Every core's JNI shim (`undra-ffi`, feature `jni`) resolves this interface by its name,
 * `dev.undra.runtime.NativeCallbacks`, and its methods by name and descriptor:
 *
 * ```
 * onReply       (ILjava/nio/ByteBuffer;)V
 * onChangeSet   (Ljava/nio/ByteBuffer;)V
 * onStream      (ILjava/nio/ByteBuffer;)V
 * onPortCall    (IIILjava/nio/ByteBuffer;)I
 * portSyncReply ()[B
 * ```
 *
 * The interface is shared by every core in the process and declares no natives (ADR-044); the runtime's
 * R8 rules (`META-INF/proguard/undra-runtime.pro`) keep it and the methods of its implementations.
 *
 * ### Threading contract
 *
 * The callbacks are invoked from the core thread, a blocking-pool thread or the calling thread, possibly
 * while the core lock is held, and the [ByteBuffer]s they receive are **direct buffers over core memory
 * that are valid only until the callback returns**. An implementation must copy what it needs, must not
 * call any native method from inside a callback (the core would deadlock or answer `E_REENTRANT`), and must
 * not throw: an exception crossing back into native code is treated as "unavailable" by the shim.
 *
 * Apps do not implement it: the in-process transport of [UndraCore] does.
 */
public interface NativeCallbacks {
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
     *   [NativeApi.portReply], `2` when the port is unavailable.
     */
    public fun onPortCall(portId: Int, methodId: Int, portCallId: Int, args: ByteBuffer): Int

    /**
     * The `PortReply` payload (SPEC 3.6) of the port call that [onPortCall] just answered with `0`.
     *
     * The core calls it on the same thread, right after [onPortCall] returned `0`, while callbacks run
     * concurrently on other threads: hand the reply over through thread-local state (as the in-process
     * transport does), never through a field that all threads share.
     */
    public fun portSyncReply(): ByteArray
}
