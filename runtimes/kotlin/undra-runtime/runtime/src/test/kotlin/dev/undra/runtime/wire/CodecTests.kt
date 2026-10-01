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
import dev.undra.runtime.testing.hex
import org.junit.jupiter.api.Test
import java.time.Instant
import java.util.LinkedList
import java.util.UUID
import kotlin.random.Random
import kotlin.time.Duration
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.nanoseconds
import kotlin.time.Duration.Companion.seconds

/** Round trips and edge cases of every codec in [Codecs] and its combinators. */
class CodecTests : Suite() {
    /** A `UByte` from an int (an unsigned literal like `1u` is a `UInt`). */
    private fun ub(i: Int): UByte = i.toUByte()

    /** Encodes, decodes over every backing, checks the result equals the input and no byte is left over. */
    private fun <T> roundTrip(codec: UndraCodec<T>, value: T, message: String = ""): ByteArray {
        val bytes = codec.encodeToByteArray(value)
        assertEq(value, codec.decodeAll(bytes), "$message (array)")
        val direct = UndraReader(directBuffer(bytes))
        assertEq(value, codec.decode(direct), "$message (direct buffer)")
        direct.finish()
        val padded = ByteArray(bytes.size + 7) { 0x33 }
        System.arraycopy(bytes, 0, padded, 4, bytes.size)
        val window = UndraReader(padded, 4, bytes.size)
        assertEq(value, codec.decode(window), "$message (window)")
        window.finish()
        return bytes
    }

    private val strings = listOf(
        "",
        "a",
        "The quick brown fox",
        "h" + cp(0xE9) + "llo " + cp(0x1F30A),
        cp(0x4E2D, 0x6587, 0x5B57, 0x7B26),
        cp(0x1F468, 0x200D, 0x1F469, 0x200D, 0x1F467),
        cp(0),
        "line1\nline2\r\n\ttab",
        cp(0x7F, 0x80, 0x7FF, 0x800, 0xFFFF, 0x10000, 0x10FFFF),
        "x".repeat(10_000),
        cp(0x1F600).repeat(3_000),
    )

