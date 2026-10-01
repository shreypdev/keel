package dev.undra.runtime.wire

import java.nio.Buffer
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Deserializer for the Undra wire format (SPEC §3).
 *
 * A reader is a cursor over a *window* of bytes. Every method advances the cursor, and every
 * malformed input surfaces as a [WireException]; a reader never throws anything else on bad data and
 * never allocates based on an unchecked length (see [readLen]). Offsets in errors are relative to the
 * start of the window.
 *
 * Two backings are supported:
 *  - a [ByteArray] window (`UndraReader(bytes, offset, length)`), and
 *  - a [ByteBuffer], intended for the direct buffers the JNI shim hands to callbacks (SPEC §6.1). The
 *    buffer is read in place, without copying it; its position, limit and byte order are left
 *    untouched. **Such a reader is only valid while the buffer is** (for a JNI callback: until the
 *    callback returns), so decode everything you need before returning.
 *
 * Methods that return `ByteArray` ([readBytes], [readRaw], [readRemaining]) **copy**; strings are
 * necessarily copied into a `String`. Everything else is zero-copy. A reader is not thread-safe.
 *
 * After a [WireException] the cursor is somewhere inside the window but otherwise unspecified: discard
 * the reader, do not try to resume decoding from it.
 */
public class UndraReader private constructor(
    private val array: ByteArray?,
    private val buffer: ByteBuffer?,
    start: Int,
    end: Int,
) {
    private var start: Int = start
    private var end: Int = end
    private var pos: Int = start

    /**
     * Reads `bytes[offset until offset + length]`.
     *
     * @throws IllegalArgumentException if the window is not inside [bytes].
     */
    public constructor(bytes: ByteArray, offset: Int = 0, length: Int = bytes.size - offset) :
        this(bytes, null, offset, windowEnd(bytes.size, offset, length))

    /**
     * Reads the bytes between [buffer]'s position and limit, in place. Works for any `ByteBuffer`
     * but is meant for direct buffers over core memory. The buffer's own position, limit and byte
     * order are not modified (the reader works on a private little-endian slice).
     */
    public constructor(buffer: ByteBuffer) :
        this(null, buffer.slice().order(ByteOrder.LITTLE_ENDIAN), 0, buffer.remaining())

    /** Bytes left to read. */
    public val remaining: Int get() = end - pos

    /** Offset of the cursor from the start of the window. */
    public val position: Int get() = pos - start

    /** Reads one byte as unsigned. */
    public fun readU8(): UByte {
        need(1)
        return byteAt(pos++).toUByte()
    }

    /** Reads one byte as signed. */
    public fun readI8(): Byte {
        need(1)
        return byteAt(pos++)
    }

    /** Reads 2 bytes, little-endian, as unsigned. */
    public fun readU16(): UShort = readI16().toUShort()

    /** Reads 2 bytes, little-endian, as signed. */
    public fun readI16(): Short {
        need(2)
        val v = shortAt(pos)
        pos += 2
        return v
    }

    /** Reads 4 bytes, little-endian, as unsigned. */
    public fun readU32(): UInt = readI32().toUInt()

    /** Reads 4 bytes, little-endian, as signed. */
    public fun readI32(): Int {
        need(4)
        val v = intAt(pos)
        pos += 4
        return v
    }

    /** Reads 8 bytes, little-endian, as unsigned. */
    public fun readU64(): ULong = readI64().toULong()

    /** Reads 8 bytes, little-endian, as signed. */
    public fun readI64(): Long {
        need(8)
        val v = longAt(pos)
        pos += 8
        return v
    }

    /** Reads an IEEE 754 single, preserving NaN payload bits. */
    public fun readF32(): Float = Float.fromBits(readI32())

    /** Reads an IEEE 754 double, preserving NaN payload bits. */
    public fun readF64(): Double = Double.fromBits(readI64())

    /**
     * Reads a bool. Only 0 and 1 are valid.
     *
     * @throws WireException.InvalidTag for any other byte.
     */
    public fun readBool(): Boolean {
        val at = position
        return when (val b = readU8().toInt()) {
            0 -> false
            1 -> true
            else -> throw WireException.InvalidTag(b.toUInt(), at, "bool")
        }
    }

    /**
     * Reads a `u32` length or count and checks that it is plausible: the count times [minItemBytes]
     * must not exceed the bytes that remain. This is what stops a 4-byte message from making the
     * decoder allocate gigabytes, so every collection decoder goes through it.
     *
     * The default of 1 is exact for strings and byte arrays and a safe lower bound for anything that
     * encodes to at least one byte per item. Consequently a `Vec` of a zero-width type (`Unit`, an
     * empty record) cannot be decoded when its count exceeds the remaining bytes.
     *
     * @param minItemBytes smallest possible encoded size of one item; must be at least 1.
     * @throws WireException.LengthTooLarge if the count cannot fit in the remaining bytes.
     */
    public fun readLen(minItemBytes: Int = 1): Int {
        require(minItemBytes >= 1) { "minItemBytes must be at least 1, was $minItemBytes" }
        val at = position
        need(4)
        val raw = intAt(pos).toLong() and 0xFFFFFFFFL
        pos += 4
        // raw < 2^32 and minItemBytes is small, so the product cannot overflow a Long.
        if (raw * minItemBytes > (end - pos).toLong()) throw WireException.LengthTooLarge(raw.toUInt(), at)
        return raw.toInt()
    }

    /**
     * Reads a `u32`-length-prefixed string and validates it as strict UTF-8.
     *
     * @throws WireException.LengthTooLarge if the declared length exceeds the remaining bytes.
     * @throws WireException.InvalidUtf8 if the bytes are not well-formed UTF-8.
     */
    public fun readStr(): String {
        val n = readLen()
        val s = if (array != null) {
            Utf8Decoder.decode(array, pos, n, position)
        } else {
            val view = buffer!!.duplicate()
            val v: Buffer = view // Buffer-typed so the call binds to Buffer.limit/position on every JVM/ART
            v.limit(pos + n)
            v.position(pos)
            Utf8Decoder.decode(view, n, position)
        }
        pos += n
        return s
    }

    /**
     * Reads a `u32`-length-prefixed byte array. The result is a **copy**, so it stays valid after the
     * underlying buffer is recycled.
     *
     * @throws WireException.LengthTooLarge if the declared length exceeds the remaining bytes.
     */
    public fun readBytes(): ByteArray = readRaw(readLen())

    /**
     * Reads exactly [n] bytes with no length prefix and returns a **copy**.
     *
     * @throws IllegalArgumentException if [n] is negative.
     * @throws WireException.UnexpectedEof if fewer than [n] bytes remain.
     */
    public fun readRaw(n: Int): ByteArray {
        require(n >= 0) { "byte count must not be negative, was $n" }
        need(n)
        val out = copyRange(pos, n)
        pos += n
        return out
    }

    /** Reads everything up to the end of the window as a **copy** (payload bodies with no length prefix). */
    public fun readRemaining(): ByteArray = readRaw(end - pos)

    /**
     * Skips [n] bytes.
     *
     * @throws IllegalArgumentException if [n] is negative.
     * @throws WireException.UnexpectedEof if fewer than [n] bytes remain.
     */
    public fun skip(n: Int) {
        require(n >= 0) { "byte count must not be negative, was $n" }
        need(n)
        pos += n
    }

    /**
     * Asserts that the whole window was consumed.
     *
     * @throws WireException.TrailingBytes if bytes are left over.
     */
    public fun finish() {
        if (pos != end) throw WireException.TrailingBytes(end - pos)
    }

    // ---- internal: used by inline iteration helpers that hand out reusable sub-readers -------------

    /** A new empty reader over the same backing store, to be pointed at sub-ranges with [aimAt]. */
    @PublishedApi
    internal fun newView(): UndraReader = UndraReader(array, buffer, pos, pos)

    /** Re-points this reader at the next [length] bytes of [parent] (which does not advance). */
    @PublishedApi
    internal fun aimAt(parent: UndraReader, length: Int) {
        start = parent.pos
        end = start + length
        pos = start
    }

    /** Reads 8 bytes, big-endian (used by UUIDs). */
    internal fun readI64BigEndian(): Long {
        need(8)
        val v = java.lang.Long.reverseBytes(longAt(pos))
        pos += 8
        return v
    }

    // ---- private ------------------------------------------------------------------------------------

    private fun need(n: Int) {
        if (n > end - pos) throw WireException.UnexpectedEof(n, pos - start)
    }

    private fun byteAt(i: Int): Byte = if (array != null) array[i] else buffer!!.get(i)

    private fun shortAt(i: Int): Short {
        if (array == null) return buffer!!.getShort(i)
        return ((array[i].toInt() and 0xFF) or (array[i + 1].toInt() shl 8)).toShort()
    }

    private fun intAt(i: Int): Int {
        if (array == null) return buffer!!.getInt(i)
        return (array[i].toInt() and 0xFF) or
            ((array[i + 1].toInt() and 0xFF) shl 8) or
            ((array[i + 2].toInt() and 0xFF) shl 16) or
            (array[i + 3].toInt() shl 24)
    }

    private fun longAt(i: Int): Long {
        if (array == null) return buffer!!.getLong(i)
        return (array[i].toLong() and 0xFF) or
            ((array[i + 1].toLong() and 0xFF) shl 8) or
            ((array[i + 2].toLong() and 0xFF) shl 16) or
            ((array[i + 3].toLong() and 0xFF) shl 24) or
            ((array[i + 4].toLong() and 0xFF) shl 32) or
            ((array[i + 5].toLong() and 0xFF) shl 40) or
            ((array[i + 6].toLong() and 0xFF) shl 48) or
            (array[i + 7].toLong() shl 56)
    }

    private fun copyRange(from: Int, n: Int): ByteArray {
        if (array != null) return array.copyOfRange(from, from + n)
        val out = ByteArray(n)
        val view = buffer!!.duplicate()
        val v: Buffer = view
        v.limit(from + n)
        v.position(from)
        view.get(out)
        return out
    }
}

private fun windowEnd(size: Int, offset: Int, length: Int): Int {
    require(offset >= 0 && length >= 0 && offset <= size - length) {
        "window offset=$offset length=$length is outside an array of size $size"
    }
    return offset + length
}
