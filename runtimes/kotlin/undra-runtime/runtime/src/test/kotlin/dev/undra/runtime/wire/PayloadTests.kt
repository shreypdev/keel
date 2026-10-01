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
import dev.undra.runtime.testing.unhex
import org.junit.jupiter.api.Test

/** Exact encodings, round trips and malformed-input handling for every payload in [Payloads]. */
class PayloadTests : Suite() {

    /**
     * Encodes [payload] and expects [expectedHex]; decodes it back through the byte-array entry point and
     * through readers over an array window and a direct buffer.
     *
     * For payloads that end in an unprefixed body ([openEnded]) trailing bytes are part of the body, so
     * only prefixes shorter than [headerLen] must fail; for the others every strict prefix must fail and
     * one extra byte must be TrailingBytes.
     */
    private fun <P : Payloads.Payload> check(
        expectedHex: String,
        payload: P,
        decodeBytes: (ByteArray) -> P,
        decodeReader: (UndraReader) -> P,
        openEnded: Boolean = false,
        headerLen: Int = 0,
    ) {
        assertBytes(expectedHex, payload.toByteArray(), "encoding")
        val canonical = unhex(expectedHex)
        assertEq(payload, decodeBytes(canonical), "decode(bytes)")
        val direct = UndraReader(directBuffer(canonical))
        assertEq(payload, decodeReader(direct), "decode(direct buffer)")
        direct.finish()
        val padded = ByteArray(canonical.size + 6) { 0x44 }
        System.arraycopy(canonical, 0, padded, 3, canonical.size)
        val window = UndraReader(padded, 3, canonical.size)
        assertEq(payload, decodeReader(window), "decode(window)")
        window.finish()
        assertEq(payload.hashCode(), decodeBytes(canonical).hashCode(), "hashCode of equal payloads")
        val w = UndraWriter()
        w.writeU8(0x99u)
        payload.encode(w)
        assertBytes("99$expectedHex", w.toByteArray(), "encode(w) appends")
        if (!openEnded) {
            assertWire<WireException.TrailingBytes>("trailing byte") { decodeBytes(canonical + 0) }
        }
        val shortest = if (openEnded) headerLen else canonical.size
        for (n in 0 until shortest) {
            assertWire<WireException>("prefix of $n bytes") { decodeBytes(canonical.copyOf(n)) }
        }
    }

    private val method = Payloads.CallTarget.ObjectMethod(Handle(4294967297), 2353348832u)

