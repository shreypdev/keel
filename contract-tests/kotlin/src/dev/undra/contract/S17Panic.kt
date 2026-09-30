package dev.undra.contract

import dev.undra.playground.core.Counter
import dev.undra.playground.core.add
import dev.undra.playground.core.explode
import dev.undra.playground.core.explodeLater
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.wire.Payloads.ReplyStatus

/** S17: a panic in the core unwinds to the boundary and arrives as a reply with status panic; nothing else stops. */
fun s17Panic(w: World) {
    val counter = Counter.create()
    val panicsBefore = w.stats().panics

    // 1. A sync panic: status panic, the message and a backtrace, and the process is still here to see it.
    val sync = expectFails<UndraReplyException>("explode(\"kaboom\")") { explode("kaboom") }
    expectEq("the status of the panic", ReplyStatus.PANIC, sync.status)
    val info = sync.panicInfo ?: fail("the panic reply has no readable message")
    check("kaboom" in info.message) { "the panic message is \"${info.message}\"" }

    // 2. An async panic, inside a call that was already running.
    val later = expectFailsAsync<UndraReplyException>("explode_later(10, \"later\")") { explodeLater(10u, "later") }
    expectEq("the status of the async panic", ReplyStatus.PANIC, later.status)
    check("later" in (later.panicInfo?.message ?: "")) { "the async panic message is \"${later.panicInfo?.message}\"" }

    // 3. The core keeps working, a store made before still updates, and both panics were counted.
    expectEq("add(1, 2) after the panics", 3, add(1, 2))
    counter.increment()
    awaitEq("Counter.count after the panics", 1) { counter.count.value }
    expectEq("panics grown by the two panics", 2L, w.stats().panics - panicsBefore)

    // 4. Each panic reached the Log port at error level or above, under the target undra::panic.
    for (reason in listOf("kaboom", "later")) {
        awaitUntil("an undra::panic log record about \"$reason\"") {
            w.log.records.any { it.level >= 4 && it.target == "undra::panic" && reason in it.message }
        }
    }
    counter.close()
}
