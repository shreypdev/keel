package dev.undra.runtime.support

import dev.undra.runtime.testing.fail

/** Polls [condition] every 5 ms until it holds; fails with [message] after [timeoutMs]. */
fun eventually(message: String = "condition", timeoutMs: Long = 10_000, condition: () -> Boolean) {
    val deadline = System.nanoTime() + timeoutMs * 1_000_000
    while (!condition()) {
        if (System.nanoTime() > deadline) fail("timed out after ${timeoutMs}ms waiting for: $message")
        Thread.sleep(5)
    }
}
