package dev.keel.runtime.wire

/**
 * Encodes and decodes values of type [T] in the Keel wire format.
 *
 * Generated records implement it on their companion (`companion object : KeelCodec<Todo>`) and the
 * generated code composes them with the combinators on [Codecs]. Codecs are stateless and
 * thread-safe; the [KeelWriter] / [KeelReader] passed in carry all the state.
 *
 * Implementations must uphold the wire layer's contract (see [WireException]): `decode` reports
 * malformed input only by throwing a [WireException], and must go through [KeelReader.readLen] before
 * allocating anything sized by the input.
 */
public interface KeelCodec<T> {

    /** Appends the encoding of [v] to [w]. */
    public fun encode(w: KeelWriter, v: T)

    /**
     * Reads one value from [r], advancing it past the value. Does not check that the reader is
     * exhausted; call [KeelReader.finish] (or use [decodeAll]) when the value is the whole message.
     */
    public fun decode(r: KeelReader): T
}

/** Encodes [v] with this codec into a fresh byte array. */
public fun <T> KeelCodec<T>.encodeToByteArray(v: T): ByteArray {
    val w = KeelWriter()
    encode(w, v)
    return w.toByteArray()
}

/**
 * Decodes a single value that must span all of [bytes].
 *
 * @throws WireException.TrailingBytes if bytes are left after the value.
 */
public fun <T> KeelCodec<T>.decodeAll(bytes: ByteArray): T {
    val r = KeelReader(bytes)
    val v = decode(r)
    r.finish()
    return v
}
