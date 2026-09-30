package dev.keel.runtime.wire

import dev.keel.runtime.testing.Suite
import dev.keel.runtime.testing.assertBytes
import dev.keel.runtime.testing.assertEq
import dev.keel.runtime.testing.assertThrows
import dev.keel.runtime.testing.assertTrue
import dev.keel.runtime.testing.bytesOf
import dev.keel.runtime.testing.cp
import dev.keel.runtime.testing.hex
import org.junit.jupiter.api.Test

class WriterTests : Suite() {
    private fun write(block: KeelWriter.() -> Unit): ByteArray = KeelWriter().apply(block).toByteArray()

    init {
        case("integers are little-endian with the documented widths") {
            assertBytes("ab", write { writeU8(0xABu) })
            assertBytes("d6", write { writeI8(-42) })
            assertBytes("3412", write { writeU16(0x1234u) })
            assertBytes("feff", write { writeI16(-2) })
            assertBytes("78563412", write { writeU32(0x12345678u) })
            assertBytes("feffffff", write { writeI32(-2) })
            assertBytes("efcdab8967452301", write { writeU64(0x0123456789ABCDEFuL) })
            assertBytes("1032547698badcfe", write { writeI64(-0x0123456789ABCDF0L) })
        }

        case("extremes of every width") {
            assertBytes("00", write { writeU8(UByte.MIN_VALUE) })
            assertBytes("ff", write { writeU8(UByte.MAX_VALUE) })
            assertBytes("80", write { writeI8(Byte.MIN_VALUE) })
            assertBytes("7f", write { writeI8(Byte.MAX_VALUE) })
            assertBytes("ffff", write { writeU16(UShort.MAX_VALUE) })
            assertBytes("0080", write { writeI16(Short.MIN_VALUE) })
            assertBytes("ffffffff", write { writeU32(UInt.MAX_VALUE) })
            assertBytes("00000080", write { writeI32(Int.MIN_VALUE) })
            assertBytes("ffffffffffffffff", write { writeU64(ULong.MAX_VALUE) })
            assertBytes("0000000000000080", write { writeI64(Long.MIN_VALUE) })
            assertBytes("ffffffffffffff7f", write { writeI64(Long.MAX_VALUE) })
        }

        case("signed and unsigned writes of the same bits are identical") {
            assertBytes(hex(write { writeI32(-1) }), write { writeU32(UInt.MAX_VALUE) })
            assertBytes(hex(write { writeI64(-1L) }), write { writeU64(ULong.MAX_VALUE) })
            assertBytes(hex(write { writeI16(-1) }), write { writeU16(UShort.MAX_VALUE) })
            assertBytes(hex(write { writeI8(-1) }), write { writeU8(UByte.MAX_VALUE) })
        }

        case("floats are written as IEEE 754 bits, NaN payload included") {
            assertBytes("c3f54840", write { writeF32(3.14f) })
            assertBytes("6957148b0abf0540", write { writeF64(Math.E) })
            assertBytes("0000807f", write { writeF32(Float.POSITIVE_INFINITY) })
            assertBytes("000000000000f0ff", write { writeF64(Double.NEGATIVE_INFINITY) })
            assertBytes("00000080", write { writeF32(-0.0f) })
            assertBytes("0100c07f", write { writeF32(Float.fromBits(0x7fc00001)) })
            assertBytes("010000000000f87f", write { writeF64(Double.fromBits(0x7ff8000000000001L)) })
        }

        case("bool is one byte, 0 or 1") {
            assertBytes("01", write { writeBool(true) })
            assertBytes("00", write { writeBool(false) })
        }

        case("strings are u32 byte length plus UTF-8") {
            assertBytes("00000000", write { writeStr("") })
            assertBytes("03000000616263", write { writeStr("abc") })
            assertBytes("02000000c3a9", write { writeStr(cp(0xE9)) })
            assertBytes("03000000e282ac", write { writeStr(cp(0x20AC)) })
            assertBytes("04000000f09f8c8a", write { writeStr(cp(0x1F30A)) })
            assertBytes("0b00000068c3a96c6c6f20f09f8c8a", write { writeStr("h" + cp(0xE9) + "llo " + cp(0x1F30A)) })
        }

        case("embedded NUL and boundary code points encode as their shortest form") {
            assertBytes("0100000000", write { writeStr(cp(0)) })
            assertBytes("010000007f", write { writeStr(cp(0x7F)) })
            assertBytes("02000000c280", write { writeStr(cp(0x80)) })
            assertBytes("02000000dfbf", write { writeStr(cp(0x7FF)) })
            assertBytes("03000000e0a080", write { writeStr(cp(0x800)) })
            assertBytes("03000000efbfbf", write { writeStr(cp(0xFFFF)) })
            assertBytes("04000000f0908080", write { writeStr(cp(0x10000)) })
            assertBytes("04000000f48fbfbf", write { writeStr(cp(0x10FFFF)) })
        }

        case("writeStr length matches String.toByteArray for many strings") {
            val samples = listOf(
                "", "a", "hello world", cp(0xE9, 0xE8), cp(0x4E2D, 0x6587), cp(0x1F600) + " emoji",
                "mix " + cp(0xE9, 0x20AC, 0x1F600) + " end", "x".repeat(1000), cp(0x20AC).repeat(500),
            )
            for (s in samples) {
                val expected = s.toByteArray(Charsets.UTF_8)
                val out = write { writeStr(s) }
                assertBytes(hex(expected), out.copyOfRange(4, out.size), "body of '${s.take(12)}'")
                assertEq(expected.size, KeelReader(out).readLen(), "length prefix")
            }
        }

        case("a string with an unpaired surrogate is rejected and writes nothing") {
            for (bad in listOf("\ud800", "\udc00", "a\ud800", "\ud800a", "\udc00\ud800", "ok\ud83d")) {
                val w = KeelWriter()
                w.writeU8(7u)
                assertThrows<IllegalArgumentException>("'${bad.length} chars'") { w.writeStr(bad) }
                assertEq(1, w.size, "writer must be untouched after the failure")
            }
        }

        case("bytes are length-prefixed, raw is not") {
            assertBytes("0400000001020304", write { writeBytes(bytesOf(1, 2, 3, 4)) })
            assertBytes("00000000", write { writeBytes(ByteArray(0)) })
            assertBytes("01020304", write { writeRaw(bytesOf(1, 2, 3, 4)) })
            assertBytes("", write { writeRaw(ByteArray(0)) })
        }

        case("byte ranges are honoured and validated") {
            val src = bytesOf(10, 20, 30, 40, 50)
            assertBytes("020000001e28", write { writeBytes(src, 2, 2) })
            assertBytes("1e28", write { writeRaw(src, 2, 2) })
            assertBytes("00000000", write { writeBytes(src, 5, 0) })
            assertBytes("03000000141e28", write { writeBytes(src, 1, 3) })
            assertThrows<IllegalArgumentException> { KeelWriter().writeBytes(src, -1, 2) }
            assertThrows<IllegalArgumentException> { KeelWriter().writeBytes(src, 0, 6) }
            assertThrows<IllegalArgumentException> { KeelWriter().writeBytes(src, 4, 2) }
            assertThrows<IllegalArgumentException> { KeelWriter().writeRaw(src, 3, -1) }
            assertThrows<IllegalArgumentException> { KeelWriter().writeRaw(src, Int.MAX_VALUE, 2) }
        }

        case("writeLen writes a u32 and rejects negatives") {
            assertBytes("00000000", write { writeLen(0) })
            assertBytes("ffffff7f", write { writeLen(Int.MAX_VALUE) })
            val w = KeelWriter()
            assertThrows<IllegalArgumentException> { w.writeLen(-1) }
            assertEq(0, w.size)
        }

        case("size tracks bytes written and reset reuses the writer") {
            val w = KeelWriter()
            assertEq(0, w.size)
            w.writeU32(1u)
            w.writeU8(2u)
            assertEq(5, w.size)
            w.reset()
            assertEq(0, w.size)
            assertBytes("", w.toByteArray())
            w.writeU8(9u)
            assertBytes("09", w.toByteArray())
        }

        case("the writer grows from any initial capacity without corrupting data") {
            for (initial in listOf(0, 1, 2, 3, 7, 64, 1000)) {
                val w = KeelWriter(initial)
                val expected = KeelWriter(1 shl 16)
                for (i in 0 until 3000) {
                    w.writeI32(i)
                    w.writeU8((i and 0xFF).toUByte())
                    w.writeI64(i.toLong() * 0x0101010101L)
                    expected.writeI32(i)
                    expected.writeU8((i and 0xFF).toUByte())
                    expected.writeI64(i.toLong() * 0x0101010101L)
                }
                assertBytes(hex(expected.toByteArray()), w.toByteArray(), "initial capacity $initial")
                assertEq(3000 * 13, w.size)
            }
        }

        case("a single huge write grows in one step") {
            val w = KeelWriter(0)
            val big = ByteArray(5_000_000) { (it * 31).toByte() }
            w.writeBytes(big)
            assertEq(big.size + 4, w.size)
            val out = w.toByteArray()
            assertTrue(out.copyOfRange(4, out.size).contentEquals(big), "large payload corrupted")
        }

        case("toByteArray returns an independent copy and the writer stays usable") {
            val w = KeelWriter()
            w.writeU8(1u)
            val first = w.toByteArray()
            first[0] = 99
            assertBytes("01", w.toByteArray(), "mutating the copy must not change the writer")
            w.writeU8(2u)
            assertBytes("0102", w.toByteArray())
            assertBytes("01", first.also { it[0] = 1 }, "earlier copy is unaffected by later writes")
        }

        case("a negative initial capacity is rejected") {
            assertThrows<IllegalArgumentException> { KeelWriter(-1) }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
