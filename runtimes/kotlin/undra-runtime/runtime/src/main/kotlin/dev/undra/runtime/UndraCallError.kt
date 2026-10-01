package dev.undra.runtime

import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.decodeAll
import kotlin.coroutines.cancellation.CancellationException

/**
 * Why a call into the core did not produce its result, when the reason is neither the method's own error
 * type nor the cancellation of the calling coroutine (ADR-032, amendment A).
 *
 * A generated call fails with exactly one of three things: its own typed error (`TodoError`, reply status 1),
 * [CancellationException] (the calling coroutine was cancelled), or an [UndraCallError]:
 *
 * ```kotlin
 * try {
 *     store.add(draft)
 * } catch (e: TodoError) {
 *     problem = "Give it a title first."
 * } catch (e: UndraCallError) {
 *     problem = e.message                     // Panicked, Refused, Unavailable, ...
 * }
 * ```
 *
 * A generated *command* (a synchronous method that returns nothing and has no error type) never throws;
 * it reports the failure to [LoadOptions.onError] instead.
 */
public sealed class UndraCallError(message: String, cause: Throwable? = null) : UndraException(message, cause) {

    /**
     * The core cancelled the call: a restore replaced or invalidated the object it ran on, or the core shut
     * down while it ran (reply status 3). A cancelled coroutine throws [CancellationException] instead.
     */
    public class CancelledByCore :
        UndraCallError("the Undra core cancelled the call (a restore replaced its object, or the core shut down)")

    /**
     * The core panicked while running the call (reply status 2). The core caught the panic and keeps working.
     *
     * @property panicMessage the panic message.
     * @property backtrace the core's backtrace; empty for a stream panic.
     */
    public class Panicked(public val panicMessage: String, public val backtrace: String) :
        UndraCallError("the Undra core panicked: $panicMessage")

    /**
     * The core refused the call without running it (reply status 5): the object was closed or replaced by a
     * restore, the call was made from inside one of the core's callbacks (`E_REENTRANT`), or the request could
     * not be decoded.
     *
     * @property reason the core's text.
     */
    public class Refused(public val reason: String) : UndraCallError("the Undra core refused the call: $reason")

    /**
     * The core cannot be reached: this [UndraCore] was closed or never loaded, or the remote connection closed
     * or timed out. Over `undra dev` it is also what every call fails with while the connection is down and the
     * core is [ConnectionState.Reconnecting] (and what was in flight when it dropped fails with), with a
     * [transport] reason of [UndraTransportException.Reason.CONNECTION_LOST]: [UndraCore.connectionState] says what the
     * runtime is doing about it, and a command's failure of that kind is not handed to [LoadOptions.onError].
     *
     * @property transport what happened; also the [cause].
     */
    public class Unavailable(public val transport: UndraTransportException) :
        UndraCallError("the Undra core is unavailable: ${transport.message}", transport)

    /**
     * The core answered with something the bindings cannot read. After a successful schema check this is a bug
     * in Undra; please report it with the text.
     *
     * @property detail what could not be read.
     */
    public class Malformed internal constructor(public val detail: String, cause: Throwable?, message: String) :
        UndraCallError(message, cause) {
        /** A reply or message the bindings cannot read. */
        public constructor(detail: String, cause: Throwable? = null) :
            this(detail, cause, "the Undra core sent a reply the bindings cannot read: $detail")
    }

