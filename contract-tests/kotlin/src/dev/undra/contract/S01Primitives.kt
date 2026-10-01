package dev.undra.contract

import dev.undra.playground.core.Primitives
import dev.undra.playground.core.echoPrimitives
import dev.undra.playground.core.ping
import dev.undra.runtime.wire.Timestamp
import java.util.UUID
import kotlin.time.Duration.Companion.nanoseconds

/** S01: every primitive the wire has makes the round trip through the Kotlin codec unchanged. */
fun s01Primitives(w: World) {
    // 1. The typical value.
    val typical = Primitives(
        flag = true,
        tiny = -8,
        small = -16_000,
        int = -2_000_000_000,
        long = -9_000_000_000_000_000_000L,
        byte = 255u,
        word = 65_535u,
        dword = 4_000_000_000u,
        qword = 18_000_000_000_000_000_000uL,
        single = 1.5f,
        double = -2.25e100,
        text = "héllo, wörld ✓",
        blob = byteArrayOf(0, 1, 2, 254.toByte(), 255.toByte()),
        span = 1_500_000_123L.nanoseconds,
        at = Timestamp(1_700_000_000_123L),
        id = UUID.fromString("12345678-9abc-def0-0102-030405060708"),
    )
    sameAfterEcho("typical value", typical)

    // 2. The extremes: the smallest of the signed, the largest of the unsigned, -0.0, infinity, nothing at all.
    val lowest = typical.copy(
        flag = false,
        tiny = Byte.MIN_VALUE,
        small = Short.MIN_VALUE,
        int = Int.MIN_VALUE,
        long = Long.MIN_VALUE,
        byte = UByte.MIN_VALUE,
        word = UShort.MIN_VALUE,
        dword = UInt.MIN_VALUE,
        qword = ULong.MAX_VALUE,
        single = -0.0f,
        double = Double.POSITIVE_INFINITY,
        text = "",
        blob = ByteArray(0),
    )
    sameAfterEcho("lowest signed, -0.0, +infinity, empty text and blob", lowest)
    val highest = typical.copy(
        tiny = Byte.MAX_VALUE,
        small = Short.MAX_VALUE,
        int = Int.MAX_VALUE,
        long = Long.MAX_VALUE,
        byte = UByte.MAX_VALUE,
        word = UShort.MAX_VALUE,
        dword = UInt.MAX_VALUE,
        double = Double.NaN,
        text = "ü".repeat(10_000),
        blob = ByteArray(65_536) { (it % 251).toByte() },
    )
    val echoed = echoPrimitives(highest)
    check(echoed.double.isNaN()) { "NaN came back as ${echoed.double}" }
    sameAfterEcho("highest signed, 10,000 characters, 64 KiB", highest.copy(double = 1.0 / 3.0))
    expectEq("the 10,000-character text", highest.text, echoed.text)
    expectEq("the 65,536-byte blob", highest.blob, echoed.blob)

    // 3. A call with no arguments and no result completes.
    ping()

    // 4. Bytes are copied, never aliased: the returned array is another array, and writing to it leaves the sent one alone.
    // (A Kotlin String and a UUID are immutable, so they cannot be aliased by construction.)
    val sent = typical.blob.copyOf()
    val back = echoPrimitives(typical.copy(blob = sent)).blob
    check(back !== sent) { "the returned blob is the very array that was sent" }
    back[0] = 99
    expectEq("the sent blob after writing to the returned one", byteArrayOf(0, 1, 2, 254.toByte(), 255.toByte()), sent)
}

/** Echoes [sent] through the core and compares every field, bit for bit where it is floating point. */
private fun sameAfterEcho(what: String, sent: Primitives) {
    val got = echoPrimitives(sent)
    expectEq("$what: flag", sent.flag, got.flag)
    expectEq("$what: tiny", sent.tiny, got.tiny)
    expectEq("$what: small", sent.small, got.small)
    expectEq("$what: int", sent.int, got.int)
    expectEq("$what: long", sent.long, got.long)
    expectEq("$what: byte", sent.byte, got.byte)
    expectEq("$what: word", sent.word, got.word)
    expectEq("$what: dword", sent.dword, got.dword)
    expectEq("$what: qword", sent.qword, got.qword)
    expectEq("$what: single", sent.single, got.single)
    expectEq("$what: double", sent.double, got.double)
    expectEq("$what: text", sent.text, got.text)
    expectEq("$what: blob", sent.blob, got.blob)
    expectEq("$what: span", sent.span, got.span)
    expectEq("$what: at", sent.at, got.at)
    expectEq("$what: id", sent.id, got.id)
}
