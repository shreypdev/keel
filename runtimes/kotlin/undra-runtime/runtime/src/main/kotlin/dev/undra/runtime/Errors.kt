package dev.undra.runtime

import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.PanicInfo
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.WireException

/**
 * Base class of every exception the Undra runtime and generated bindings throw on purpose.
 *
 * Generated error types (`#[undra::error]`) extend it, so `catch (e: UndraException)` catches both the
 * typed errors of your core and the runtime's own failures: [UndraCallError] (what a generated call
 * throws besides its own error type, ADR-032 amendment A), and the raw ones behind it
 * ([UndraReplyException], [UndraTransportException], [UndraProtocolException],
 * [dev.undra.runtime.wire.WireException], [UndraModeException], [UndraSchemaMismatchException],
 * [UndraRestoreException]).
 */
public open class UndraException(message: String, cause: Throwable? = null) : RuntimeException(message, cause)

/**
 * A call, constructor or stream finished with something other than a success: the core answered with
 * a non-`OK` [ReplyStatus] (SPEC section 3.4), or a stream ended with the error flag (section 3.7).
 *
 * This is the *raw* failure of [UndraCore.callSync], [UndraCore.call], [UndraCore.stream] and
 * [UndraCore.construct], for what bindings do not expose. Generated code never lets it through: it throws the
 * method's own typed error for the `ERROR` status and an [UndraCallError] for every other one
 * ([UndraCallError.mapped]).
 *
 * @property status what the core answered: [ReplyStatus.ERROR] (the [body] is the encoded typed error),
 *   [ReplyStatus.PANIC] (see [panicInfo]), [ReplyStatus.CANCELLED], [ReplyStatus.BAD_REQUEST] (see
 *   [badRequestReason]) or [ReplyStatus.STREAM_OPENED] when a stream was opened where none was expected.
 * @property body the raw reply body; its meaning depends on [status].
 */
public class UndraReplyException(public val status: ReplyStatus, public val body: ByteArray) :
    UndraException(describe(status, body)) {

    /** The panic message and backtrace of a [ReplyStatus.PANIC] reply, or `null` for other statuses or an unreadable body. */
    public val panicInfo: PanicInfo?
        get() = if (status == ReplyStatus.PANIC) readOrNull { it.readPanic() } else null

    /** The reason of a [ReplyStatus.BAD_REQUEST] reply, or `null` for other statuses or an unreadable body. */
    public val badRequestReason: String?
        get() = if (status == ReplyStatus.BAD_REQUEST) readOrNull { it.readBadRequestReason() } else null

    private fun <T> readOrNull(read: (Payloads.Reply) -> T): T? =
        try {
            read(Payloads.Reply(0u, status, body))
        } catch (e: WireException) {
            null
        }

    private companion object {
        fun describe(status: ReplyStatus, body: ByteArray): String {
            val reply = Payloads.Reply(0u, status, body)
            return try {
                when (status) {
                    ReplyStatus.OK -> "undra reply: ok"
                    ReplyStatus.ERROR -> "undra call failed with a typed error (${body.size} bytes)"
                    ReplyStatus.PANIC -> reply.readPanic().let { "the core panicked: ${it.message}" }
                    ReplyStatus.CANCELLED -> "the call was cancelled"
                    ReplyStatus.STREAM_OPENED -> "a stream was opened where a single reply was expected"
                    ReplyStatus.BAD_REQUEST -> "the core rejected the request: ${reply.readBadRequestReason()}"
                }
            } catch (e: WireException) {
                "undra reply with status $status and an unreadable ${body.size}-byte body"
            }
        }
    }
}

/**
 * The channel to the core failed, or the core is gone: this [UndraCore] was closed (or never loaded), a
 * remote core did not answer in time, or its connection was lost. Pending calls and streams fail with it.
 * A generated call reports it as [UndraCallError.Unavailable].
 *
 * @property reason what went wrong.
 */
public class UndraTransportException(public val reason: Reason, message: String, cause: Throwable? = null) :
    UndraException(message, cause) {

    /** Why a transport failed. */
    public enum class Reason {
        /** This [UndraCore] was closed (`close()`), or no core is loaded. */
        CLOSED,

        /** The remote core did not answer within [LoadOptions.remoteTimeout]. */
        TIMEOUT,

        /** The connection to a remote core ended or failed. */
        CONNECTION_LOST,

        /** The calling thread was interrupted while it waited for a remote core. */
        INTERRUPTED,
    }
}

/**
 * The core sent something the protocol does not allow: a reply that does not decode, a reply for another
 * call, a constructor that answered with the null handle, a single value where a stream was expected. After a
 * successful schema check this is a bug in Undra. A generated call reports it as [UndraCallError.Malformed].
 */
public class UndraProtocolException(message: String, cause: Throwable? = null) : UndraException(message, cause)

/**
 * The core rejected a snapshot passed to [UndraCore.restore]; the core is unchanged (SPEC section 5.9).
 *
 * @property code the non-zero code `undra_restore` returned.
 */
public class UndraRestoreException(public val code: Int) :
    UndraException("the Undra core rejected the snapshot (code $code); a rejected restore leaves the core unchanged")

/**
 * An operation is not available in the mode the core was loaded in, or the [LoadOptions] contradict
 * each other. For example, snapshots exist only for an in-process core, and `Mode.REMOTE` needs a URL.
 */
public class UndraModeException(message: String) : UndraException(message)

/**
 * The core was built from a different schema than the bindings in this app (SPEC section 11): the
 * `expectedSchemaHash` passed to [UndraCore.load] differs from the hash the core reports. Regenerate
 * the bindings (`undra bindgen`) and rebuild the core, or use matching builds of both.
 *
 * @property expected the hash the bindings were generated from (`UndraIds.SCHEMA_HASH`).
 * @property got the hash the core reports.
 */
public class UndraSchemaMismatchException(public val expected: ULong, public val got: ULong) :
    UndraException(
        "schema mismatch: this app expects schema 0x${expected.toString(16)} but the core reports 0x${got.toString(16)}; " +
            "regenerate the bindings and rebuild the core so both come from the same schema",
    )

/**
 * The dev server no longer holds the objects of this core (ADR-051): it was restarted (`undra dev` rebuilt the
 * core) or the session's grace period passed while the client was away. The handles of every store and object of
 * this core are dead; load a new core and create them again. The core reports it as [ConnectionState.Closed]
 * with [ClosedReason.SESSION_LOST].
 */
public class UndraSessionLostException(message: String = DEFAULT) : UndraException(message) {
    private companion object {
        const val DEFAULT: String = "the dev server no longer has this core's objects (it was restarted, or the session expired); load a new core"
    }
}

/**
 * Thrown by a generated port adapter when the port implementation fails with the port's typed error.
 * The runtime answers the core's port call with `PortReply` status 1 and [body] as the error value.
 *
 * @property body the encoded typed error.
 */
public class UndraPortException(public val body: ByteArray) : UndraException("typed port failure")
