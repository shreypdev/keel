package dev.keel.runtime.wire

/**
 * The decoded form of a wire `Result<T, E>` (SPEC §3.1): tag byte 0 is [Ok], 1 is [Err].
 *
 * It is a plain value, not an exception: `kotlin.Result` cannot carry a typed error `E`, and the
 * generated code decides when an [Err] becomes a thrown `KeelException` subclass.
 */
public sealed class KeelResult<out T, out E> {

    /** The success case, carrying the decoded `T`. */
    public data class Ok<out T>(val value: T) : KeelResult<T, Nothing>()

    /** The failure case, carrying the decoded `E`. */
    public data class Err<out E>(val error: E) : KeelResult<Nothing, E>()

    /** `true` for [Ok]. */
    public val isOk: Boolean get() = this is Ok

    /** `true` for [Err]. */
    public val isErr: Boolean get() = this is Err

    /** The success value, or `null` for [Err]. */
    public fun getOrNull(): T? = if (this is Ok) value else null

    /** The error value, or `null` for [Ok]. */
    public fun errorOrNull(): E? = if (this is Err) error else null

    /** Collapses this result into one value by applying [onOk] or [onErr]. */
    public inline fun <R> fold(onOk: (T) -> R, onErr: (E) -> R): R = when (this) {
        is Ok -> onOk(value)
        is Err -> onErr(error)
    }
}
