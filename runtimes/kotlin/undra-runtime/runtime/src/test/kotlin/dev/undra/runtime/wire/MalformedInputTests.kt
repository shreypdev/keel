package dev.undra.runtime.wire

import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.assertWire
import dev.undra.runtime.testing.bytesOf
import dev.undra.runtime.testing.unhex
import org.junit.jupiter.api.Test
import java.util.UUID
import kotlin.time.Duration.Companion.seconds

/**
 * One section per [WireException] subclass: how each is triggered through the public API, with the
 * fields it must carry. The exhaustive `when` in [nameOf] stops compiling if a subclass is added
 * without extending this suite.
 */
class MalformedInputTests : Suite() {
    private fun nameOf(e: WireException): String = when (e) {
        is WireException.UnexpectedEof -> "UnexpectedEof"
        is WireException.InvalidUtf8 -> "InvalidUtf8"
        is WireException.InvalidTag -> "InvalidTag"
        is WireException.LengthTooLarge -> "LengthTooLarge"
        is WireException.TrailingBytes -> "TrailingBytes"
        is WireException.BadMagic -> "BadMagic"
        is WireException.UnsupportedVersion -> "UnsupportedVersion"
        is WireException.SchemaMismatch -> "SchemaMismatch"
        is WireException.DuplicateKey -> "DuplicateKey"
        is WireException.NegativeDuration -> "NegativeDuration"
        is WireException.PatchOutOfBounds -> "PatchOutOfBounds"
    }

    private val validEnvelope = Envelope.encode(Envelope.Kind.CALL, 1u, 5uL, bytesOf(1))

