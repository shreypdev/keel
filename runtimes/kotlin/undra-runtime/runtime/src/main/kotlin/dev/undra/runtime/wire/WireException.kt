package dev.undra.runtime.wire

/**
 * Every way decoding an Undra wire message can fail (SPEC §3.9 `WireError`).
 *
 * The contract for the whole wire layer:
 *  - **Decoding** malformed input throws a subclass of this exception and nothing else. Truncated,
 *    oversized, garbage or hostile bytes never surface as `IndexOutOfBoundsException`,
 *    `NegativeArraySizeException`, `OutOfMemoryError` from an attacker-chosen length, and so on.
 *  - **Encoding** throws a subclass of this exception only for values the wire cannot represent and
 *    that a peer would reject with the same error ([NegativeDuration], [DuplicateKey]). Programming
 *    errors with no wire analogue (a string with an unpaired surrogate, a negative length) throw
 *    [IllegalArgumentException].
 *
 * All offsets (`at`) are byte offsets relative to the start of the [UndraReader] window that was being
 * read, so they are stable regardless of where the window sits inside a larger array or buffer.
 */
public sealed class WireException(message: String) : RuntimeException(message) {

    /**
     * A read needed more bytes than the input has left.
     *
     * @property needed number of bytes the failing read requested.
     * @property at offset at which the read was attempted.
     */
    public class UnexpectedEof(public val needed: Int, public val at: Int) :
        WireException("unexpected end of input at offset $at: read needs $needed byte(s)")

    /**
     * A string field is not well-formed UTF-8 (overlong forms, surrogates, code points above
     * U+10FFFF and truncated sequences are all rejected).
     *
     * @property at offset of the first offending byte.
     */
    public class InvalidUtf8(public val at: Int) :
        WireException("invalid UTF-8 at offset $at")

    /**
     * A tag byte (enum discriminant, `Option`/`Result` tag, bool, message kind ...) has no meaning.
     *
     * @property tag the offending value.
     * @property at offset of the tag, or `-1` when it was validated outside a reader
     *   (for example by `Envelope.Kind.fromByte`).
     * @property type what the tag selects, for example `"bool"` or `"ReplyStatus"`.
     */
    public class InvalidTag(public val tag: UInt, public val at: Int, public val type: String) :
        WireException("invalid tag $tag for $type at offset $at")

    /**
     * A length or count prefix is larger than the bytes that remain, so the value cannot possibly
     * fit. Raised before anything is allocated.
     *
     * @property len the declared length (an unsigned 32-bit value).
     * @property at offset of the length prefix.
     */
    public class LengthTooLarge(public val len: UInt, public val at: Int) :
        WireException("length $len at offset $at exceeds the remaining input")

    /**
     * A message decoded successfully but bytes were left over.
     *
     * @property count number of unread bytes.
     */
    public class TrailingBytes(public val count: Int) :
        WireException("$count trailing byte(s) after a complete message")

    /**
     * The envelope does not start with the `UNDRA` magic.
     *
     * @property found the four bytes found instead, as lowercase hex.
     */
    public class BadMagic(public val found: String) :
        WireException("bad envelope magic: expected 4b45454c (\"UNDRA\"), found $found")

    /**
     * The envelope declares a wire version this runtime does not speak.
     *
     * @property version the declared version.
     */
    public class UnsupportedVersion(public val version: UShort) :
        WireException("unsupported envelope version $version (this runtime speaks ${Envelope.VERSION})")

    /**
     * The peer was built from a different schema (SPEC §1.1 `schema_hash`).
     *
     * @property expected schema hash of this side.
     * @property got schema hash carried by the message.
     */
    public class SchemaMismatch(public val expected: ULong, public val got: ULong) :
        WireException("schema mismatch: expected ${expected.toString(16)}, got ${got.toString(16)}")

    /**
     * A `Map` contains the same key twice: on decode, two entries with equal keys; on encode, two
     * keys whose encodings are byte-identical.
     *
     * @property at decode: offset of the repeated key. Encode: output offset where the map's
     *   entries begin.
     */
    public class DuplicateKey(public val at: Int) :
        WireException("duplicate map key at offset $at")

    /**
     * A `Duration` is negative. The wire carries `i64` nanoseconds but the core's `Duration` is
     * unsigned, so a negative value can never be delivered.
     *
     * @property nanos the offending value in nanoseconds.
     * @property at decode: offset of the duration. Encode: output offset it would have been written at.
     */
    public class NegativeDuration(public val nanos: Long, public val at: Int) :
        WireException("negative duration ($nanos ns) at offset $at")

    /**
     * A keyed patch does not apply to the list it was applied to (an index is out of range). This is
     * not a malformed encoding but a divergence between the core's and the host's copy of the list.
     *
     * @property opIndex position of the failing op within the patch.
     * @property op name of the failing op (`Insert`, `Remove`, `Update`, `Move`).
     * @property index the offending index.
     * @property size size of the list when the op was applied.
     */
    public class PatchOutOfBounds(
        public val opIndex: Int,
        public val op: String,
        public val index: UInt,
        public val size: Int,
    ) : WireException("patch op #$opIndex ($op): index $index is out of bounds for a list of size $size")
}
