package dev.keel.runtime.testing

import dev.keel.runtime.wire.WireException
import java.nio.ByteBuffer

fun fail(message: String): Nothing = throw AssertionError(message)

fun assertTrue(condition: Boolean, message: String = "expected true") {
    if (!condition) fail(message)
}

fun <T> assertEq(expected: T, actual: T, message: String = "") {
    val same = when {
        expected is ByteArray && actual is ByteArray -> expected.contentEquals(actual)
        expected is Float && actual is Float -> expected.toRawBits() == actual.toRawBits()
        expected is Double && actual is Double -> expected.toRawBits() == actual.toRawBits()
        else -> expected == actual
    }
    if (!same) {
        fail("${if (message.isEmpty()) "" else "$message: "}expected <${show(expected)}> but was <${show(actual)}>")
    }
}

fun assertBytes(expectedHex: String, actual: ByteArray, message: String = "") {
    assertEq(expectedHex.lowercase(), hex(actual), message)
}

private fun show(v: Any?): String = when (v) {
    is ByteArray -> "bytes[${v.size}] ${hex(v).take(96)}"
    is UByte, is UShort, is UInt, is ULong -> "$v (unsigned)"
    else -> v.toString()
}

/** Runs [block], which must throw exactly a subtype of [E]; returns the exception for field checks. */
inline fun <reified E : Throwable> assertThrows(message: String = "", block: () -> Unit): E {
    try {
        block()
    } catch (t: Throwable) {
        if (t is E) return t
        throw AssertionError(
            "${if (message.isEmpty()) "" else "$message: "}expected ${E::class.java.simpleName} but got ${t.javaClass.name}: ${t.message}",
            t,
        )
    }
    fail("${if (message.isEmpty()) "" else "$message: "}expected ${E::class.java.simpleName} but nothing was thrown")
}

/** Convenience for decode failures: the block must throw exactly the [WireException] subtype [E]. */
inline fun <reified E : WireException> assertWire(message: String = "", block: () -> Unit): E =
    assertThrows<E>(message, block)

// ---- bytes and hex ---------------------------------------------------------------------------------

private const val HEX = "0123456789abcdef"

fun hex(bytes: ByteArray): String {
    val sb = StringBuilder(bytes.size * 2)
    for (b in bytes) {
        val v = b.toInt() and 0xFF
        sb.append(HEX[v shr 4]).append(HEX[v and 0xF])
    }
    return sb.toString()
}

fun unhex(s: String): ByteArray {
    require(s.length % 2 == 0) { "odd hex length: '$s'" }
    return ByteArray(s.length / 2) { i -> s.substring(2 * i, 2 * i + 2).toInt(16).toByte() }
}

/** `bytesOf(0x01, 0xFF)`: bytes from ints so callers need no `.toByte()` noise. */
fun bytesOf(vararg values: Int): ByteArray = ByteArray(values.size) { values[it].toByte() }

/**
 * The string made of the given Unicode code points, so tests can name boundary characters
 * (`cp(0x7FF)`, `cp(0x10FFFF)`) without pasting invisible literals into the source.
 */
fun cp(vararg codePoints: Int): String {
    val sb = StringBuilder()
    for (c in codePoints) sb.appendCodePoint(c)
    return sb.toString()
}

/** A direct buffer holding [bytes], positioned at 0 and big-endian (Java's default, like the ones JNI hands out). */
fun directBuffer(bytes: ByteArray): ByteBuffer {
    val b = ByteBuffer.allocateDirect(bytes.size)
    b.put(bytes)
    b.flip()
    return b
}
