package dev.undra.runtime.wire

/**
 * FNV-1a hashes over UTF-8 bytes, the basis of every stable Undra identifier (SPEC §1.1): type ids,
 * method ids, port ids, query ids (32-bit) and the schema hash (64-bit).
 *
 * ```kotlin
 * Fnv.fnv1a32("Calculator.add")  // 2353348832u  (a method_id)
 * Fnv.fnv1a64("undra")            // 6367360722358687308uL
 * ```
 */
public object Fnv {
    private const val OFFSET_32: UInt = 0x811c9dc5u
    private const val PRIME_32: UInt = 0x01000193u
    private const val OFFSET_64: ULong = 0xcbf29ce484222325uL
    private const val PRIME_64: ULong = 0x100000001b3uL

    /**
     * 32-bit FNV-1a of the UTF-8 encoding of [s]; does not allocate.
     *
     * @throws IllegalArgumentException if [s] contains an unpaired surrogate.
     */
    public fun fnv1a32(s: String): UInt {
        var h = OFFSET_32
        utf8ForEachByte(s) { h = (h xor it.toUInt()) * PRIME_32 }
        return h
    }

    /**
     * 64-bit FNV-1a of the UTF-8 encoding of [s]; does not allocate.
     *
     * @throws IllegalArgumentException if [s] contains an unpaired surrogate.
     */
    public fun fnv1a64(s: String): ULong {
        var h = OFFSET_64
        utf8ForEachByte(s) { h = (h xor it.toULong()) * PRIME_64 }
        return h
    }

    /**
     * 32-bit FNV-1a of `bytes[offset until offset + length]`.
     *
     * @throws IllegalArgumentException if the range is not inside [bytes].
     */
    public fun fnv1a32(bytes: ByteArray, offset: Int = 0, length: Int = bytes.size - offset): UInt {
        checkRange(bytes.size, offset, length)
        var h = OFFSET_32
        for (i in offset until offset + length) h = (h xor (bytes[i].toInt() and 0xFF).toUInt()) * PRIME_32
        return h
    }

    /**
     * 64-bit FNV-1a of `bytes[offset until offset + length]`.
     *
     * @throws IllegalArgumentException if the range is not inside [bytes].
     */
    public fun fnv1a64(bytes: ByteArray, offset: Int = 0, length: Int = bytes.size - offset): ULong {
        checkRange(bytes.size, offset, length)
        var h = OFFSET_64
        for (i in offset until offset + length) h = (h xor (bytes[i].toInt() and 0xFF).toULong()) * PRIME_64
        return h
    }

    private fun checkRange(size: Int, offset: Int, length: Int) {
        require(offset >= 0 && length >= 0 && offset <= size - length) {
            "range offset=$offset length=$length is outside an array of size $size"
        }
    }
}
