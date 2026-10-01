package dev.undra.runtime.wire

import java.nio.ByteBuffer
import java.nio.CharBuffer
import java.nio.charset.CharsetDecoder
import java.nio.charset.CodingErrorAction

/**
 * Number of bytes [s] occupies in UTF-8.
 *
 * @throws IllegalArgumentException if [s] contains an unpaired surrogate: such a string has no UTF-8
 *   form, and silently substituting `?` (what `String.toByteArray` does) would corrupt data.
 */
internal fun utf8Length(s: String): Int {
    val n = s.length
    var bytes = n
    var i = 0
    while (i < n) {
        val c = s[i].code
        if (c >= 0x80) {
            if (c < 0x800) {
                bytes += 1
            } else if (c in 0xD800..0xDFFF) {
                if (c >= 0xDC00 || i + 1 >= n || s[i + 1].code !in 0xDC00..0xDFFF) throw unpairedSurrogate(i)
                bytes += 2 // the pair is 2 chars and 4 bytes
                i++
            } else {
                bytes += 2
            }
        }
        i++
    }
    return bytes
}

internal fun unpairedSurrogate(index: Int): IllegalArgumentException =
    IllegalArgumentException("string has an unpaired surrogate at index $index and cannot be encoded as UTF-8")

/**
 * Calls [emit] with every UTF-8 byte (0..255) of [s], in order, without allocating.
 *
 * @throws IllegalArgumentException on an unpaired surrogate (see [utf8Length]).
 */
internal inline fun utf8ForEachByte(s: String, emit: (Int) -> Unit) {
    val n = s.length
    var i = 0
    while (i < n) {
        val c = s[i].code
        if (c < 0x80) {
            emit(c)
        } else if (c < 0x800) {
            emit(0xC0 or (c shr 6))
            emit(0x80 or (c and 0x3F))
        } else if (c in 0xD800..0xDFFF) {
            if (c >= 0xDC00 || i + 1 >= n) throw unpairedSurrogate(i)
            val d = s[i + 1].code
            if (d !in 0xDC00..0xDFFF) throw unpairedSurrogate(i)
            val cp = 0x10000 + ((c - 0xD800) shl 10) + (d - 0xDC00)
            emit(0xF0 or (cp shr 18))
            emit(0x80 or ((cp shr 12) and 0x3F))
            emit(0x80 or ((cp shr 6) and 0x3F))
            emit(0x80 or (cp and 0x3F))
            i++
        } else {
            emit(0xE0 or (c shr 12))
            emit(0x80 or ((c shr 6) and 0x3F))
            emit(0x80 or (c and 0x3F))
        }
        i++
    }
}

/** Strict UTF-8 decoding shared by [UndraReader]. */
internal object Utf8Decoder {
    // CharsetDecoder is stateful and not thread-safe; one per thread avoids allocating one per string.
    private val decoders = object : ThreadLocal<CharsetDecoder>() {
        override fun initialValue(): CharsetDecoder = Charsets.UTF_8.newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT)
    }

    /** Decodes `bytes[offset until offset + length]`; [at] is the window offset of `bytes[offset]`. */
    fun decode(bytes: ByteArray, offset: Int, length: Int, at: Int): String {
        if (length == 0) return ""
        var ascii = true
        var i = offset
        val stop = offset + length
        while (i < stop) {
            if (bytes[i] < 0) {
                ascii = false
                break
            }
            i++
        }
        // Pure ASCII is by far the common case (identifiers, keys, URLs): skip the decoder.
        if (ascii) return String(bytes, offset, length, Charsets.ISO_8859_1)
        return decode(ByteBuffer.wrap(bytes, offset, length), length, at)
    }

    /** Decodes the [length] bytes remaining in [source]; [at] is the window offset of its first byte. */
    fun decode(source: ByteBuffer, length: Int, at: Int): String {
        if (length == 0) return ""
        val decoder = decoders.get()
        decoder.reset()
        val startPosition = source.position()
        // Each UTF-8 byte yields at most one UTF-16 char, so `length` chars can never overflow.
        val out = CharBuffer.allocate(length)
        val result = decoder.decode(source, out, true)
        if (result.isError) throw WireException.InvalidUtf8(at + (source.position() - startPosition))
        decoder.flush(out)
        return String(out.array(), 0, out.position())
    }
}
