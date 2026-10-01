package dev.undra.runtime

import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.PanicInfo
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.WireException

/**
 * Base class of every exception the Undra runtime and generated bindings throw on purpose.
 *
 * Generated error types (`#[undra::error]`) extend it, so `catch (e: UndraException)` catches both the
 * typed errors of your core and the runtime's own failures ([UndraReplyException],
 * [UndraModeException], [UndraSchemaMismatchException]).
 */
public open class UndraException(message: String, cause: Throwable? = null) : RuntimeException(message, cause)

/**
 * A call, constructor or stream finished with something other than a success: the core answered with
 * a non-`OK` [ReplyStatus] (SPEC section 3.4), or a stream ended with the error flag (section 3.7).
 *
 * Generated code turns the `ERROR` status into the method's typed error (`TodoError.fromReply`) and
 * lets every other status through unchanged.
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
 * Thrown by a generated port adapter when the port implementation fails with the port's typed error.
 * The runtime answers the core's port call with `PortReply` status 1 and [body] as the error value.
 *
 * @property body the encoded typed error.
 */
public class UndraPortException(public val body: ByteArray) : UndraException("typed port failure")
