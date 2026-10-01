package dev.undra.runtime.wire

import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertBytes
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.assertWire
import dev.undra.runtime.testing.bytesOf
import dev.undra.runtime.testing.cp
import dev.undra.runtime.testing.directBuffer
import org.junit.jupiter.api.Test
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.random.Random

class ReaderTests : Suite() {
    private fun reader(vararg values: Int) = UndraReader(bytesOf(*values))

    /** A string field: u32 length then the given raw (possibly invalid) bytes. */
    private fun stringField(vararg body: Int): ByteArray {
        val w = UndraWriter()
        w.writeLen(body.size)
        w.writeRaw(bytesOf(*body))
        return w.toByteArray()
    }

    /** A direct buffer with junk before and after [data], positioned at [data] (position 5, limit 5 + size). */
    private fun embeddedDirect(data: ByteArray): ByteBuffer {
        val b = ByteBuffer.allocateDirect(data.size + 12)
        for (i in 0 until b.capacity()) b.put(i, 0x77)
        b.position(5)
        b.put(data)
        b.position(5)
        b.limit(5 + data.size)
        return b
    }

    init {
        case("integers decode little-endian") {
            assertEq(0xABu.toUByte(), reader(0xAB).readU8())
            assertEq((-42).toByte(), reader(0xD6).readI8())
            assertEq(0x1234u.toUShort(), reader(0x34, 0x12).readU16())
            assertEq((-2).toShort(), reader(0xFE, 0xFF).readI16())
            assertEq(0x12345678u, reader(0x78, 0x56, 0x34, 0x12).readU32())
            assertEq(-2, reader(0xFE, 0xFF, 0xFF, 0xFF).readI32())
            assertEq(0x0123456789ABCDEFuL, reader(0xEF, 0xCD, 0xAB, 0x89, 0x67, 0x45, 0x23, 0x01).readU64())
            assertEq(Long.MIN_VALUE, reader(0, 0, 0, 0, 0, 0, 0, 0x80).readI64())
        }

        case("extremes decode correctly") {
            assertEq(UInt.MAX_VALUE, reader(0xFF, 0xFF, 0xFF, 0xFF).readU32())
            assertEq(-1, reader(0xFF, 0xFF, 0xFF, 0xFF).readI32())
            assertEq(ULong.MAX_VALUE, reader(0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF).readU64())
            assertEq(-1L, reader(0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF).readI64())
            assertEq(Long.MAX_VALUE, reader(0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F).readI64())
            assertEq(UShort.MAX_VALUE, reader(0xFF, 0xFF).readU16())
            assertEq(Short.MIN_VALUE, reader(0x00, 0x80).readI16())
            assertEq(Int.MIN_VALUE, reader(0, 0, 0, 0x80).readI32())
            assertEq(Byte.MIN_VALUE, reader(0x80).readI8())
            assertEq(UByte.MAX_VALUE, reader(0xFF).readU8())
        }

        case("floats decode from IEEE bits and keep NaN payloads") {
            assertEq(3.14f, reader(0xC3, 0xF5, 0x48, 0x40).readF32())
            assertEq(Math.E, reader(0x69, 0x57, 0x14, 0x8B, 0x0A, 0xBF, 0x05, 0x40).readF64())
            assertEq(0x7fc00001, reader(0x01, 0x00, 0xC0, 0x7F).readF32().toRawBits(), "f32 NaN payload")
            assertEq(0x7ff8000000000001L, reader(0x01, 0, 0, 0, 0, 0, 0xF8, 0x7F).readF64().toRawBits(), "f64 NaN payload")
            assertEq((-0.0f).toRawBits(), reader(0, 0, 0, 0x80).readF32().toRawBits(), "negative zero")
            assertTrue(reader(0, 0, 0x80, 0x7F).readF32() == Float.POSITIVE_INFINITY)
        }

        case("random values written by UndraWriter read back identically") {
            val rnd = Random(12345)
            val w = UndraWriter()
            val u8s = IntArray(500) { rnd.nextInt(256) }
            val i16s = IntArray(500) { rnd.nextInt() }
            val u32s = IntArray(500) { rnd.nextInt() }
            val i64s = LongArray(500) { rnd.nextLong() }
            val f64s = LongArray(500) { rnd.nextLong() }
            for (i in 0 until 500) {
                w.writeU8(u8s[i].toUByte())
                w.writeI16(i16s[i].toShort())
                w.writeU32(u32s[i].toUInt())
                w.writeI64(i64s[i])
                w.writeF64(Double.fromBits(f64s[i]))
            }
            val r = UndraReader(w.toByteArray())
            for (i in 0 until 500) {
                assertEq(u8s[i].toUByte(), r.readU8())
                assertEq(i16s[i].toShort(), r.readI16())
                assertEq(u32s[i].toUInt(), r.readU32())
                assertEq(i64s[i], r.readI64())
                assertEq(f64s[i], r.readF64().toRawBits())
            }
            r.finish()
        }

        case("bool accepts 0 and 1 and rejects every other byte") {
            assertEq(false, reader(0).readBool())
            assertEq(true, reader(1).readBool())
            for (b in 2..255) {
                val e = assertWire<WireException.InvalidTag>("byte $b") { reader(b).readBool() }
                assertEq(b.toUInt(), e.tag)
                assertEq(0, e.at)
                assertEq("bool", e.type)
            }
            val r = reader(7, 7, 2)
            r.skip(2)
            assertEq(2, assertWire<WireException.InvalidTag> { r.readBool() }.at, "offset of the bad byte")
        }

        case("reads past the end raise UnexpectedEof with the request size and offset") {
            val e1 = assertWire<WireException.UnexpectedEof> { reader().readU8() }
            assertEq(1, e1.needed)
            assertEq(0, e1.at)
            val e2 = assertWire<WireException.UnexpectedEof> { reader(1, 2, 3).readU32() }
            assertEq(4, e2.needed)
            assertEq(0, e2.at)
            val r = reader(1, 2, 3, 4, 5)
            r.readU32()
            val e3 = assertWire<WireException.UnexpectedEof> { r.readI64() }
            assertEq(8, e3.needed)
            assertEq(4, e3.at)
            for (n in 0 until 8) {
                assertWire<WireException.UnexpectedEof>("readI64 over $n bytes") { UndraReader(ByteArray(n)).readI64() }
            }
        }

        case("a failed read does not advance the cursor") {
            val r = reader(1, 2, 3)
            assertWire<WireException.UnexpectedEof> { r.readU32() }
            assertEq(0, r.position)
            assertEq(3, r.remaining)
            assertEq(0x0201.toShort(), r.readI16())
        }

        case("a window with offset and length is relative and bounded") {
            val backing = bytesOf(0xEE, 0xEE, 0x01, 0x00, 0x00, 0x00, 0xEE, 0xEE)
            val r = UndraReader(backing, 2, 4)
            assertEq(4, r.remaining)
            assertEq(0, r.position)
            assertEq(1, r.readI32())
            assertEq(0, r.remaining)
            assertEq(4, r.position)
            r.finish()
            val e = assertWire<WireException.UnexpectedEof> { r.readU8() }
            assertEq(4, e.at, "offsets are relative to the window start")
        }

        case("bytes outside the window are never visible") {
            val backing = bytesOf(1, 2, 3, 4, 5, 6)
            assertWire<WireException.UnexpectedEof> { UndraReader(backing, 1, 3).readI32() }
            assertWire<WireException.LengthTooLarge> { UndraReader(bytesOf(2, 0, 0, 0, 9, 9, 9), 0, 5).readBytes() }
            val r = UndraReader(backing, 2, 0)
            assertEq(0, r.remaining)
            r.finish()
        }

        case("invalid windows are rejected up front") {
            val backing = ByteArray(4)
            assertThrows<IllegalArgumentException> { UndraReader(backing, -1, 2) }
            assertThrows<IllegalArgumentException> { UndraReader(backing, 0, -1) }
            assertThrows<IllegalArgumentException> { UndraReader(backing, 3, 2) }
            assertThrows<IllegalArgumentException> { UndraReader(backing, 5, 0) }
            assertThrows<IllegalArgumentException> { UndraReader(backing, Int.MAX_VALUE, Int.MAX_VALUE) }
            assertThrows<IllegalArgumentException> { UndraReader(backing, 1, Int.MAX_VALUE) }
            UndraReader(backing, 4, 0).finish() // an empty window at the very end is fine
        }

        case("readLen accepts counts that fit") {
            assertEq(3, reader(3, 0, 0, 0, 1, 2, 3).readLen())
            assertEq(0, reader(0, 0, 0, 0).readLen())
            val r = reader(9, 9, 9, 4, 0, 0, 0, 1, 2, 3, 4)
            r.skip(3)
            assertEq(4, r.readLen(), "exactly the remaining bytes is fine")
        }

        case("readLen rejects counts larger than the remaining bytes before allocating anything") {
            val e = assertWire<WireException.LengthTooLarge> { reader(4, 0, 0, 0, 1, 2, 3).readLen() }
            assertEq(4u, e.len)
            assertEq(0, e.at)
            val huge = assertWire<WireException.LengthTooLarge> { reader(0xFF, 0xFF, 0xFF, 0xFF).readLen() }
            assertEq(UInt.MAX_VALUE, huge.len)
            val big = assertWire<WireException.LengthTooLarge> { reader(0, 0, 0, 0x80).readLen() }
            assertEq(0x80000000u, big.len, "counts of 2^31 and above are unsigned, never negative")
            val r = reader(0, 0, 0, 0xFF, 0, 0, 0)
            r.skip(3)
            assertEq(3, assertWire<WireException.LengthTooLarge> { r.readLen() }.at, "offset of the length prefix")
            assertWire<WireException.UnexpectedEof> { reader(1, 0, 0).readLen() }
        }

        case("readLen scales the check by the minimum item size") {
            val prefix = intArrayOf(3, 0, 0, 0)
            // 3 items of at least 4 bytes need 12 bytes to follow.
            assertEq(3, UndraReader(bytesOf(*prefix) + ByteArray(12)).readLen(4))
            val e = assertWire<WireException.LengthTooLarge> { UndraReader(bytesOf(*prefix) + ByteArray(11)).readLen(4) }
            assertEq(3u, e.len)
            assertEq(3, UndraReader(bytesOf(*prefix) + ByteArray(3)).readLen(1))
            assertThrows<IllegalArgumentException> { reader(0, 0, 0, 0).readLen(0) }
            assertThrows<IllegalArgumentException> { reader(0, 0, 0, 0).readLen(-1) }
            // The product must not overflow: 2^32-1 items of 2^30 bytes each.
            assertWire<WireException.LengthTooLarge> { reader(0xFF, 0xFF, 0xFF, 0xFF).readLen(1 shl 30) }
        }

        case("readStr decodes ASCII, multi-byte sequences and empty strings") {
            fun read(text: String): String {
                val w = UndraWriter()
                w.writeStr(text)
                val r = UndraReader(w.toByteArray())
                val s = r.readStr()
                r.finish()
                return s
            }
            for (s in listOf("", "a", "undra", cp(0xE9), cp(0x20AC), cp(0x1F30A), "h" + cp(0xE9) + "llo " + cp(0x1F30A), cp(0), "a" + cp(0) + "b")) {
                assertEq(s, read(s), "'${s.take(10)}'")
            }
        }

        case("readStr accepts the boundary code points of every UTF-8 length") {
            for (c in intArrayOf(0x7F, 0x80, 0x7FF, 0x800, 0xD7FF, 0xE000, 0xFFFD, 0xFFFE, 0xFFFF, 0x10000, 0x10FFFF)) {
                val text = cp(c)
                val w = UndraWriter()
                w.writeStr(text)
                assertEq(text, UndraReader(w.toByteArray()).readStr(), "U+${c.toString(16)}")
            }
        }

        case("readStr rejects malformed UTF-8 and reports the first bad byte") {
            // stringField puts 4 length bytes first, so the string body starts at offset 4.
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0x80)).readStr() }.at, "lone continuation byte")
            assertEq(6, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0x61, 0x62, 0xFF)).readStr() }.at, "0xFF after ascii")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xC0, 0x80)).readStr() }.at, "overlong NUL")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xC1, 0xBF)).readStr() }.at, "overlong DEL")
            assertEq(5, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0x61, 0xE2, 0x82)).readStr() }.at, "truncated at the end")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xE0, 0x80, 0x80)).readStr() }.at, "overlong 3-byte")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xED, 0xA0, 0x80)).readStr() }.at, "encoded surrogate U+D800")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xED, 0xBF, 0xBF)).readStr() }.at, "encoded surrogate U+DFFF")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xF4, 0x90, 0x80, 0x80)).readStr() }.at, "above U+10FFFF")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xF0, 0x80, 0x80, 0x80)).readStr() }.at, "overlong 4-byte")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xF8, 0x88, 0x80, 0x80, 0x80)).readStr() }.at, "5-byte form")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xC2)).readStr() }.at, "lead byte with no continuation")
            assertEq(4, assertWire<WireException.InvalidUtf8> { UndraReader(stringField(0xC2, 0x41)).readStr() }.at, "continuation replaced by ascii")
        }

        case("a string that is too long for the input is a length error, not a UTF-8 error") {
            assertWire<WireException.LengthTooLarge> { reader(5, 0, 0, 0, 0x61, 0x62).readStr() }
            assertWire<WireException.UnexpectedEof> { reader(5, 0).readStr() }
        }

        case("readStr leaves the cursor just after the string") {
            val w = UndraWriter()
            w.writeStr("ab")
            w.writeU8(9u)
            val r = UndraReader(w.toByteArray())
            assertEq("ab", r.readStr())
            assertEq(6, r.position)
            assertEq(9, r.readU8().toInt())
        }

        case("readBytes returns a copy that survives changes to the source") {
            val src = bytesOf(2, 0, 0, 0, 10, 20, 99)
            val r = UndraReader(src)
            val copy = r.readBytes()
            assertBytes("0a14", copy)
            src[4] = 0
            src[5] = 0
            assertBytes("0a14", copy, "the copy must be independent of the source array")
            assertEq(1, r.remaining)
        }

        case("readBytes rejects a length beyond the input") {
            assertWire<WireException.LengthTooLarge> { reader(3, 0, 0, 0, 1, 2).readBytes() }
            assertWire<WireException.LengthTooLarge> { reader(0xFF, 0xFF, 0xFF, 0xFF).readBytes() }
            assertBytes("", reader(0, 0, 0, 0).readBytes())
        }

        case("readRaw, readRemaining and skip") {
            val r = reader(1, 2, 3, 4, 5, 6)
            assertBytes("0102", r.readRaw(2))
            r.skip(1)
            assertBytes("040506", r.readRemaining())
            assertEq(0, r.remaining)
            assertBytes("", r.readRemaining())
            assertBytes("", r.readRaw(0))
            r.skip(0)
            assertWire<WireException.UnexpectedEof> { r.readRaw(1) }
            assertWire<WireException.UnexpectedEof> { r.skip(1) }
            assertThrows<IllegalArgumentException> { r.readRaw(-1) }
            assertThrows<IllegalArgumentException> { r.skip(-1) }
            assertWire<WireException.UnexpectedEof> { reader(1, 2).readRaw(Int.MAX_VALUE) }
        }

        case("finish accepts an exhausted reader and counts trailing bytes otherwise") {
            reader().finish()
            val r = reader(1, 2, 3)
            r.readU8()
            assertEq(2, assertWire<WireException.TrailingBytes> { r.finish() }.count)
            r.skip(2)
            r.finish()
        }

        case("a direct buffer is read in place, honouring its position and limit") {
            val data = bytesOf(0x78, 0x56, 0x34, 0x12, 0xFF, 0xEE)
            val buf = embeddedDirect(data)
            val r = UndraReader(buf)
            assertEq(6, r.remaining)
            assertEq(0, r.position)
            assertEq(0x12345678, r.readI32())
            assertEq(0xEEFF.toShort(), r.readI16())
            r.finish()
            assertWire<WireException.UnexpectedEof> { r.readU8() }
        }

        case("a direct buffer reader leaves the caller's buffer untouched") {
            val buf = embeddedDirect(bytesOf(1, 0, 0, 0, 2, 0, 0, 0))
            val r = UndraReader(buf)
            r.readI32()
            r.readI32()
            assertEq(5, buf.position())
            assertEq(13, buf.limit())
            assertEq(ByteOrder.BIG_ENDIAN, buf.order(), "byte order of the caller's buffer must not change")
        }

        case("a direct buffer reader decodes little-endian whatever the buffer's own order") {
            for (order in listOf(ByteOrder.BIG_ENDIAN, ByteOrder.LITTLE_ENDIAN)) {
                val buf = directBuffer(bytesOf(0x78, 0x56, 0x34, 0x12, 1, 2, 3, 4, 5, 6, 7, 8))
                buf.order(order)
                val r = UndraReader(buf)
                assertEq(0x12345678, r.readI32(), "$order i32")
                assertEq(0x0807060504030201L, r.readI64(), "$order i64")
            }
        }

        case("a direct buffer reader reads live memory, proving nothing was copied up front") {
            val buf = directBuffer(bytesOf(1, 0, 0, 0))
            val r = UndraReader(buf)
            buf.put(0, 2)
            assertEq(2, r.readI32(), "the reader must see the change made after it was created")
        }

        case("a direct buffer reader supports strings, bytes and lengths") {
            val w = UndraWriter()
            w.writeStr("h" + cp(0xE9) + "llo " + cp(0x1F30A))
            w.writeBytes(bytesOf(9, 8, 7))
            w.writeStr("plain ascii")
            val r = UndraReader(embeddedDirect(w.toByteArray()))
            assertEq("h" + cp(0xE9) + "llo " + cp(0x1F30A), r.readStr())
            assertBytes("090807", r.readBytes())
            assertEq("plain ascii", r.readStr())
            r.finish()
        }

        case("a direct buffer reader validates UTF-8 and lengths like the array reader") {
            assertEq(6, assertWire<WireException.InvalidUtf8> { UndraReader(directBuffer(stringField(0x61, 0x62, 0xFF))).readStr() }.at)
            assertEq(5, assertWire<WireException.InvalidUtf8> { UndraReader(directBuffer(stringField(0x61, 0xE2, 0x82))).readStr() }.at)
            assertWire<WireException.LengthTooLarge> { UndraReader(directBuffer(bytesOf(9, 0, 0, 0, 1))).readStr() }
            assertWire<WireException.LengthTooLarge> { UndraReader(directBuffer(bytesOf(9, 0, 0, 0, 1))).readBytes() }
            assertWire<WireException.UnexpectedEof> { UndraReader(directBuffer(bytesOf(1, 2, 3))).readI32() }
        }

        case("bytes read from a direct buffer are copies") {
            val buf = directBuffer(bytesOf(2, 0, 0, 0, 10, 20))
            val copy = UndraReader(buf).readBytes()
            buf.put(4, 0)
            buf.put(5, 0)
            assertBytes("0a14", copy, "recycling the buffer must not change bytes already read")
            val raw = UndraReader(buf).also { it.skip(4) }.readRaw(2)
            buf.put(4, 55)
            assertBytes("0000", raw)
        }

        case("empty, heap and read-only buffers work too") {
            UndraReader(ByteBuffer.allocateDirect(0)).finish()
            UndraReader(ByteBuffer.wrap(bytesOf(1, 0, 0, 0))).also { assertEq(1, it.readI32()) }
            val ro = ByteBuffer.wrap(bytesOf(0, 0, 0, 0, 7, 0, 0, 0)).asReadOnlyBuffer()
            ro.position(4)
            assertEq(7, UndraReader(ro).readI32(), "position of a read-only buffer is honoured")
            assertEq("hi", UndraReader(ByteBuffer.wrap(stringField(0x68, 0x69))).readStr())
        }

        case("big-endian uuid decoding works on every backing") {
            val bytes = bytesOf(0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF)
            val expected = java.util.UUID.fromString("00112233-4455-6677-8899-aabbccddeeff")
            assertEq(expected, Codecs.uuid.decode(UndraReader(bytes)))
            assertEq(expected, Codecs.uuid.decode(UndraReader(directBuffer(bytes))))
            assertEq(expected, Codecs.uuid.decode(UndraReader(ByteArray(3) + bytes, 3, 16)))
        }
    }

    @Test
    fun allCases() = assertPassed()
}
