package dev.undra.testkit.testing

import java.io.File

fun fail(message: String): Nothing = throw AssertionError(message)

fun assertTrue(condition: Boolean, message: String = "expected true") {
    if (!condition) fail(message)
}

fun <T> assertEq(expected: T, actual: T, message: String = "") {
    val same = if (expected is ByteArray && actual is ByteArray) expected.contentEquals(actual) else expected == actual
    if (!same) fail("${if (message.isEmpty()) "" else "$message: "}expected <${show(expected)}> but was <${show(actual)}>")
}

private fun show(v: Any?): String = when (v) {
    is ByteArray -> "bytes[${v.size}] " + v.joinToString("") { "%02x".format(it) }.take(96)
    is UByte, is UShort, is UInt, is ULong -> "$v (unsigned)"
    else -> v.toString()
}

/** Runs [block], which must throw a subtype of [E]; returns it for field checks. */
inline fun <reified E : Throwable> assertThrows(message: String = "", block: () -> Unit): E {
    try {
        block()
    } catch (t: Throwable) {
        if (t is E) return t
        throw AssertionError("${if (message.isEmpty()) "" else "$message: "}expected ${E::class.java.simpleName} but got ${t.javaClass.name}: ${t.message}", t)
    }
    fail("${if (message.isEmpty()) "" else "$message: "}expected ${E::class.java.simpleName} but nothing was thrown")
}

/** The checked-in fixtures every kit reads (`testkit/` of the repository). `-Dundra.testkit.dir` names it; otherwise it is found above the working directory. */
fun fixture(name: String): String {
    val configured = System.getProperty("undra.testkit.dir")
    val root = if (configured != null) File(configured) else generateSequence(File(System.getProperty("user.dir")).absoluteFile) { it.parentFile }
        .map { File(it, "testkit") }.firstOrNull { File(it, "fixtures").isDirectory } ?: fail("cannot find the testkit/ directory of the repository; pass -Dundra.testkit.dir")
    return File(root, name).readText()
}