    /**
     * The mapping of a raw failure onto the closed set, once, for the whole runtime (ADR-032, amendment A,
     * decision 9). Generated code wraps every call in one `try`/`catch` and throws what a function here returns:
     *
     * | Raw failure | Result |
     * |---|---|
     * | [CancellationException], any other [UndraException] subclass (a typed error), or any throwable that is not Undra's | itself |
     * | an [UndraCallError] | itself |
     * | [UndraReplyException], status `ERROR` and a `domain` | the decoded typed error (a body that does not decode: [Malformed]) |
     * | status `ERROR` without one, `OK`, `STREAM_OPENED` | [Malformed] |
     * | status `PANIC` | [Panicked] |
     * | status `CANCELLED` | [CancelledByCore] |
     * | status `BAD_REQUEST`, [UndraModeException], [UndraRestoreException] | [Refused] |
     * | [UndraTransportException] (what every transport throws for a closed core or a lost, reconnecting or timed-out connection, ADR-051), [UndraSchemaMismatchException] and [UndraSessionLostException] (what ended a remote core's connection for good), a plain [UndraException] (a foreign [Transport]'s failure) | [Unavailable] |
     * | [UndraProtocolException], [WireException], [UndraPortException] | [Malformed] |
     */
    public companion object {
        /** The error a generated method without an error type throws for [error], a failure of [UndraCore.callSync], [UndraCore.call] or [UndraCore.construct], or of decoding their result. */
        public fun mapped(error: Throwable): Throwable = classify(error)

        /**
         * The same for a method whose Rust signature returns `Result<_, E>`: a typed reply is decoded with
         * [domain] (the companion of the generated error class) and returned as `E`.
         */
        public fun <E : Throwable> mapped(error: Throwable, domain: UndraCodec<E>): Throwable {
            if (error is UndraReplyException && error.status == ReplyStatus.ERROR) {
                return decodeTyped(error.body, domain) ?: Malformed("a typed error that does not decode (${error.body.size} bytes)")
            }
            return classify(error)
        }

        /**
         * The error a generated stream method ends with, for a failure of [UndraCore.stream].
         *
         * A stream fails in the vocabulary of a failed reply (ADR-036): a stream the core ended itself (a restore
         * or a shutdown) is [CancelledByCore], one that panicked is [Panicked] with the message and backtrace, one
         * the core refused is [Refused], exactly as for a call, and one the runtime could not read is [Malformed].
         * Only a stream with an error type can end with a typed error, so one here (reply status `ERROR`) is
         * [Malformed].
         */
        public fun mappedStream(error: Throwable): Throwable {
            if (error is UndraReplyException && error.status == ReplyStatus.ERROR) {
                return Malformed("a stream without an error type ended with a typed error item (${error.body.size} bytes)")
            }
            return classify(error)
        }

        /**
         * The same for a stream whose Rust signature carries an error type `E` (a `Result<impl Stream, E>`, or a
         * stream of `Result<T, E>`): its typed error item is decoded with [domain] and returned as `E`, and one that
         * does not decode as `E` is [Malformed]. Every other failure maps as for a stream without an error type.
         */
        public fun <E : Throwable> mappedStream(error: Throwable, domain: UndraCodec<E>): Throwable = mapped(error, domain)

        /** [error] as the [UndraCallError] a report carries: what [mapped] returns, or [Malformed] for a failure that is not Undra's. */
        internal fun asCallError(error: Throwable): UndraCallError =
            classify(error) as? UndraCallError ?: Malformed(error.toString(), error, "an unexpected failure: $error")

        private fun <E : Throwable> decodeTyped(body: ByteArray, domain: UndraCodec<E>): E? =
            try {
                domain.decodeAll(body)
            } catch (e: WireException) {
                null
            }

        /** The single place every failure goes through. */
        private fun classify(error: Throwable): Throwable =
            when (error) {
                is CancellationException -> error
                is UndraCallError -> error
                is UndraReplyException -> fromStatus(error)
                is UndraTransportException -> Unavailable(error)
                is UndraSchemaMismatchException, is UndraSessionLostException ->
                    // A remote core that came back with another schema (`undra dev` rebuilt it) or without this core's
                    // objects: the connection is closed for good, and `ConnectedCore` fails every call with the
                    // transport exception that wraps it. This arm is for one that reaches a caller unwrapped.
                    Unavailable(UndraTransportException(UndraTransportException.Reason.CONNECTION_LOST, error.message.orEmpty(), error))
                is UndraProtocolException -> Malformed(error.message.orEmpty(), error)
                is WireException -> Malformed("the reply does not decode: ${error.message}", error)
                is UndraModeException -> Refused(error.message.orEmpty())
                is UndraRestoreException -> Refused(error.message.orEmpty())
                is UndraPortException -> Malformed("a port implementation's typed failure reached a call (${error.body.size} bytes)", error)
                else ->
                    // A foreign `Transport` (a test double, another host's) that throws a plain UndraException for a lost
                    // connection: the core cannot be reached. The runtime's own transports throw UndraTransportException.
                    // Any other throwable (an UndraException subclass such as a typed error, or not Undra's at all) is not
                    // the runtime's to classify.
                    if (error.javaClass == UndraException::class.java) {
                        Unavailable(UndraTransportException(UndraTransportException.Reason.CONNECTION_LOST, error.message.orEmpty(), error))
                    } else {
                        error
                    }
            }

        /** The mapping of a reply status that is not the method's own error. */
        private fun fromStatus(error: UndraReplyException): UndraCallError =
            when (error.status) {
                ReplyStatus.CANCELLED -> CancelledByCore()
                ReplyStatus.PANIC -> {
                    val info = error.panicInfo
                    if (info == null) Panicked("<undecodable panic report>", "") else Panicked(info.message, info.backtrace)
                }
                ReplyStatus.BAD_REQUEST -> Refused(error.badRequestReason ?: "<undecodable reason>")
                ReplyStatus.ERROR -> Malformed("the core answered with a typed error, but this method has none (${error.body.size} bytes)")
                ReplyStatus.STREAM_OPENED -> Malformed("the core opened a stream where a single reply was expected")
                ReplyStatus.OK -> Malformed("the core answered ok as a failure")
            }
    }
}

/**
 * A failure no caller could see: a generated command (a synchronous method that returns nothing and has no
 * error type), a store's change that could not be applied, a malformed change-set or a failed port. Delivered to
 * [LoadOptions.onError] by [UndraCore.report] (ADR-032, amendment A).
 *
 * @property operation what failed, as Kotlin spells it: `"TodoStore.toggle"`, `"configureRemote"`,
 *   `"TodoStore.apply(signal: 2)"`.
 * @property error why.
 */
public class UndraUnhandledError(public val operation: String, public val error: UndraCallError) :
    UndraException("$operation failed: ${error.message}", error)
