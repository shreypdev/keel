package dev.keel.runtime.wire

private const val DEFAULT_CAPACITY = 64

/** Largest array the JVM can reliably allocate; growing past it fails cleanly instead of overflowing. */
private const val MAX_CAPACITY = Int.MAX_VALUE - 8

/**
 * Serializer for the Keel wire format (SPEC §3).
 *
 * Everything is little-endian, unaligned and unpadded. The writer owns a growable byte array
 * (doubling growth, so appends are amortized O(1)) and never allocates per call except when it has
 * to grow. It is **not** thread-safe; use one writer per message.
 *
 * ```kotlin
 * val w = KeelWriter()
 * w.writeU32(7u)
 * w.writeStr("héllo")
 * val bytes = w.toByteArray()
 * ```
 *
 * Signed and unsigned integers are separate methods because Kotlin has separate types; both write
 * the same two's-complement bytes.
 *
 * @param initialCapacity starting size of the backing array in bytes; pass a good estimate to avoid
 *   regrowth. Must not be negative.
 */
public class KeelWriter(initialCapacity: Int = DEFAULT_CAPACITY) {
    private var buf: ByteArray = ByteArray(requireCapacity(initialCapacity))
    private var pos: Int = 0

    /** Number of bytes written so far. */
    public val size: Int get() = pos

    /** Discards everything written so far but keeps the backing array, so the writer can be reused. */
    public fun reset() {
        pos = 0
    }

    /** Writes one byte. */
    public fun writeU8(v: UByte) {
        ensure(1)
        buf[pos++] = v.toByte()
    }

    /** Writes one byte (two's complement). */
    public fun writeI8(v: Byte) {
        ensure(1)
        buf[pos++] = v
    }

    /** Writes 2 bytes, little-endian. */
    public fun writeU16(v: UShort) {
        writeI16(v.toShort())
    }

    /** Writes 2 bytes, little-endian. */
    public fun writeI16(v: Short) {
        ensure(2)
        val b = buf
        val p = pos
        val x = v.toInt()
        b[p] = x.toByte()
        b[p + 1] = (x shr 8).toByte()
        pos = p + 2
    }

    /** Writes 4 bytes, little-endian. */
    public fun writeU32(v: UInt) {
        writeI32(v.toInt())
    }

    /** Writes 4 bytes, little-endian. */
    public fun writeI32(v: Int) {
        ensure(4)
        val b = buf
        val p = pos
        b[p] = v.toByte()
        b[p + 1] = (v shr 8).toByte()
        b[p + 2] = (v shr 16).toByte()
        b[p + 3] = (v shr 24).toByte()
        pos = p + 4
    }

    /** Writes 8 bytes, little-endian. */
    public fun writeU64(v: ULong) {
        writeI64(v.toLong())
    }

    /** Writes 8 bytes, little-endian. */
    public fun writeI64(v: Long) {
        ensure(8)
        val b = buf
        val p = pos
        b[p] = v.toByte()
        b[p + 1] = (v shr 8).toByte()
        b[p + 2] = (v shr 16).toByte()
        b[p + 3] = (v shr 24).toByte()
        b[p + 4] = (v shr 32).toByte()
        b[p + 5] = (v shr 40).toByte()
        b[p + 6] = (v shr 48).toByte()
        b[p + 7] = (v shr 56).toByte()
        pos = p + 8
    }

    /** Writes the IEEE 754 bits of [v], 4 bytes little-endian. NaN payload bits are preserved. */
    public fun writeF32(v: Float) {
        writeI32(v.toRawBits())
    }

    /** Writes the IEEE 754 bits of [v], 8 bytes little-endian. NaN payload bits are preserved. */
    public fun writeF64(v: Double) {
        writeI64(v.toRawBits())
    }

    /** Writes one byte: 1 for `true`, 0 for `false`. */
    public fun writeBool(v: Boolean) {
        ensure(1)
        buf[pos++] = if (v) 1 else 0
    }