    init {
        case("bool") {
            assertBytes("01", roundTrip(Codecs.bool, true))
            assertBytes("00", roundTrip(Codecs.bool, false))
        }

        case("u8 and i8 round-trip every value") {
            for (i in 0..255) {
                assertBytes("%02x".format(i), roundTrip(Codecs.u8, i.toUByte()))
                roundTrip(Codecs.i8, i.toByte())
            }
        }

        case("u16 and i16 at and around their limits") {
            for (v in intArrayOf(0, 1, 0xFF, 0x100, 0x7FFF, 0x8000, 0xFFFE, 0xFFFF)) {
                roundTrip(Codecs.u16, v.toUShort())
                roundTrip(Codecs.i16, v.toShort())
            }
            assertBytes("ffff", roundTrip(Codecs.u16, UShort.MAX_VALUE))
            assertBytes("0080", roundTrip(Codecs.i16, Short.MIN_VALUE))
        }

        case("u32 and i32 at and around their limits") {
            val values = intArrayOf(0, 1, -1, 255, 256, 65535, 65536, Int.MAX_VALUE, Int.MIN_VALUE, Int.MIN_VALUE + 1, 0x12345678)
            for (v in values) {
                roundTrip(Codecs.i32, v)
                roundTrip(Codecs.u32, v.toUInt())
            }
            assertBytes("ffffffff", roundTrip(Codecs.u32, UInt.MAX_VALUE))
            assertBytes("00000080", roundTrip(Codecs.i32, Int.MIN_VALUE))
        }

        case("u64 and i64 at and around their limits") {
            val values = longArrayOf(
                0, 1, -1, Long.MAX_VALUE, Long.MIN_VALUE, Long.MIN_VALUE + 1, 0xFFFFFFFFL, 0x100000000L, 0x0123456789ABCDEFL,
            )
            for (v in values) {
                roundTrip(Codecs.i64, v)
                roundTrip(Codecs.u64, v.toULong())
            }
            assertBytes("ffffffffffffffff", roundTrip(Codecs.u64, ULong.MAX_VALUE))
            assertBytes("0000000000000080", roundTrip(Codecs.i64, Long.MIN_VALUE))
        }

        case("random integers of every width round-trip") {
            val rnd = Random(2024)
            repeat(2000) {
                roundTrip(Codecs.i16, rnd.nextInt().toShort())
                roundTrip(Codecs.u32, rnd.nextInt().toUInt())
                roundTrip(Codecs.i64, rnd.nextLong())
                roundTrip(Codecs.u64, rnd.nextLong().toULong())
            }
        }

        case("f32 round-trips specials bit-exactly") {
            val values = floatArrayOf(
                0f, -0f, 1f, -1f, Float.MIN_VALUE, Float.MAX_VALUE, -Float.MAX_VALUE, Float.POSITIVE_INFINITY,
                Float.NEGATIVE_INFINITY, Float.NaN, Float.fromBits(0x7fc00001), Float.fromBits(0xffc12345.toInt()), 1.17549435E-38f,
            )
            for (v in values) {
                assertEq(v.toRawBits(), Codecs.f32.decodeAll(Codecs.f32.encodeToByteArray(v)).toRawBits(), "bits of $v")
            }
        }

        case("f64 round-trips specials bit-exactly") {
            val values = doubleArrayOf(
                0.0, -0.0, 1.0, -1.0, Double.MIN_VALUE, Double.MAX_VALUE, -Double.MAX_VALUE, Double.POSITIVE_INFINITY,
                Double.NEGATIVE_INFINITY, Double.NaN, Double.fromBits(0x7ff8000000000001L), Double.fromBits(-0x000d4321a5b6c7d9L),
                2.2250738585072014E-308,
            )
            for (v in values) {
                assertEq(v.toRawBits(), Codecs.f64.decodeAll(Codecs.f64.encodeToByteArray(v)).toRawBits(), "bits of $v")
            }
        }

        case("unit encodes to nothing") {
            assertBytes("", roundTrip(Codecs.unit, Unit))
            assertEq(0, Codecs.unit.encodeToByteArray(Unit).size)
        }

        case("string round-trips unicode, empty and long strings") {
            for (s in strings) {
                val bytes = roundTrip(Codecs.string, s, "'${s.take(8)}'")
                assertEq(s.toByteArray(Charsets.UTF_8).size + 4, bytes.size)
            }
        }

        case("string encoding refuses unpaired surrogates instead of corrupting them") {
            assertThrows<IllegalArgumentException> { Codecs.string.encodeToByteArray("\ud83d") }
            assertThrows<IllegalArgumentException> { Codecs.string.encodeToByteArray("a\udfffb") }
        }

        case("bytes round-trips empty, every byte value and 1 MiB") {
            assertBytes("00000000", roundTrip(Codecs.bytes, ByteArray(0)))
            roundTrip(Codecs.bytes, ByteArray(256) { it.toByte() })
            val big = ByteArray(1 shl 20) { (it * 7 + (it shr 8)).toByte() }
            assertEq(big.size + 4, roundTrip(Codecs.bytes, big).size)
        }

        case("duration is i64 nanoseconds") {
            assertBytes("0000000000000000", roundTrip(Codecs.duration, Duration.ZERO))
            assertBytes("0100000000000000", roundTrip(Codecs.duration, 1.nanoseconds))
            assertBytes("002f685900000000", roundTrip(Codecs.duration, 1500.milliseconds))
            roundTrip(Codecs.duration, 1.seconds)
            roundTrip(Codecs.duration, 365.days)
            roundTrip(Codecs.duration, 4_611_686_018_427_387_903L.nanoseconds) // the largest value kotlin.time keeps exactly
            roundTrip(Codecs.duration, 123_456_789L.nanoseconds)
        }

        case("duration keeps only millisecond precision beyond kotlin.time's exact range") {
            val decoded = Codecs.duration.decodeAll(Codecs.i64.encodeToByteArray(Long.MAX_VALUE))
            assertEq(Long.MAX_VALUE / 1_000_000, decoded.inWholeMilliseconds)
            val reencoded = Codecs.duration.encodeToByteArray(decoded)
            assertBytes(
                hex(Codecs.i64.encodeToByteArray(Long.MAX_VALUE / 1_000_000 * 1_000_000)),
                reencoded,
                "re-encoding the lossy value gives whole-millisecond nanoseconds",
            )
        }

        case("a negative duration is rejected on encode and decode") {
            val enc = assertWire<WireException.NegativeDuration> { Codecs.duration.encodeToByteArray((-1).nanoseconds) }
            assertEq(-1L, enc.nanos)
            val w = UndraWriter()
            w.writeU32(7u)
            val atFour = assertWire<WireException.NegativeDuration> { Codecs.duration.encode(w, (-5).seconds) }
            assertEq(4, atFour.at, "encode offset")
            assertEq(4, w.size, "nothing is written for a rejected duration")
            assertWire<WireException.NegativeDuration> { Codecs.duration.encodeToByteArray(Duration.INFINITE * -1) }
            val dec = assertWire<WireException.NegativeDuration> { Codecs.duration.decodeAll(Codecs.i64.encodeToByteArray(-1L)) }
            assertEq(-1L, dec.nanos)
            assertEq(0, dec.at)
            assertWire<WireException.NegativeDuration> { Codecs.duration.decodeAll(Codecs.i64.encodeToByteArray(Long.MIN_VALUE)) }
            val r = UndraReader(UndraWriter().also { it.writeU8(1u); it.writeI64(-9) }.toByteArray())
            r.skip(1)
            assertEq(1, assertWire<WireException.NegativeDuration> { Codecs.duration.decode(r) }.at)
        }

        case("durations that do not fit in i64 nanoseconds are refused") {
            assertThrows<IllegalArgumentException> { Codecs.duration.encodeToByteArray(Duration.INFINITE) }
            assertThrows<IllegalArgumentException> { Codecs.duration.encodeToByteArray(365.days * 300) }
            assertThrows<IllegalArgumentException> { Codecs.duration.encodeToByteArray(Long.MAX_VALUE.milliseconds) }
            Codecs.duration.encodeToByteArray(365.days * 290) // about 9.15e18 ns, still below 2^63
        }

        case("timestamp is i64 milliseconds since the epoch") {
            assertBytes("0000000000000000", roundTrip(Codecs.timestamp, Timestamp(0)))
            assertBytes("00103a4092010000", roundTrip(Codecs.timestamp, Timestamp(1727654400000)))
            assertBytes("ffffffffffffffff", roundTrip(Codecs.timestamp, Timestamp(-1)))
            roundTrip(Codecs.timestamp, Timestamp(Long.MIN_VALUE))
            roundTrip(Codecs.timestamp, Timestamp(Long.MAX_VALUE))
        }

        case("Timestamp converts to and from java.time.Instant, flooring sub-millisecond parts") {
            val i = Instant.parse("2024-09-30T00:00:00Z")
            assertEq(1727654400000L, Timestamp.ofInstant(i).epochMillis)
            assertEq(i, Timestamp(1727654400000L).toInstant())
            assertEq(1L, Timestamp.ofInstant(Instant.ofEpochSecond(0, 1_999_999)).epochMillis, "1.999999 ms floors to 1")
            assertEq(-1L, Timestamp.ofInstant(Instant.ofEpochSecond(0, -1)).epochMillis, "1 ns before the epoch is -1 ms, not 0")
            assertEq(-2L, Timestamp.ofInstant(Instant.ofEpochSecond(0, -1_000_001)).epochMillis)
            assertEq(Instant.ofEpochMilli(Long.MAX_VALUE), Timestamp(Long.MAX_VALUE).toInstant())
            assertEq(Instant.ofEpochMilli(Long.MIN_VALUE), Timestamp(Long.MIN_VALUE).toInstant())
            assertThrows<IllegalArgumentException> { Timestamp.ofInstant(Instant.MAX) }
            assertThrows<IllegalArgumentException> { Timestamp.ofInstant(Instant.MIN) }
            assertTrue(Timestamp(1) < Timestamp(2))
            assertEq(Timestamp(5), Timestamp(5))
        }

        case("uuid is 16 bytes, most significant first") {
            val u = UUID.fromString("00112233-4455-6677-8899-aabbccddeeff")
            assertBytes("00112233445566778899aabbccddeeff", roundTrip(Codecs.uuid, u))
            assertBytes("00000000000000000000000000000000", roundTrip(Codecs.uuid, UUID(0, 0)))
            assertBytes("ffffffffffffffffffffffffffffffff", roundTrip(Codecs.uuid, UUID(-1, -1)))
            assertBytes("80000000000000000000000000000001", roundTrip(Codecs.uuid, UUID(Long.MIN_VALUE, 1)))
            repeat(200) { roundTrip(Codecs.uuid, UUID.randomUUID()) }
        }

        case("handle codec is the raw u64") {
            assertBytes("0100000001000000", roundTrip(Codecs.handle, Handle.make(1u, 1u).raw))
            assertBytes("0000000000000000", roundTrip(Codecs.handle, 0L))
            assertBytes("ffffffffffffffff", roundTrip(Codecs.handle, -1L))
        }

        case("option is a tag byte then the item") {
            val c = Codecs.option(Codecs.string)
            assertBytes("00", roundTrip(c, null))
            assertBytes("0100000000", roundTrip(c, ""))
            assertBytes("010100000078", roundTrip(c, "x"))
            val ints = Codecs.vec(Codecs.option(Codecs.i32))
            assertBytes("03000000" + "0101000000" + "00" + "0103000000", roundTrip(ints, listOf(1, null, 3)))
            roundTrip(Codecs.option(Codecs.vec(Codecs.string)), listOf("a", "b"))
            roundTrip(Codecs.option(Codecs.vec(Codecs.string)), emptyList())
            roundTrip(Codecs.option(Codecs.unit), Unit)
            roundTrip(Codecs.option(Codecs.unit), null)
        }

        case("option rejects unknown tags and truncated items") {
            val c = Codecs.option(Codecs.i32)
            for (tag in 2..255) {
                val e = assertWire<WireException.InvalidTag>("tag $tag") { c.decodeAll(bytesOf(tag)) }
                assertEq(tag.toUInt(), e.tag)
                assertEq(0, e.at)
                assertEq("Option", e.type)
            }
            assertWire<WireException.UnexpectedEof> { c.decodeAll(bytesOf(1, 1, 0)) }
            assertWire<WireException.UnexpectedEof> { c.decodeAll(ByteArray(0)) }
        }

        case("vec is a u32 count then the items") {
            assertBytes("00000000", roundTrip(Codecs.vec(Codecs.i32), emptyList()))
            assertBytes("0300000001000000ffffffff07000000", roundTrip(Codecs.vec(Codecs.i32), listOf(1, -1, 7)))
            roundTrip(Codecs.vec(Codecs.string), strings)
            roundTrip(Codecs.vec(Codecs.vec(Codecs.i32)), listOf(listOf(1), emptyList(), listOf(2, 3)))
            roundTrip(Codecs.vec(Codecs.bool), List(1000) { it % 3 == 0 })
            val big = List(100_000) { it * 31 }
            assertEq(4 + 4 * big.size, roundTrip(Codecs.vec(Codecs.i32), big).size)
        }

        case("vec encodes non-random-access lists exactly like array lists") {
            val values = listOf(5, 6, 7)
            val c = Codecs.vec(Codecs.i32)
            assertBytes(hex(c.encodeToByteArray(values)), c.encodeToByteArray(LinkedList(values)))
        }

        case("vec rejects counts that cannot fit, without allocating them") {
            val bomb = bytesOf(0xFF, 0xFF, 0xFF, 0xFF)
            val codecs: List<UndraCodec<*>> = listOf(
                Codecs.vec(Codecs.i32), Codecs.vec(Codecs.string), Codecs.vec(Codecs.bytes),
                Codecs.vec(Codecs.vec(Codecs.i32)), Codecs.vec(Codecs.bool), Codecs.vec(Todo),
            )
            for (c in codecs) {
                val e = assertWire<WireException.LengthTooLarge> { c.decodeAll(bomb) }
                assertEq(UInt.MAX_VALUE, e.len)
                assertEq(0, e.at)
            }
            val e = assertWire<WireException.LengthTooLarge> { Codecs.vec(Codecs.i32).decodeAll(bytesOf(0, 0, 0, 0x40, 1, 2, 3, 4)) }
            assertEq(0x40000000u, e.len)
        }

        case("vec items are validated one by one") {
            // count 2 fits the two remaining bytes, but the first i32 does not.
            val e = assertWire<WireException.UnexpectedEof> { Codecs.vec(Codecs.i32).decodeAll(bytesOf(2, 0, 0, 0, 1, 0)) }
            assertEq(4, e.needed)
            assertEq(4, e.at)
            assertWire<WireException.InvalidTag> { Codecs.vec(Codecs.bool).decodeAll(bytesOf(2, 0, 0, 0, 1, 9)) }
            assertWire<WireException.InvalidUtf8> { Codecs.vec(Codecs.string).decodeAll(bytesOf(1, 0, 0, 0, 1, 0, 0, 0, 0xFF)) }
        }

        case("a vec of a zero-width type is a documented limitation") {
            val units = Codecs.vec(Codecs.unit)
            assertBytes("00000000", roundTrip(units, emptyList()))
            assertBytes("03000000", units.encodeToByteArray(listOf(Unit, Unit, Unit)))
            // The decoder cannot tell a legitimate count of 3 from a hostile one, so it refuses both.
            assertEq(3u, assertWire<WireException.LengthTooLarge> { units.decodeAll(bytesOf(3, 0, 0, 0)) }.len)
        }

        case("map is a u32 count then pairs sorted by encoded key bytes") {
            val c = Codecs.map(Codecs.string, Codecs.i32)
            assertBytes("00000000", roundTrip(c, emptyMap()))
            assertBytes(
                "02000000" + "0100000061" + "01000000" + "0100000062" + "02000000",
                roundTrip(c, linkedMapOf("b" to 2, "a" to 1)),
            )
            assertBytes("01000000" + "0100000061" + "01000000", roundTrip(c, mapOf("a" to 1)))
        }

        case("map order is by encoded bytes: length prefix first, little-endian integers, unsigned bytes") {
            // "b" encodes as 01000000 62 and "aa" as 02000000 6161: the length prefix decides.
            val c = Codecs.map(Codecs.string, Codecs.i32)
            assertBytes(
                "02000000" + "0100000062" + "02000000" + "020000006161" + "01000000",
                roundTrip(c, linkedMapOf("aa" to 1, "b" to 2)),
            )
            // 256 encodes as 00 01 00 00 and 1 as 01 00 00 00, so 256 sorts first.
            val byInt = Codecs.map(Codecs.i32, Codecs.string)
            assertBytes(
                "02000000" + "00010000" + "0100000078" + "01000000" + "0100000079",
                roundTrip(byInt, linkedMapOf(1 to "y", 256 to "x")),
            )
            // Unsigned comparison: 0x80 sorts after 0x7f, not before.
            val byByte = Codecs.map(Codecs.u8, Codecs.u8)
            val unsorted = linkedMapOf(ub(0x80) to ub(3), ub(0x01) to ub(1), ub(0x7F) to ub(2))
            assertBytes("03000000" + "0101" + "7f02" + "8003", roundTrip(byByte, unsorted))
        }

        case("equal maps encode to identical bytes whatever their iteration order") {
            val entries = (0 until 12).map { "key-$it" to it }
            val c = Codecs.map(Codecs.string, Codecs.i32)
            val reference = c.encodeToByteArray(entries.toMap(LinkedHashMap()))
            val rnd = Random(99)
            repeat(50) {
                val shuffled = entries.shuffled(rnd).toMap(LinkedHashMap())
                assertBytes(hex(reference), c.encodeToByteArray(shuffled), "shuffled insertion order")
            }
        }

        case("map decoding accepts any pair order and preserves it") {
            val c = Codecs.map(Codecs.string, Codecs.i32)
            val unsorted = bytesOf(2, 0, 0, 0, 1, 0, 0, 0, 0x62, 2, 0, 0, 0, 1, 0, 0, 0, 0x61, 1, 0, 0, 0)
            val m = c.decodeAll(unsorted)
            assertEq(listOf("b", "a"), m.keys.toList())
            assertEq(mapOf("a" to 1, "b" to 2), m)
        }

        case("map decoding rejects a repeated key and reports where it starts") {
            val c = Codecs.map(Codecs.string, Codecs.i32)
            // count 2, then ("a", 1) at offsets 4..13 and ("a", 2) starting at 13.
            val dup = bytesOf(2, 0, 0, 0, 1, 0, 0, 0, 0x61, 1, 0, 0, 0, 1, 0, 0, 0, 0x61, 2, 0, 0, 0)
            assertEq(13, assertWire<WireException.DuplicateKey> { c.decodeAll(dup) }.at)
            // null values must not hide a duplicate.
            val nullable = Codecs.map(Codecs.string, Codecs.option(Codecs.i32))
            val dupNull = bytesOf(2, 0, 0, 0, 1, 0, 0, 0, 0x61, 0, 1, 0, 0, 0, 0x61, 0)
            assertEq(10, assertWire<WireException.DuplicateKey> { nullable.decodeAll(dupNull) }.at)
            // three entries, the third repeats the first.
            val third = bytesOf(3, 0, 0, 0, 1, 0, 0, 0, 0x61, 0, 1, 0, 0, 0, 0x62, 0, 1, 0, 0, 0, 0x61, 0)
            assertEq(16, assertWire<WireException.DuplicateKey> { nullable.decodeAll(third) }.at)
        }

        case("map encoding rejects keys whose encodings are identical") {
            val c = Codecs.map(Codecs.bytes, Codecs.u8)
            // Two distinct ByteArray instances with equal contents are two keys to Kotlin but one on the wire.
            val twins = linkedMapOf(bytesOf(1, 2) to ub(1), bytesOf(1, 2) to ub(2))
            assertEq(2, twins.size)
            assertEq(4, assertWire<WireException.DuplicateKey> { c.encodeToByteArray(twins) }.at, "offset where the entries begin")
            val distinct = linkedMapOf(bytesOf(1, 2) to ub(1), bytesOf(1, 3) to ub(2))
            assertEq(4 + 2 * (4 + 2 + 1), c.encodeToByteArray(distinct).size)
        }

        case("maps nest and carry containers and records") {
            val c = Codecs.map(Codecs.string, Codecs.vec(Codecs.i32))
            roundTrip(c, linkedMapOf("x" to listOf(1, 2, 3), "y" to emptyList(), "z" to listOf(-1)))
            val nested = Codecs.map(Codecs.i32, Codecs.map(Codecs.string, Codecs.bool))
            roundTrip(nested, linkedMapOf(3 to linkedMapOf("t" to true, "f" to false), 1 to emptyMap(), 2 to linkedMapOf("only" to true)))
            val byUuid = (0 until 20).associate { i -> UUID(0, i.toLong()) to Todo(UUID(1, i.toLong()), "t$i", i % 2 == 0) }
            roundTrip(Codecs.map(Codecs.uuid, Todo), byUuid)
            val byRecord = mapOf(Todo(UUID(0, 1), "a", true) to Unit, Todo(UUID(0, 2), "b", false) to Unit)
            roundTrip(Codecs.map(Todo, Codecs.unit), byRecord)
        }

        case("map decoding rejects counts that cannot fit") {
            val bomb = bytesOf(0xFF, 0xFF, 0xFF, 0xFF)
            val e = assertWire<WireException.LengthTooLarge> { Codecs.map(Codecs.string, Codecs.i32).decodeAll(bomb) }
            assertEq(UInt.MAX_VALUE, e.len)
            assertWire<WireException.UnexpectedEof> { Codecs.map(Codecs.i32, Codecs.i32).decodeAll(bytesOf(2, 0, 0, 0, 1, 0, 0, 0)) }
        }

        case("result is a tag byte then the ok or error value") {
            val c = Codecs.result(Codecs.i32, Codecs.string)
            assertBytes("0005000000", roundTrip(c, UndraResult.Ok(5)))
            assertBytes("0103000000626164", roundTrip(c, UndraResult.Err("bad")))
            roundTrip(Codecs.result(Codecs.unit, Codecs.unit), UndraResult.Ok(Unit))
            roundTrip(Codecs.result(Codecs.unit, Codecs.string), UndraResult.Err(""))
            roundTrip(Codecs.result(Codecs.vec(Todo), Codecs.option(Codecs.string)), UndraResult.Err(null))
            roundTrip(Codecs.result(Codecs.result(Codecs.i32, Codecs.i32), Codecs.bool), UndraResult.Ok(UndraResult.Err(-1)))
        }

        case("result rejects unknown tags") {
            val c = Codecs.result(Codecs.i32, Codecs.i32)
            for (tag in 2..255) {
                val e = assertWire<WireException.InvalidTag>("tag $tag") { c.decodeAll(bytesOf(tag, 0, 0, 0, 0)) }
                assertEq(tag.toUInt(), e.tag)
                assertEq("Result", e.type)
            }
            assertWire<WireException.UnexpectedEof> { c.decodeAll(bytesOf(0, 1)) }
        }

        case("UndraResult accessors") {
            val ok: UndraResult<Int, String> = UndraResult.Ok(3)
            val err: UndraResult<Int, String> = UndraResult.Err("no")
            assertTrue(ok.isOk && !ok.isErr)
            assertTrue(err.isErr && !err.isOk)
            assertEq(3, ok.getOrNull())
            assertEq(null, err.getOrNull())
            assertEq("no", err.errorOrNull())
            assertEq(null, ok.errorOrNull())
            assertEq("ok:3", ok.fold({ "ok:$it" }, { "err:$it" }))
            assertEq("err:no", err.fold({ "ok:$it" }, { "err:$it" }))
            assertEq<UndraResult<Int, String>>(UndraResult.Ok(3), ok)
            assertTrue(ok != err)
        }

        case("records and enums compose the way generated code does") {
            val todo = Todo(UUID.fromString("123e4567-e89b-12d3-a456-426614174000"), "Milk " + cp(0x1F95B), true)
            roundTrip(Todo, todo)
            roundTrip(Codecs.vec(Todo), List(100) { Todo(UUID(it.toLong(), -it.toLong()), "todo $it " + cp(0xE9), it % 2 == 1) })
            for (f in Filter.entries) roundTrip(Filter, f)
            roundTrip(Shape, Shape.Circle(1.5))
            roundTrip(Shape, Shape.Rect(2.0, 3.0))
            roundTrip(Codecs.option(Filter), Filter.ACTIVE)
        }

        case("unknown enum indexes are InvalidTag with the offset of the index") {
            val e = assertWire<WireException.InvalidTag> { Filter.decodeAll(bytesOf(3, 0)) }
            assertEq(3u, e.tag)
            assertEq(0, e.at)
            assertEq("Filter", e.type)
            assertEq(0xFFFFu, assertWire<WireException.InvalidTag> { Filter.decodeAll(bytesOf(0xFF, 0xFF)) }.tag)
            val inVec = assertWire<WireException.InvalidTag> { Codecs.vec(Shape).decodeAll(bytesOf(1, 0, 0, 0, 2, 0)) }
            assertEq(4, inVec.at)
            assertWire<WireException.UnexpectedEof> { Shape.decodeAll(bytesOf(1, 0, 0, 0)) }
        }

        case("decodeAll rejects trailing bytes and encodeToByteArray matches manual encoding") {
            assertEq(1, assertWire<WireException.TrailingBytes> { Codecs.i32.decodeAll(bytesOf(1, 0, 0, 0, 9)) }.count)
            assertEq(3, assertWire<WireException.TrailingBytes> { Codecs.bool.decodeAll(bytesOf(1, 9, 9, 9)) }.count)
            assertBytes("0a000000", Codecs.i32.encodeToByteArray(10))
            assertEq(9, Codecs.u8.decodeAll(bytesOf(9)).toInt())
        }
    }

    @Test
    fun allCases() = assertPassed()
}
