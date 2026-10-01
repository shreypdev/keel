package dev.undra.runtime

import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.Payloads.StreamFlag

/**
 * The link between an [UndraCore] and an Undra core: [InprocTransport] (JNI, same process) or
 * [RemoteTransport] (WebSocket to `undra dev`). Tests plug in a fake.
 *
 * A transport speaks the payloads of SPEC section 3 and knows nothing about call ids, continuations or
 * ports: [UndraCore] owns those. Host-to-core methods may be called from any thread except from inside a
 * [TransportEvents] callback. Core-to-host traffic arrives through the [TransportEvents] given to
 * [connect], on threads the transport owns (see [NativeCallbacks] for what that means in process).
 */
@UndraEmbeddingApi
public interface Transport : AutoCloseable {
    /** Which [Mode] this transport implements. */
    public val mode: Mode

    /**
     * `true` when the core runs in this process: [callSync] is a direct call and
     * `observe(on = true)` has delivered the initial change-set before it returns. `false` for
     * transports where every round trip is a message.
     */
    public val isSynchronous: Boolean

    /**
     * Establishes the link and returns the schema hash of the core at the other end. Events start
     * flowing to [events] only if the hash equals [expectedSchemaHash]; otherwise the transport must not
     * start the core, and the caller closes it and reports an [UndraSchemaMismatchException].
     *
     * @throws UndraException if the link cannot be established.
     */
    public fun connect(events: TransportEvents, expectedSchemaHash: ULong): ULong

    /** Submits a `Call` payload; returns `0` when accepted and the core's bad-request code otherwise. The reply arrives as an event. */
    public fun call(payload: ByteArray): Int

    /**
     * Runs a synchronous call and returns its whole `Reply` payload.
     *
     * @throws UndraModeException if [isSynchronous] is `false`.
     */
    public fun callSync(payload: ByteArray): ByteArray

    /** Cancels the in-flight call or stream [callId]. */
    public fun cancel(callId: UInt)

    /** Grants the stream [callId] [credit] more items. */
    public fun streamCredit(callId: UInt, credit: UInt)

    /** Starts or stops observing [signalId] (`UInt.MAX_VALUE` for all) of the store [handle]. */
    public fun observe(handle: Long, signalId: UInt, on: Boolean)

    /** Releases the object [handle]. */
    public fun release(handle: Long)

    /** Answers a deferred port call; [payload] is a whole `PortReply`. */
    public fun portReply(payload: ByteArray)

    /** Sends a host-to-core event to an event port. */
    public fun event(portId: UInt, methodId: UInt, payload: ByteArray)

    /** Reports that the timer [timerId] came due. */
    public fun timerFired(timerId: UInt)

    /**
     * Serializes every store (SPEC 5.9).
     *
     * @throws UndraModeException if this transport cannot snapshot.
     */
    public fun snapshot(): ByteArray

    /**
     * Restores stores from [snapshot]; returns `0` on success.
     *
     * @throws UndraModeException if this transport cannot restore.
     */
    public fun restore(snapshot: ByteArray): Int

    /** The core's statistics JSON, or `null` when this transport cannot ask for it. */
    public fun statsJson(): String?

    /**
     * Throws if [close] may not run on this thread right now (an in-process core refuses it from inside a
     * core callback). Checked before [UndraCore] starts closing, so a refused close changes nothing.
     */
    public fun checkClose() {}

    /**
     * Detaches from the core and, for an in-process core, ends its work (ADR-034). Idempotent. Pending
     * calls are failed by [UndraCore], not here. An in-process transport **blocks until the native shutdown
     * has finished** (the core's threads joined, port callbacks on other threads returned); [UndraCore.close]
     * documents what that asks of the caller.
     */
    override fun close()
}

/**
 * What a [Transport] tells the [UndraCore] above it. Every method is called on a thread of the
 * transport's choosing and must return quickly without calling back into the transport (the core lock
 * may be held); [UndraCore] copies what it needs and hands the rest to other threads.
 *
 * The byte arrays are owned by the callee: transports pass copies, never views of native memory.
 */
@UndraEmbeddingApi
public interface TransportEvents {
    /** A call finished (or, for a stream, was opened or rejected). */
    public fun onReply(callId: UInt, status: ReplyStatus, body: ByteArray)

    /** A stream produced an item, ended or failed. */
    public fun onStreamItem(callId: UInt, flag: StreamFlag, body: ByteArray)

    /**
     * The core's reply or stream item for [callId] does not decode (the transport learned the call id some
     * other way): the call or stream fails with [error], which generated code reports as
     * `UndraCallError.Malformed`, not as a status the core never sent.
     */
    public fun onMalformed(callId: UInt, error: UndraProtocolException)

    /** A transaction committed; [changeSet] is a whole `ChangeSet` payload. */
    public fun onChangeSet(changeSet: ByteArray)

    /** The core calls a platform port; the answer says how it is being served. */
    public fun onPortCall(portId: UInt, methodId: UInt, portCallId: UInt, args: ByteArray): PortOutcome

    /** A log record from the core (remote transports; in process the core logs through the `Log` port). */
    public fun onLog(level: UByte, target: String, message: String)

    /** The link went down for good. [cause] is `null` after a deliberate close. */
    public fun onClosed(cause: Throwable?)

    /**
     * Only for a transport that reconnects (remote, ADR-051): the link dropped (or a retry failed) and the
     * transport will try again. [attempt] counts from 1, and attempt 1 is the loss itself: whatever was in flight
     * has failed for good. [onClosed] follows only if the transport gives up.
     */
    public fun onReconnecting(attempt: Int, cause: Throwable?) {}

    /**
     * Only for a transport that reconnects: the link is back and the core's `Hello` was checked. The core observes
     * its stores again (on a thread of its own: this callback must not call into the transport).
     */
    public fun onReconnected() {}

    /** Only for a transport that reconnects: whether the core holds objects it expects the server to still have (it asks the server to resume them). */
    public fun holdsObjects(): Boolean = false
}

/** How a port call is being served (SPEC 6.3). */
@UndraEmbeddingApi
public sealed interface PortOutcome {
    /** Answered inline; [reply] is a whole `PortReply` payload. */
    public class Sync(public val reply: ByteArray) : PortOutcome

    /** Answered later through [Transport.portReply]. */
    public data object Async : PortOutcome

    /** The port is not registered, or the call failed in a way the core should see as `PortError::Unavailable`. */
    public data object Unavailable : PortOutcome
}