    /**
     * Writes a `u32` byte length followed by the UTF-8 bytes of [s], with no intermediate
     * allocation.
     *
     * @throws IllegalArgumentException if [s] contains an unpaired surrogate (it has no UTF-8 form).
     */
    public fun writeStr(s: String) {
        val n = utf8Length(s)
        writeLen(n)
        ensure(n)
        val b = buf
        var p = pos
        if (n == s.length) {
            // Pure ASCII: one byte per char.
            for (i in 0 until n) b[p + i] = s[i].code.toByte()
        } else {
            utf8ForEachByte(s) { b[p++] = it.toByte() }
        }
        pos += n
    }

    /**
     * Writes a `u32` length followed by `bytes[offset until offset + length]`.
     *
     * @throws IllegalArgumentException if the range is not inside [bytes].
     */
    public fun writeBytes(bytes: ByteArray, offset: Int = 0, length: Int = bytes.size - offset) {
        checkRange(bytes.size, offset, length)
        writeLen(length)
        copyIn(bytes, offset, length)
    }

    /**
     * Writes `bytes[offset until offset + length]` verbatim with **no** length prefix. Used for
     * payload bodies that run to the end of the message (SPEC §3.3 `args`, §3.4 `body`).
     *
     * @throws IllegalArgumentException if the range is not inside [bytes].
     */
    public fun writeRaw(bytes: ByteArray, offset: Int = 0, length: Int = bytes.size - offset) {
        checkRange(bytes.size, offset, length)
        copyIn(bytes, offset, length)
    }

    /**
     * Writes a `u32` length or count prefix.
     *
     * @throws IllegalArgumentException if [n] is negative.
     */
    public fun writeLen(n: Int) {
        require(n >= 0) { "length must not be negative, was $n" }
        writeI32(n)
    }

    /** Returns a copy of everything written so far. The writer stays usable. */
    public fun toByteArray(): ByteArray = buf.copyOf(pos)

    /** Hands out the backing array without copying when it is exactly full. The writer must not be used afterwards. */
    internal fun takeArray(): ByteArray = if (pos == buf.size) buf else buf.copyOf(pos)

    /** The backing array; only the first [size] bytes are meaningful. Valid until the next write. */
    internal fun backingArray(): ByteArray = buf

    /** Writes 8 bytes, big-endian (used by UUIDs, which are big-endian on the wire). */
    internal fun writeI64BigEndian(v: Long) {
        ensure(8)
        val b = buf
        val p = pos
        b[p] = (v shr 56).toByte()
        b[p + 1] = (v shr 48).toByte()
        b[p + 2] = (v shr 40).toByte()
        b[p + 3] = (v shr 32).toByte()
        b[p + 4] = (v shr 24).toByte()
        b[p + 5] = (v shr 16).toByte()
        b[p + 6] = (v shr 8).toByte()
        b[p + 7] = v.toByte()
        pos = p + 8
    }

    private fun copyIn(bytes: ByteArray, offset: Int, length: Int) {
        ensure(length)
        System.arraycopy(bytes, offset, buf, pos, length)
        pos += length
    }

    private fun ensure(extra: Int) {
        if (extra > buf.size - pos) grow(extra)
    }

    private fun grow(extra: Int) {
        val needed = pos.toLong() + extra
        check(needed <= MAX_CAPACITY) { "KeelWriter cannot hold more than $MAX_CAPACITY bytes (needs $needed)" }
        var capacity = maxOf(buf.size.toLong() * 2, needed, 16L)
        if (capacity > MAX_CAPACITY) capacity = MAX_CAPACITY.toLong()
        buf = buf.copyOf(capacity.toInt())
    }
}

private fun requireCapacity(initialCapacity: Int): Int {
    require(initialCapacity >= 0) { "initialCapacity must not be negative, was $initialCapacity" }
    return initialCapacity
}

private fun checkRange(size: Int, offset: Int, length: Int) {
    require(offset >= 0 && length >= 0 && offset <= size - length) {
        "range offset=$offset length=$length is outside an array of size $size"
    }
}
