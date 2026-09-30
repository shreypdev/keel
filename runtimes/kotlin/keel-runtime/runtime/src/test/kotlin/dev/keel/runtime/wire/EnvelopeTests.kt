package dev.keel.runtime.wire

import dev.keel.runtime.testing.Suite
import dev.keel.runtime.testing.assertBytes
import dev.keel.runtime.testing.assertEq
import dev.keel.runtime.testing.assertTrue
import dev.keel.runtime.testing.assertWire
import dev.keel.runtime.testing.bytesOf
import dev.keel.runtime.testing.hex
import dev.keel.runtime.testing.unhex
import org.junit.jupiter.api.Test

class EnvelopeTests : Suite() {
    private val schema = 0x0102030405060708uL
    private val sample = Envelope.encode(Envelope.Kind.CALL, 7u, schema, bytesOf(0xAA, 0xBB, 0xCC))

    /** [sample] with one byte replaced. */
    private fun patched(index: Int, value: Int): ByteArray = sample.copyOf().also { it[index] = value.toByte() }

    init {
        case("the header is 23 bytes laid out as magic, version, schema, kind, seq, len") {
            assertEq(23, Envelope.HEADER_LEN)
            assertBytes("4b45454c" + "0100" + "0807060504030201" + "01" + "07000000" + "03000000" + "aabbcc", sample)
            assertEq(Envelope.HEADER_LEN + 3, sample.size)
            assertEq(23, Envelope.encode(Envelope.Kind.CANCEL, 0u, 0uL, ByteArray(0)).size, "an empty payload is just the header")
        }

        case("magic is the ASCII bytes KEEL") {
            assertEq("KEEL", String(sample, 0, 4, Charsets.US_ASCII))
        }

        case("Kind has the 16 values of the spec table, in order, with their wire codes") {
            val expected = listOf(
                "CALL" to 1, "REPLY" to 2, "CHANGE_SET" to 3, "PORT_CALL" to 4, "PORT_REPLY" to 5, "CANCEL" to 6,
                "STREAM_CREDIT" to 7, "STREAM_ITEM" to 8, "OBSERVE" to 9, "RELEASE" to 10, "EVENT" to 11, "HELLO" to 12,
                "LOG" to 13, "TIMER_FIRED" to 14, "SNAPSHOT" to 15, "RESTORE" to 16,
            )
            assertEq(expected, Envelope.Kind.entries.map { it.name to it.code.toInt() })
        }

        case("Kind.fromByte maps every code and rejects the rest with the offending tag") {
            for (k in Envelope.Kind.entries) assertEq(k, Envelope.Kind.fromByte(k.code))
            for (b in listOf(0, 17, 18, 100, 254, 255)) {
                val e = assertWire<WireException.InvalidTag>("code $b") { Envelope.Kind.fromByte(b.toUByte()) }
                assertEq(b.toUInt(), e.tag)
                assertEq(-1, e.at, "unknown offset")
                assertEq("Envelope.Kind", e.type)
            }
            assertEq(9, assertWire<WireException.InvalidTag> { Envelope.Kind.fromByte(0u, 9) }.at)
        }

        case("every kind round-trips") {
            for (k in Envelope.Kind.entries) {
                val payload = ByteArray(k.code.toInt()) { (it + 1).toByte() }
                val decoded = Envelope.decode(Envelope.encode(k, 100u + k.code.toUInt(), schema, payload))
                assertEq(k, decoded.kind)
                assertEq(100u + k.code.toUInt(), decoded.seq)
                assertEq(schema, decoded.schemaHash)
                assertBytes(hex(payload), decoded.payload)
            }
        }

        case("seq and schema hash use their full unsigned range") {
            val bytes = Envelope.encode(Envelope.Kind.HELLO, UInt.MAX_VALUE, ULong.MAX_VALUE, ByteArray(0))
            val e = Envelope.decode(bytes)
            assertEq(UInt.MAX_VALUE, e.seq)
            assertEq(ULong.MAX_VALUE, e.schemaHash)
            val zero = Envelope.decode(Envelope.encode(Envelope.Kind.HELLO, 0u, 0uL, ByteArray(0)))
            assertEq(0u, zero.seq)
            assertEq(0uL, zero.schemaHash)
        }

        case("payloads of any size survive") {
            for (n in intArrayOf(0, 1, 2, 255, 256, 65535, 65536, 1 shl 20)) {
                val payload = ByteArray(n) { (it * 13 + n).toByte() }
                val decoded = Envelope.decode(Envelope.encode(Envelope.Kind.REPLY, 1u, schema, payload))
                assertEq(n, decoded.payload.size)
                assertTrue(decoded.payload.contentEquals(payload), "payload of $n bytes corrupted")
            }
        }

        case("decode returns a payload copy and encode does not alias its input") {
            val input = sample.copyOf()
            val decoded = Envelope.decode(input)
            input[Envelope.HEADER_LEN] = 0
            assertBytes("aabbcc", decoded.payload, "decoded payload must not alias the input array")
            val payload = bytesOf(1, 2)
            val framed = Envelope.encode(Envelope.Kind.CALL, 1u, 1uL, payload)
            payload[0] = 9
            assertEq(1, framed[Envelope.HEADER_LEN].toInt(), "encoded frame must not alias the payload")
        }

        case("a wrong magic is BadMagic and names what was found") {
            val e = assertWire<WireException.BadMagic> { Envelope.decode(patched(3, 'X'.code)) }
            assertEq("4b454558", e.found)
            assertEq("00000000", assertWire<WireException.BadMagic> { Envelope.decode(ByteArray(30)) }.found)
            assertEq("6b65656c", assertWire<WireException.BadMagic> { Envelope.decode(sample.copyOf().also { "keel".toByteArray().copyInto(it) }) }.found)
            assertWire<WireException.BadMagic>("magic is checked before the version") { Envelope.decode(ByteArray(30) { 0xFF.toByte() }) }
        }

        case("an unsupported version is reported with its number") {
            for (v in intArrayOf(0, 2, 3, 0x0100, 0xFFFF)) {
                val bytes = sample.copyOf().also { it[4] = v.toByte(); it[5] = (v shr 8).toByte() }
                assertEq(v.toUShort(), assertWire<WireException.UnsupportedVersion>("version $v") { Envelope.decode(bytes) }.version)
            }
        }

        case("every truncation of the header is UnexpectedEof") {
            for (n in 0 until Envelope.HEADER_LEN) {
                val e = assertWire<WireException.UnexpectedEof>("header truncated to $n bytes") { Envelope.decode(sample.copyOf(n)) }
                assertTrue(e.at <= n, "offset must lie inside the input")
            }
        }

        case("every truncation of the payload is LengthTooLarge") {
            for (n in Envelope.HEADER_LEN until sample.size) {
                val e = assertWire<WireException.LengthTooLarge>("truncated to $n bytes") { Envelope.decode(sample.copyOf(n)) }
                assertEq(3u, e.len)
                assertEq(19, e.at, "offset of the len field")
            }
        }

        case("a declared payload length beyond the input is rejected, however large") {
            val huge = sample.copyOf().also { for (i in 19..22) it[i] = 0xFF.toByte() }
            assertEq(UInt.MAX_VALUE, assertWire<WireException.LengthTooLarge> { Envelope.decode(huge) }.len)
            val big = sample.copyOf().also { it[19] = 4 }
            assertEq(4u, assertWire<WireException.LengthTooLarge> { Envelope.decode(big) }.len)
        }

        case("bytes after the payload are TrailingBytes") {
            assertEq(1, assertWire<WireException.TrailingBytes> { Envelope.decode(sample + 0) }.count)
            assertEq(5, assertWire<WireException.TrailingBytes> { Envelope.decode(sample + ByteArray(5)) }.count)
            val shortLen = sample.copyOf().also { it[19] = 2 }
            assertEq(1, assertWire<WireException.TrailingBytes> { Envelope.decode(shortLen) }.count, "len says 2 but 3 bytes follow")
        }

        case("an unknown kind reports the tag and its offset") {
            for (k in listOf(0, 17, 200, 255)) {
                val e = assertWire<WireException.InvalidTag>("kind $k") { Envelope.decode(patched(14, k)) }
                assertEq(k.toUInt(), e.tag)
                assertEq(14, e.at)
                assertEq("Envelope.Kind", e.type)
            }
        }

        case("expectedSchema is enforced right after the header field is read") {
            assertEq(schema, Envelope.decode(sample, expectedSchema = schema).schemaHash)
            val e = assertWire<WireException.SchemaMismatch> { Envelope.decode(sample, expectedSchema = 5uL) }
            assertEq(5uL, e.expected)
            assertEq(schema, e.got)
            assertWire<WireException.SchemaMismatch>("a mismatch outranks a later malformed field") {
                Envelope.decode(patched(14, 99), expectedSchema = 5uL)
            }
            assertWire<WireException.SchemaMismatch> { Envelope.decode(sample + 0, expectedSchema = 5uL) }
            assertEq(schema, Envelope.decode(patched(14, 2)).schemaHash, "without expectedSchema any hash is accepted")
            assertWire<WireException.BadMagic>("structure is still checked first") { Envelope.decode(patched(0, 0), expectedSchema = 5uL) }
        }

        case("Envelope compares by content") {
            val a = Envelope(Envelope.Kind.LOG, 1u, 2uL, bytesOf(1, 2))
            val b = Envelope(Envelope.Kind.LOG, 1u, 2uL, bytesOf(1, 2))
            assertEq(a, b)
            assertEq(a.hashCode(), b.hashCode())
            assertTrue(a != Envelope(Envelope.Kind.LOG, 1u, 2uL, bytesOf(1, 3)))
            assertTrue(a != Envelope(Envelope.Kind.LOG, 2u, 2uL, bytesOf(1, 2)))
            assertTrue(a != Envelope(Envelope.Kind.LOG, 1u, 3uL, bytesOf(1, 2)))
            assertTrue(a != Envelope(Envelope.Kind.HELLO, 1u, 2uL, bytesOf(1, 2)))
            assertEq(sample.toList(), Envelope(Envelope.Kind.CALL, 7u, schema, bytesOf(0xAA, 0xBB, 0xCC)).encode().toList())
            assertTrue(a.toString().contains("LOG"))
        }

        case("decoding the shared vector frame gives the documented fields") {
            val e = Envelope.decode(unhex("4b45454c01000807060504030201010700000003000000aabbcc"))
            assertEq(Envelope.Kind.CALL, e.kind)
            assertEq(7u, e.seq)
            assertEq(72623859790382856uL, e.schemaHash)
            assertBytes("aabbcc", e.payload)
        }
    }

    @Test
    fun allCases() = assertPassed()
}
