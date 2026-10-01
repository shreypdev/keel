package dev.undra.contract

import kotlinx.coroutines.runBlocking

/** What every scenario waits at most, for anything asynchronous (scenarios.md, "Waiting"). */
const val WAIT_MS: Long = 5_000L

/** How often a wait looks at its condition. */
private const val POLL_MS: Long = 10L

/** A step of a scenario did not hold. The message says which step and what was seen. */
class Mismatch(message: String) : AssertionError(message)

/** Fails the scenario with [message]. */
fun fail(message: String): Nothing = throw Mismatch(message)

/** Fails with [what] unless [condition] holds. */
inline fun check(condition: Boolean, what: () -> String) {
    if (!condition) throw Mismatch(what())
}

/** Fails unless [actual] equals [expected]; arrays compare by content. */
fun <T> expectEq(what: String, expected: T, actual: T) {
    if (!sameValue(expected, actual)) throw Mismatch("$what: expected <${show(expected)}> but was <${show(actual)}>")
}

private fun sameValue(a: Any?, b: Any?): Boolean =
    when {
        a is ByteArray && b is ByteArray -> a.contentEquals(b)
        a is Float && b is Float -> a.toRawBits() == b.toRawBits() // -0.0 is not 0.0; NaN is NaN
        a is Double && b is Double -> a.toRawBits() == b.toRawBits()
        else -> a == b
    }

private fun show(value: Any?): String =
    when (value) {
        is ByteArray -> "ByteArray(${value.size})" + if (value.size <= 16) value.joinToString(prefix = "[", postfix = "]") else ""
        is String -> if (value.length > 80) "\"${value.take(80)}...\" (${value.length} chars)" else "\"$value\""
        is List<*> -> if (value.size > 12) "List(${value.size}) ${value.take(6)}..." else value.toString()
        else -> value.toString()
    }

/** Runs [body], which must throw an [E]; returns it so that the caller can look inside. */
inline fun <reified E : Throwable> expectFails(what: String, body: () -> Unit): E {
    try {
        body()
    } catch (e: Throwable) {
        if (e is E) return e
        throw Mismatch("$what: expected ${E::class.simpleName} but got $e")
    }
    throw Mismatch("$what: expected ${E::class.simpleName} but nothing was thrown")
}

/** [expectFails] for a suspending [body], run to completion on the calling thread. */
inline fun <reified E : Throwable> expectFailsAsync(what: String, crossinline body: suspend () -> Unit): E =
    expectFails(what) { runBlocking { body() } }

/** Polls [condition] every 10 ms until it holds; fails after [timeoutMs] naming [what]. */
fun awaitUntil(what: String, timeoutMs: Long = WAIT_MS, condition: () -> Boolean) {
    val deadline = System.nanoTime() + timeoutMs * 1_000_000L
    while (!condition()) {
        if (System.nanoTime() > deadline) throw Mismatch("timed out after $timeoutMs ms waiting for $what")
        Thread.sleep(POLL_MS)
    }
}

/** Polls [produce] until it returns something other than `null`, and returns that. */
fun <T : Any> awaitValue(what: String, timeoutMs: Long = WAIT_MS, produce: () -> T?): T {
    var found: T? = null
    awaitUntil(what, timeoutMs) {
        found = produce()
        found != null
    }
    return found!!
}

/** Waits until [read] returns [expected], then returns; on timeout the message shows the last value seen. */
fun <T> awaitEq(what: String, expected: T, timeoutMs: Long = WAIT_MS, read: () -> T) {
    var last: T = read()
    try {
        awaitUntil(what, timeoutMs) {
            last = read()
            sameValue(expected, last)
        }
    } catch (e: Mismatch) {
        throw Mismatch("$what: expected <${show(expected)}> but it stayed <${show(last)}> (${e.message})")
    }
}

/** "For [millis] ms nothing happens": fails as soon as [condition] stops holding during that time. */
fun holdsFor(what: String, millis: Long = 200L, condition: () -> Boolean) {
    val deadline = System.nanoTime() + millis * 1_000_000L
    while (System.nanoTime() < deadline) {
        if (!condition()) throw Mismatch("$what: changed within $millis ms")
        Thread.sleep(POLL_MS)
    }
    if (!condition()) throw Mismatch("$what: changed within $millis ms")
}
