package dev.undra.runtime.wire

/**
 * Encodes and decodes values of type [T] in the Undra wire format.
 *
 * Generated records implement it on their companion (`companion object : UndraCodec<Todo>`) and the
 * generated code composes them with the combinators on [Codecs]. Codecs are stateless and
 * thread-safe; the [UndraWriter] / [UndraReader] passed in carry all the state.
 *
 * Implementations must uphold the wire layer's contract (see [WireException]): `decode` reports
 * malformed input only by throwing a [WireException], and must go through [UndraReader.readLen] before
 * allocating anything sized by the input.
 */
public interface UndraCodec<T> {

    /** Appends the encoding of [v] to [w]. */
    public fun encode(w: UndraWriter, v: T)

    /**
     * Reads one value from [r], advancing it past the value. Does not check that the reader is
     * exhausted; call [UndraReader.finish] (or use [decodeAll]) when the value is the whole message.
     */
    public fun decode(r: UndraReader): T
}

/** Encodes [v] with this codec into a fresh byte array. */
public fun <T> UndraCodec<T>.encodeToByteArray(v: T): ByteArray {
    val w = UndraWriter()
    encode(w, v)
    return w.toByteArray()
}

/**
 * Decodes a single value that must span all of [bytes].
 *
 * @throws WireException.TrailingBytes if bytes are left after the value.
 */
public fun <T> UndraCodec<T>.decodeAll(bytes: ByteArray): T {
    val r = UndraReader(bytes)
    val v = decode(r)
    r.finish()
    return v
}