    init {
        // ---- Call -----------------------------------------------------------------------------------------

        case("Call to a free function writes a zero handle slot") {
            val call = Payloads.Call(Payloads.CallTarget.FreeFunction(0x11223344u), 9u, bytesOf(1, 2))
            check("00" + "0000000000000000" + "44332211" + "09000000" + "0102", call,
                { Payloads.Call.decode(it) }, { Payloads.Call.decode(it) }, openEnded = true, headerLen = 17)
        }

        case("Call to an object method carries handle, method id, call id and args") {
            val call = Payloads.Call(method, 9u, bytesOf(2, 0, 0, 0, 3, 0, 0, 0))
            check("01" + "0100000001000000" + "e040458c" + "09000000" + "0200000003000000", call,
                { Payloads.Call.decode(it) }, { Payloads.Call.decode(it) }, openEnded = true, headerLen = 17)
        }

        case("Call to a constructor has a type id and no handle") {
            val call = Payloads.Call(Payloads.CallTarget.Constructor(0xAABBCCDDu, 1u), 2u, bytesOf(7))
            check("02" + "ddccbbaa" + "01000000" + "02000000" + "07", call,
                { Payloads.Call.decode(it) }, { Payloads.Call.decode(it) }, openEnded = true, headerLen = 13)
        }

        case("Call for a lazy-list page has offset and limit and no args") {
            val call = Payloads.Call(Payloads.CallTarget.LazyListPage(Handle.make(2u, 3u), 10u, 20u), 5u, ByteArray(0))
            check("03" + "0200000003000000" + "0a000000" + "14000000" + "05000000", call,
                { Payloads.Call.decode(it) }, { Payloads.Call.decode(it) })
        }

        case("Call with empty args, and extremes of every id") {
            val a = Payloads.Call(Payloads.CallTarget.FreeFunction(UInt.MAX_VALUE), UInt.MAX_VALUE, ByteArray(0))
            check("00" + "0000000000000000" + "ffffffff" + "ffffffff", a,
                { Payloads.Call.decode(it) }, { Payloads.Call.decode(it) }, openEnded = true, headerLen = 17)
            val b = Payloads.Call(Payloads.CallTarget.ObjectMethod(Handle(-1L), 0u), 0u, ByteArray(0))
            check("01" + "ffffffffffffffff" + "00000000" + "00000000", b,
                { Payloads.Call.decode(it) }, { Payloads.Call.decode(it) }, openEnded = true, headerLen = 17)
        }

        case("Call decoding ignores the handle slot of a free-function call") {
            val decoded = Payloads.Call.decode(unhex("00" + "ffffffffffffffff" + "01000000" + "02000000"))
            assertEq(Payloads.Call(Payloads.CallTarget.FreeFunction(1u), 2u, ByteArray(0)), decoded)
        }

        case("Call rejects unknown targets and cannot be built with args for a lazy-list page") {
            for (t in listOf(4, 5, 128, 255)) {
                val e = assertWire<WireException.InvalidTag>("target $t") { Payloads.Call.decode(bytesOf(t, 0, 0, 0, 0)) }
                assertEq(t.toUInt(), e.tag)
                assertEq(0, e.at)
                assertEq("CallTarget", e.type)
            }
            assertThrows<IllegalArgumentException> {
                Payloads.Call(Payloads.CallTarget.LazyListPage(Handle(1), 0u, 1u), 1u, bytesOf(1))
            }
            assertEq(1, assertWire<WireException.TrailingBytes> { Payloads.Call.decode(unhex("03" + "0100000000000000" + "00000000" + "01000000" + "05000000" + "ff")) }.count)
        }

        case("Call equality is by content, including args") {
            val a = Payloads.Call(method, 1u, bytesOf(1, 2))
            assertEq(a, Payloads.Call(method, 1u, bytesOf(1, 2)))
            assertTrue(a != Payloads.Call(method, 1u, bytesOf(1, 3)))
            assertTrue(a != Payloads.Call(method, 2u, bytesOf(1, 2)))
            assertTrue(a != Payloads.Call(Payloads.CallTarget.ObjectMethod(Handle(1), 2353348832u), 1u, bytesOf(1, 2)))
            assertTrue(a.toString().contains("args=2B"))
        }

        // ---- Reply ----------------------------------------------------------------------------------------

        case("Reply is call id, status and body") {
            val ok = Payloads.Reply(9u, Payloads.ReplyStatus.OK, bytesOf(5, 0, 0, 0))
            check("09000000" + "00" + "05000000", ok, { Payloads.Reply.decode(it) }, { Payloads.Reply.decode(it) }, openEnded = true, headerLen = 5)
            for (s in Payloads.ReplyStatus.entries) {
                val r = Payloads.Reply(0xFFFFFFFFu, s, ByteArray(0))
                check("ffffffff" + "%02x".format(s.code.toInt()), r, { Payloads.Reply.decode(it) }, { Payloads.Reply.decode(it) }, openEnded = true, headerLen = 5)
            }
        }

        case("ReplyStatus has the six codes of the spec, and ordinal equals code") {
            assertEq(
                listOf("OK" to 0, "ERROR" to 1, "PANIC" to 2, "CANCELLED" to 3, "STREAM_OPENED" to 4, "BAD_REQUEST" to 5),
                Payloads.ReplyStatus.entries.map { it.name to it.code.toInt() },
            )
            for (s in Payloads.ReplyStatus.entries) assertEq(s, Payloads.ReplyStatus.fromByte(s.code))
        }

        case("Reply rejects unknown statuses") {
            for (s in listOf(6, 7, 255)) {
                val e = assertWire<WireException.InvalidTag>("status $s") { Payloads.Reply.decode(bytesOf(1, 0, 0, 0, s)) }
                assertEq(s.toUInt(), e.tag)
                assertEq(4, e.at)
                assertEq("ReplyStatus", e.type)
            }
            assertEq(-1, assertWire<WireException.InvalidTag> { Payloads.ReplyStatus.fromByte(9u) }.at)
        }

        case("a panic reply carries message and backtrace") {
            val body = UndraWriter().also { it.writeStr("index out of bounds"); it.writeStr("frame 0\nframe 1") }.toByteArray()
            val reply = Payloads.Reply.decode(Payloads.Reply(3u, Payloads.ReplyStatus.PANIC, body).toByteArray())
            assertEq(Payloads.PanicInfo("index out of bounds", "frame 0\nframe 1"), reply.readPanic())
            assertWire<WireException.TrailingBytes> { Payloads.Reply(3u, Payloads.ReplyStatus.PANIC, body + 0).readPanic() }
            assertWire<WireException.UnexpectedEof> { Payloads.Reply(3u, Payloads.ReplyStatus.PANIC, ByteArray(0)).readPanic() }
            assertWire<WireException.LengthTooLarge> { Payloads.Reply(3u, Payloads.ReplyStatus.PANIC, bytesOf(9, 0, 0, 0, 1)).readPanic() }
        }

        case("a bad-request reply carries a reason") {
            val body = UndraWriter().also { it.writeStr("unknown method " + cp(0x1F4A5)) }.toByteArray()
            assertEq("unknown method " + cp(0x1F4A5), Payloads.Reply(4u, Payloads.ReplyStatus.BAD_REQUEST, body).readBadRequestReason())
            assertWire<WireException.TrailingBytes> { Payloads.Reply(4u, Payloads.ReplyStatus.BAD_REQUEST, body + 1).readBadRequestReason() }
            assertWire<WireException.UnexpectedEof> { Payloads.Reply(4u, Payloads.ReplyStatus.BAD_REQUEST, ByteArray(0)).readBadRequestReason() }
        }

        case("Reply.bodyReader decodes a return value with the method's codec") {
            val body = Codecs.vec(Codecs.string).encodeToByteArray(listOf("a", "b"))
            val reply = Payloads.Reply.decode(Payloads.Reply(1u, Payloads.ReplyStatus.OK, body).toByteArray())
            assertEq(listOf("a", "b"), Codecs.vec(Codecs.string).decode(reply.bodyReader()))
        }

        // ---- ChangeSet --------------------------------------------------------------------------------------

        case("ChangeSet is txn id, count and length-prefixed entries") {
            val cs = Payloads.ChangeSet(
                42u,
                listOf(Payloads.ChangeEntry(Handle(4294967297), 0u, Payloads.ChangeOp.FULL, bytesOf(1, 0, 0, 0))),
            )
            check("2a00000000000000" + "01000000" + "0100000001000000" + "00000000" + "00" + "04000000" + "01000000", cs,
                { Payloads.ChangeSet.decode(it) }, { Payloads.ChangeSet.decode(it) })
        }

        case("ChangeSet with several entries, every op, empty values and the all-signals id") {
            val cs = Payloads.ChangeSet(
                ULong.MAX_VALUE,
                listOf(
                    Payloads.ChangeEntry(Handle.make(1u, 1u), 0u, Payloads.ChangeOp.FULL, bytesOf(9)),
                    Payloads.ChangeEntry(Handle.make(2u, 1u), 3u, Payloads.ChangeOp.PATCH, bytesOf(0, 0, 0, 0)),
                    Payloads.ChangeEntry(Handle.make(3u, 7u), UInt.MAX_VALUE, Payloads.ChangeOp.INVALIDATED, ByteArray(0)),
                ),
            )
            val expected = "ffffffffffffffff" + "03000000" +
                "0100000001000000" + "00000000" + "00" + "01000000" + "09" +
                "0200000001000000" + "03000000" + "01" + "04000000" + "00000000" +
                "0300000007000000" + "ffffffff" + "02" + "00000000"
            check(expected, cs, { Payloads.ChangeSet.decode(it) }, { Payloads.ChangeSet.decode(it) })
        }

        case("an empty ChangeSet is just the txn id and a zero count") {
            check("0500000000000000" + "00000000", Payloads.ChangeSet(5u, emptyList()),
                { Payloads.ChangeSet.decode(it) }, { Payloads.ChangeSet.decode(it) })
        }

        case("ChangeSet decoding rejects unknown ops, oversized counts and oversized entries") {
            val head = "2a00000000000000" + "01000000" + "0100000001000000" + "00000000"
            val e = assertWire<WireException.InvalidTag> { Payloads.ChangeSet.decode(unhex(head + "03" + "00000000")) }
            assertEq(3u, e.tag)
            assertEq(24, e.at)
            assertEq("ChangeOp", e.type)
            assertEq(UInt.MAX_VALUE, assertWire<WireException.LengthTooLarge> { Payloads.ChangeSet.decode(unhex("2a00000000000000" + "ffffffff")) }.len)
            // A count of 2 needs at least 34 bytes; with 17 it must be refused before decoding entries.
            val twoEntries = assertWire<WireException.LengthTooLarge> {
                Payloads.ChangeSet.decode(unhex("2a00000000000000" + "02000000" + "0100000001000000" + "00000000" + "00" + "00000000"))
            }
            assertEq(2u, twoEntries.len)
            assertWire<WireException.LengthTooLarge> { Payloads.ChangeSet.decode(unhex(head + "00" + "05000000" + "0102")) }
        }

        // ---- Ports ------------------------------------------------------------------------------------------

        case("PortCall is port id, method id, port call id and args") {
            check("10000000" + "20000000" + "03000000" + "09", Payloads.PortCall(0x10u, 0x20u, 3u, bytesOf(9)),
                { Payloads.PortCall.decode(it) }, { Payloads.PortCall.decode(it) }, openEnded = true, headerLen = 12)
            check("ffffffff" + "ffffffff" + "ffffffff", Payloads.PortCall(UInt.MAX_VALUE, UInt.MAX_VALUE, UInt.MAX_VALUE, ByteArray(0)),
                { Payloads.PortCall.decode(it) }, { Payloads.PortCall.decode(it) }, openEnded = true, headerLen = 12)
        }

        case("PortReply is port call id, status and body") {
            check("03000000" + "00" + "01", Payloads.PortReply(3u, Payloads.PortStatus.OK, bytesOf(1)),
                { Payloads.PortReply.decode(it) }, { Payloads.PortReply.decode(it) }, openEnded = true, headerLen = 5)
            check("03000000" + "01" + "0100000078", Payloads.PortReply(3u, Payloads.PortStatus.ERROR, unhex("0100000078")),
                { Payloads.PortReply.decode(it) }, { Payloads.PortReply.decode(it) }, openEnded = true, headerLen = 5)
            check("03000000" + "02", Payloads.PortReply(3u, Payloads.PortStatus.UNAVAILABLE, ByteArray(0)),
                { Payloads.PortReply.decode(it) }, { Payloads.PortReply.decode(it) }, openEnded = true, headerLen = 5)
        }

        case("PortStatus has the three codes of the spec and rejects the rest") {
            assertEq(listOf("OK" to 0, "ERROR" to 1, "UNAVAILABLE" to 2), Payloads.PortStatus.entries.map { it.name to it.code.toInt() })
            for (s in Payloads.PortStatus.entries) assertEq(s, Payloads.PortStatus.fromByte(s.code))
            val e = assertWire<WireException.InvalidTag> { Payloads.PortReply.decode(bytesOf(1, 0, 0, 0, 3)) }
            assertEq(3u, e.tag)
            assertEq(4, e.at)
            assertEq("PortStatus", e.type)
        }

        // ---- Small fixed payloads ---------------------------------------------------------------------------

        case("Cancel is a call id") {
            check("07000000", Payloads.Cancel(7u), { Payloads.Cancel.decode(it) }, { Payloads.Cancel.decode(it) })
            check("ffffffff", Payloads.Cancel(UInt.MAX_VALUE), { Payloads.Cancel.decode(it) }, { Payloads.Cancel.decode(it) })
        }

        case("StreamCredit is call id and credit") {
            check("07000000" + "10000000", Payloads.StreamCredit(7u, 16u), { Payloads.StreamCredit.decode(it) }, { Payloads.StreamCredit.decode(it) })
            check("00000000" + "ffffffff", Payloads.StreamCredit(0u, UInt.MAX_VALUE), { Payloads.StreamCredit.decode(it) }, { Payloads.StreamCredit.decode(it) })
        }

        case("StreamItem is call id, flag and body") {
            check("07000000" + "00" + "01020304", Payloads.StreamItem(7u, Payloads.StreamFlag.ITEM, bytesOf(1, 2, 3, 4)),
                { Payloads.StreamItem.decode(it) }, { Payloads.StreamItem.decode(it) }, openEnded = true, headerLen = 5)
            check("07000000" + "01", Payloads.StreamItem(7u, Payloads.StreamFlag.END, ByteArray(0)),
                { Payloads.StreamItem.decode(it) }, { Payloads.StreamItem.decode(it) }, openEnded = true, headerLen = 5)
            check("07000000" + "02" + "0100000078", Payloads.StreamItem(7u, Payloads.StreamFlag.ERROR, unhex("0100000078")),
                { Payloads.StreamItem.decode(it) }, { Payloads.StreamItem.decode(it) }, openEnded = true, headerLen = 5)
        }

        case("StreamFlag has the three codes of the spec and rejects the rest") {
            assertEq(listOf("ITEM" to 0, "END" to 1, "ERROR" to 2), Payloads.StreamFlag.entries.map { it.name to it.code.toInt() })
            for (f in Payloads.StreamFlag.entries) assertEq(f, Payloads.StreamFlag.fromByte(f.code))
            val e = assertWire<WireException.InvalidTag> { Payloads.StreamItem.decode(bytesOf(1, 0, 0, 0, 3)) }
            assertEq(3u, e.tag)
            assertEq(4, e.at)
            assertEq("StreamFlag", e.type)
        }

        case("Observe is handle, signal id and an on flag") {
            check("0100000001000000" + "00000000" + "01", Payloads.Observe(Handle(4294967297), 0u, true),
                { Payloads.Observe.decode(it) }, { Payloads.Observe.decode(it) })
            check("0100000001000000" + "ffffffff" + "00", Payloads.Observe(Handle(4294967297), UInt.MAX_VALUE, false),
                { Payloads.Observe.decode(it) }, { Payloads.Observe.decode(it) })
            val e = assertWire<WireException.InvalidTag> { Payloads.Observe.decode(unhex("0100000001000000" + "00000000" + "02")) }
            assertEq(2u, e.tag)
            assertEq(12, e.at)
            assertEq("bool", e.type)
        }

        case("Release is a handle") {
            check("0100000001000000", Payloads.Release(Handle(4294967297)), { Payloads.Release.decode(it) }, { Payloads.Release.decode(it) })
            check("ffffffffffffffff", Payloads.Release(Handle(-1)), { Payloads.Release.decode(it) }, { Payloads.Release.decode(it) })
        }

        case("Event is port id, method id and a payload") {
            check("01000000" + "02000000" + "03", Payloads.Event(1u, 2u, bytesOf(3)),
                { Payloads.Event.decode(it) }, { Payloads.Event.decode(it) }, openEnded = true, headerLen = 8)
            check("01000000" + "02000000", Payloads.Event(1u, 2u, ByteArray(0)),
                { Payloads.Event.decode(it) }, { Payloads.Event.decode(it) }, openEnded = true, headerLen = 8)
        }

        case("Hello is version, schema hash, platform and mode") {
            val hello = Payloads.Hello("0.1.0", 0x0102030405060708uL, "android", "inproc")
            check("05000000" + "302e312e30" + "0807060504030201" + "07000000" + "616e64726f6964" + "06000000" + "696e70726f63", hello,
                { Payloads.Hello.decode(it) }, { Payloads.Hello.decode(it) })
            val unicode = Payloads.Hello("v" + cp(0xE9), ULong.MAX_VALUE, cp(0x1F4F1), "")
            check(hex(unicode.toByteArray()), unicode, { Payloads.Hello.decode(it) }, { Payloads.Hello.decode(it) })
        }

        case("Log is level, target and message") {
            check("02" + "09000000" + "636f72653a3a74786e" + "06000000" + "636f6d6d6974", Payloads.Log(2u, "core::txn", "commit"),
                { Payloads.Log.decode(it) }, { Payloads.Log.decode(it) })
            val unicode = Payloads.Log(255u, "", "multi\nline " + cp(0x1F600))
            check(hex(unicode.toByteArray()), unicode, { Payloads.Log.decode(it) }, { Payloads.Log.decode(it) })
        }

        case("TimerFired is a timer id") {
            check("efbeadde", Payloads.TimerFired(0xDEADBEEFu), { Payloads.TimerFired.decode(it) }, { Payloads.TimerFired.decode(it) })
            check("00000000", Payloads.TimerFired(0u), { Payloads.TimerFired.decode(it) }, { Payloads.TimerFired.decode(it) })
        }

        // ---- Snapshot ---------------------------------------------------------------------------------------

        case("Snapshot is a count of stores and a generation floor, then each store with its persisted signals") {
            val snapshot = Payloads.Snapshot(
                0x01020304u,
                listOf(
                    Payloads.Snapshot.Store(
                        Handle.make(1u, 1u),
                        0x1234u,
                        listOf(Payloads.Snapshot.Signal(0u, bytesOf(1, 2, 3)), Payloads.Snapshot.Signal(1u, ByteArray(0))),
                    ),
                ),
            )
            check(
                "01000000" + "04030201" + "0100000001000000" + "34120000" + "02000000" + "00000000" + "03000000" + "010203" + "01000000" + "00000000",
                snapshot, { Payloads.Snapshot.decode(it) }, { Payloads.Snapshot.decode(it) },
            )
        }

        case("Snapshot edge cases: no stores, a store with no signals, several stores") {
            check("00000000" + "00000000", Payloads.Snapshot(0u, emptyList()), { Payloads.Snapshot.decode(it) }, { Payloads.Snapshot.decode(it) })
            val bare = Payloads.Snapshot(7u, listOf(Payloads.Snapshot.Store(Handle(5), 9u, emptyList())))
            check("01000000" + "07000000" + "0500000000000000" + "09000000" + "00000000", bare, { Payloads.Snapshot.decode(it) }, { Payloads.Snapshot.decode(it) })
            val many = Payloads.Snapshot(
                20u,
                List(20) { i ->
                    Payloads.Snapshot.Store(
                        Handle.make(i.toUInt(), 1u),
                        i.toUInt() * 1000u,
                        List(i % 4) { j -> Payloads.Snapshot.Signal(j.toUInt(), ByteArray(i + j) { (it + i).toByte() }) },
                    )
                },
            )
            check(hex(many.toByteArray()), many, { Payloads.Snapshot.decode(it) }, { Payloads.Snapshot.decode(it) })
        }

        case("Snapshot decoding rejects impossible counts before allocating") {
            assertEq(UInt.MAX_VALUE, assertWire<WireException.LengthTooLarge> { Payloads.Snapshot.decode(unhex("ffffffff" + "00000000")) }.len)
            // One store needs at least 16 bytes (the generation floor is not one of them).
            assertEq(1u, assertWire<WireException.LengthTooLarge> { Payloads.Snapshot.decode(unhex("01000000" + "00000000" + "00".repeat(11))) }.len)
            // A store claiming 2^32-1 signals with nothing behind it.
            val hostile = "01000000" + "00000000" + "0100000001000000" + "01000000" + "ffffffff"
            assertEq(UInt.MAX_VALUE, assertWire<WireException.LengthTooLarge> { Payloads.Snapshot.decode(unhex(hostile)) }.len)
        }

        case("every payload's decode(bytes) rejects the empty input") {
            assertWire<WireException.UnexpectedEof> { Payloads.Call.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.Reply.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.ChangeSet.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.PortCall.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.PortReply.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.Cancel.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.StreamCredit.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.StreamItem.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.Observe.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.Release.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.Event.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.Hello.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.Log.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.TimerFired.decode(ByteArray(0)) }
            assertWire<WireException.UnexpectedEof> { Payloads.Snapshot.decode(ByteArray(0)) }
            // The layout before the generation floor (`count u32` only) ends where the floor should be.
            assertWire<WireException.UnexpectedEof> { Payloads.Snapshot.decode(unhex("00000000")) }
        }

        case("payloads travel inside an envelope of the matching kind") {
            val cancel = Payloads.Cancel(77u)
            val frame = Envelope.encode(Envelope.Kind.CANCEL, 1u, 9uL, cancel.toByteArray())
            val env = Envelope.decode(frame)
            assertEq(Envelope.Kind.CANCEL, env.kind)
            assertEq(cancel, Payloads.Cancel.decode(env.payload))
            val hello = Payloads.Hello("0.1.0", 9uL, "jvm", "dev")
            val helloEnv = Envelope.decode(Envelope.encode(Envelope.Kind.HELLO, 0u, 9uL, hello.toByteArray()))
            assertEq(hello, Payloads.Hello.decode(helloEnv.payload))
        }
    }

    @Test
    fun allCases() = assertPassed()
}