    init {
        case("every subclass is a RuntimeException with a descriptive message") {
            val all: List<WireException> = listOf(
                assertWire<WireException.UnexpectedEof> { UndraReader(ByteArray(0)).readU32() },
                assertWire<WireException.InvalidUtf8> { UndraReader(bytesOf(1, 0, 0, 0, 0xFF)).readStr() },
                assertWire<WireException.InvalidTag> { UndraReader(bytesOf(2)).readBool() },
                assertWire<WireException.LengthTooLarge> { UndraReader(bytesOf(9, 0, 0, 0)).readLen() },
                assertWire<WireException.TrailingBytes> { UndraReader(bytesOf(1)).finish() },
                assertWire<WireException.BadMagic> { Envelope.decode(ByteArray(30)) },
                assertWire<WireException.UnsupportedVersion> { Envelope.decode(validEnvelope.copyOf().also { it[4] = 2 }) },
                assertWire<WireException.SchemaMismatch> { Envelope.decode(validEnvelope, 6uL) },
                assertWire<WireException.DuplicateKey> { Codecs.map(Codecs.u8, Codecs.u8).decodeAll(bytesOf(2, 0, 0, 0, 1, 1, 1, 2)) },
                assertWire<WireException.NegativeDuration> { Codecs.duration.decodeAll(ByteArray(8) { 0xFF.toByte() }) },
                assertWire<WireException.PatchOutOfBounds> { KeyedPatch.applyPatch(emptyList<Int>(), listOf(PatchOp.Remove(0u))) },
            )
            assertEq(11, all.map { nameOf(it) }.toSet().size, "one distinct subclass per case")
            for (e in all) {
                assertTrue(RuntimeException::class.java.isInstance(e), "${nameOf(e)} must be unchecked")
                assertTrue(!e.message.isNullOrBlank(), "${nameOf(e)} has no message")
            }
        }

        case("UnexpectedEof: every fixed-width read on too little input") {
            val reads: List<Pair<Int, (UndraReader) -> Any>> = listOf(
                1 to { r: UndraReader -> r.readU8() }, 1 to { r: UndraReader -> r.readI8() }, 1 to { r: UndraReader -> r.readBool() },
                2 to { r: UndraReader -> r.readU16() }, 2 to { r: UndraReader -> r.readI16() },
                4 to { r: UndraReader -> r.readU32() }, 4 to { r: UndraReader -> r.readI32() }, 4 to { r: UndraReader -> r.readF32() },
                8 to { r: UndraReader -> r.readU64() }, 8 to { r: UndraReader -> r.readI64() }, 8 to { r: UndraReader -> r.readF64() },
            )
            for ((width, read) in reads) {
                for (have in 0 until width) {
                    val e = assertWire<WireException.UnexpectedEof>("$width-byte read with $have bytes") { read(UndraReader(ByteArray(have))) }
                    assertEq(width, e.needed)
                    assertEq(0, e.at)
                }
            }
        }

        case("UnexpectedEof: every codec on empty input") {
            val codecs: List<Pair<String, UndraCodec<*>>> = listOf(
                "bool" to Codecs.bool, "u8" to Codecs.u8, "i8" to Codecs.i8, "u16" to Codecs.u16, "i16" to Codecs.i16,
                "u32" to Codecs.u32, "i32" to Codecs.i32, "u64" to Codecs.u64, "i64" to Codecs.i64, "f32" to Codecs.f32,
                "f64" to Codecs.f64, "string" to Codecs.string, "bytes" to Codecs.bytes, "duration" to Codecs.duration,
                "timestamp" to Codecs.timestamp, "uuid" to Codecs.uuid, "handle" to Codecs.handle,
                "option" to Codecs.option(Codecs.i32), "vec" to Codecs.vec(Codecs.i32), "map" to Codecs.map(Codecs.i32, Codecs.i32),
                "result" to Codecs.result(Codecs.i32, Codecs.i32), "Todo" to Todo, "Filter" to Filter, "Shape" to Shape,
            )
            for ((name, codec) in codecs) {
                val e = assertWire<WireException.UnexpectedEof>(name) { codec.decodeAll(ByteArray(0)) }
                assertEq(0, e.at, "$name: offset")
                assertTrue(e.needed >= 1, "$name: needed")
            }
        }

        case("UnexpectedEof: the offset is where the failing read started") {
            // count 1 fits the 7 bytes that follow, but the i64 item needs 8.
            val e = assertWire<WireException.UnexpectedEof> { Codecs.vec(Codecs.i64).decodeAll(bytesOf(1, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7)) }
            assertEq(8, e.needed)
            assertEq(4, e.at)
            // 16 bytes of uuid, then only 2 of the title's 4-byte length prefix.
            val nested = assertWire<WireException.UnexpectedEof> { Todo.decodeAll(ByteArray(16) + bytesOf(0, 0)) }
            assertEq(4, nested.needed)
            assertEq(16, nested.at)
        }

        case("InvalidUtf8: strings in every position of every container") {
            val bad = bytesOf(1, 0, 0, 0, 0xFF)
            assertEq(4, assertWire<WireException.InvalidUtf8> { Codecs.string.decodeAll(bad) }.at)
            assertEq(8, assertWire<WireException.InvalidUtf8> { Codecs.vec(Codecs.string).decodeAll(bytesOf(1, 0, 0, 0) + bad) }.at)
            assertEq(5, assertWire<WireException.InvalidUtf8> { Codecs.option(Codecs.string).decodeAll(bytesOf(1) + bad) }.at)
            assertWire<WireException.InvalidUtf8> { Codecs.map(Codecs.string, Codecs.u8).decodeAll(bytesOf(1, 0, 0, 0) + bad + bytesOf(0)) }
            assertWire<WireException.InvalidUtf8> { Codecs.result(Codecs.u8, Codecs.string).decodeAll(bytesOf(1) + bad) }
            assertWire<WireException.InvalidUtf8> { Payloads.Hello.decode(bad) }
            assertWire<WireException.InvalidUtf8> { Payloads.Log.decode(bytesOf(1) + bad) }
            assertWire<WireException.InvalidUtf8> { Payloads.Reply(1u, Payloads.ReplyStatus.BAD_REQUEST, bad).readBadRequestReason() }
        }

        case("InvalidTag: every tag-bearing type names itself and the offset of the bad byte") {
            fun tag(type: String, at: Int, tag: Int, block: () -> Unit) {
                val e = assertWire<WireException.InvalidTag>(type, block)
                assertEq(type, e.type)
                assertEq(at, e.at, "$type offset")
                assertEq(tag.toUInt(), e.tag, "$type tag")
            }
            tag("bool", 0, 2) { Codecs.bool.decodeAll(bytesOf(2)) }
            tag("Option", 0, 2) { Codecs.option(Codecs.u8).decodeAll(bytesOf(2, 0)) }
            tag("Result", 0, 2) { Codecs.result(Codecs.u8, Codecs.u8).decodeAll(bytesOf(2, 0)) }
            tag("Filter", 0, 3) { Filter.decodeAll(bytesOf(3, 0)) }
            tag("Shape", 0, 2) { Shape.decodeAll(bytesOf(2, 0)) }
            tag("Envelope.Kind", 14, 0) { Envelope.decode(validEnvelope.copyOf().also { it[14] = 0 }) }
            tag("ReplyStatus", 4, 6) { Payloads.Reply.decode(bytesOf(0, 0, 0, 0, 6)) }
            tag("PortStatus", 4, 3) { Payloads.PortReply.decode(bytesOf(0, 0, 0, 0, 3)) }
            tag("StreamFlag", 4, 3) { Payloads.StreamItem.decode(bytesOf(0, 0, 0, 0, 3)) }
            tag("ChangeOp", 24, 3) { Payloads.ChangeSet.decode(unhex("00".repeat(8) + "01000000" + "00".repeat(12) + "03" + "00000000")) }
            tag("CallTarget", 0, 4) { Payloads.Call.decode(bytesOf(4)) }
            tag("PatchOp", 4, 5) { KeyedPatch.decodePatch(bytesOf(1, 0, 0, 0, 5), Codecs.u8) }
            tag("bool", 12, 2) { Payloads.Observe.decode(unhex("00".repeat(12) + "02")) }
        }

        case("LengthTooLarge: every length-prefixed shape, at the prefix's offset") {
            val bomb = bytesOf(0xFF, 0xFF, 0xFF, 0xFF)
            val codecs: List<Pair<String, UndraCodec<*>>> = listOf(
                "string" to Codecs.string, "bytes" to Codecs.bytes, "vec" to Codecs.vec(Codecs.u8), "vec<vec>" to Codecs.vec(Codecs.vec(Codecs.u8)),
                "map" to Codecs.map(Codecs.u8, Codecs.u8),
            )
            for ((name, codec) in codecs) {
                val e = assertWire<WireException.LengthTooLarge>(name) { codec.decodeAll(bomb) }
                assertEq(UInt.MAX_VALUE, e.len, "$name: len")
                assertEq(0, e.at, "$name: at")
                // One more than the input holds is refused too.
                assertWire<WireException.LengthTooLarge>("$name (off by one)") { codec.decodeAll(bytesOf(3, 0, 0, 0, 1, 2)) }
            }
            assertEq(1, assertWire<WireException.LengthTooLarge> { Codecs.option(Codecs.string).decodeAll(bytesOf(1, 9, 0, 0, 0)) }.at, "after the option tag")
            assertWire<WireException.LengthTooLarge> { Payloads.Snapshot.decode(bomb) }
            assertWire<WireException.LengthTooLarge> { KeyedPatch.decodePatch(bomb, Codecs.u8) }
            assertWire<WireException.LengthTooLarge> { Envelope.decode(validEnvelope.copyOf().also { it[19] = 9 }) }
        }

        case("TrailingBytes: finish, decodeAll, every fixed-shape payload and the envelope") {
            assertEq(2, assertWire<WireException.TrailingBytes> { UndraReader(bytesOf(1, 2)).finish() }.count)
            assertEq(1, assertWire<WireException.TrailingBytes> { Codecs.string.decodeAll(bytesOf(0, 0, 0, 0, 9)) }.count)
            assertEq(1, assertWire<WireException.TrailingBytes> { Envelope.decode(validEnvelope + 0) }.count)
            assertWire<WireException.TrailingBytes> { Payloads.Cancel.decode(bytesOf(1, 0, 0, 0, 0)) }
            assertWire<WireException.TrailingBytes> { Payloads.StreamCredit.decode(ByteArray(9)) }
            assertWire<WireException.TrailingBytes> { Payloads.Observe.decode(ByteArray(14)) }
            assertWire<WireException.TrailingBytes> { Payloads.Release.decode(ByteArray(9)) }
            assertWire<WireException.TrailingBytes> { Payloads.TimerFired.decode(ByteArray(5)) }
            assertWire<WireException.TrailingBytes> { Payloads.Hello.decode(Payloads.Hello("v", 1uL, "p", "m").toByteArray() + 0) }
            assertWire<WireException.TrailingBytes> { Payloads.Log.decode(Payloads.Log(1u, "t", "m").toByteArray() + 0) }
            assertWire<WireException.TrailingBytes> { Payloads.Snapshot.decode(bytesOf(0, 0, 0, 0, 0, 0, 0, 0, 1)) }
            assertWire<WireException.TrailingBytes> { Payloads.ChangeSet.decode(Payloads.ChangeSet(1u, emptyList()).toByteArray() + 0) }
            assertWire<WireException.TrailingBytes> { KeyedPatch.decodePatch(bytesOf(0, 0, 0, 0, 1), Codecs.u8) }
        }

        case("BadMagic: reports the four bytes found") {
            assertEq("00000000", assertWire<WireException.BadMagic> { Envelope.decode(ByteArray(23)) }.found)
            assertEq("554e4458", assertWire<WireException.BadMagic> { Envelope.decode(validEnvelope.copyOf().also { it[3] = 'X'.code.toByte() }) }.found)
            assertEq("deadbeef", assertWire<WireException.BadMagic> { Envelope.decode(bytesOf(0xDE, 0xAD, 0xBE, 0xEF) + ByteArray(30)) }.found)
        }

        case("UnsupportedVersion: reports the number found") {
            assertEq(0u.toUShort(), assertWire<WireException.UnsupportedVersion> { Envelope.decode(validEnvelope.copyOf().also { it[4] = 0 }) }.version)
            assertEq(2u.toUShort(), assertWire<WireException.UnsupportedVersion> { Envelope.decode(validEnvelope.copyOf().also { it[4] = 2 }) }.version)
            assertEq(0xFFFFu.toUShort(), assertWire<WireException.UnsupportedVersion> { Envelope.decode(validEnvelope.copyOf().also { it[4] = -1; it[5] = -1 }) }.version)
        }

        case("SchemaMismatch: reports both hashes") {
            val e = assertWire<WireException.SchemaMismatch> { Envelope.decode(validEnvelope, expectedSchema = 99uL) }
            assertEq(99uL, e.expected)
            assertEq(5uL, e.got)
            assertTrue(e.message!!.contains("63") && e.message!!.contains("5"), "message shows both hashes in hex: ${e.message}")
        }

        case("DuplicateKey: on decode at the repeated key, on encode where the entries begin") {
            val dec = assertWire<WireException.DuplicateKey> { Codecs.map(Codecs.u8, Codecs.u8).decodeAll(bytesOf(3, 0, 0, 0, 1, 1, 2, 2, 1, 3)) }
            assertEq(8, dec.at)
            val twins = linkedMapOf(bytesOf(7) to 1.toUByte(), bytesOf(7) to 2.toUByte())
            val enc = assertWire<WireException.DuplicateKey> { Codecs.map(Codecs.bytes, Codecs.u8).encodeToByteArray(twins) }
            assertEq(4, enc.at)
            // The same uuid twice: count 2, then (uuid, 1) and (uuid, 2).
            val id = Codecs.uuid.encodeToByteArray(UUID(1, 2))
            val twice = bytesOf(2, 0, 0, 0) + id + bytesOf(1) + id + bytesOf(2)
            assertEq(4 + 17, assertWire<WireException.DuplicateKey> { Codecs.map(Codecs.uuid, Codecs.u8).decodeAll(twice) }.at)
        }

        case("NegativeDuration: on decode and on encode") {
            val dec = assertWire<WireException.NegativeDuration> { Codecs.duration.decodeAll(ByteArray(8) { 0xFF.toByte() }) }
            assertEq(-1L, dec.nanos)
            assertEq(0, dec.at)
            val enc = assertWire<WireException.NegativeDuration> { Codecs.duration.encodeToByteArray((-3).seconds) }
            assertEq(-3_000_000_000L, enc.nanos)
            assertEq(0, enc.at)
        }

        case("PatchOutOfBounds: reports the op that failed") {
            val e = assertWire<WireException.PatchOutOfBounds> {
                KeyedPatch.applyPatch(listOf(1), listOf(PatchOp.Insert(1u, 2), PatchOp.Update(2u, 3)))
            }
            assertEq(1, e.opIndex)
            assertEq("Update", e.op)
            assertEq(2u, e.index)
            assertEq(2, e.size)
        }

        case("after a failure the reader's position stays inside its window") {
            val r = UndraReader(bytesOf(1, 0, 0, 0, 0xFF, 7))
            assertWire<WireException.InvalidUtf8> { r.readStr() }
            assertTrue(r.position in 0..6 && r.remaining >= 0, "position ${r.position} left the window")
        }
    }

    @Test
    fun allCases() = assertPassed()
}
