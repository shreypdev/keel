package dev.undra.contract

import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.add
import dev.undra.playground.core.greet
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus

/**
 * S03: the synchronous path answers without an event loop. `add` and `greet` are plain functions (not
 * `suspend`), and this scenario calls them from a plain function, which is the whole of step 2: the
 * file would not compile otherwise.
 */
fun s03SyncCall(w: World) {
    // 1. Sync calls compute; i32 addition wraps.
    expectEq("add(40, 2)", 42, add(40, 2))
    expectEq("add(2147483647, 1)", Int.MIN_VALUE, add(Int.MAX_VALUE, 1))
    expectEq("greet(\"Ada\")", "Hello, Ada, from the playground core", greet("Ada"))

    // 3. Ten thousand sync calls, each right, each one crossing.
    val before = w.stats().calls
    val started = System.nanoTime()
    for (i in 0 until 10_000) {
        val sum = add(i, 2 * i)
        if (sum != 3 * i) fail("add($i, ${2 * i}) was $sum in the loop")
    }
    val nanos = System.nanoTime() - started
    expectEq("crossings.calls grown by 10,000 sync calls", 10_000L, w.stats().calls - before)
    println("note S03 mean ${nanos / 10_000} ns per sync add call (no budget asserted here)")

    // 4. A sync call of an async method is refused as a bad request, and nothing is harmed.
    val w2 = UndraWriter()
    w2.writeI32(1)
    w2.writeI32(1)
    w2.writeU32(10u)
    val refused = expectFails<UndraReplyException>("callSync of the async add_later") {
        w.core.callSync(CallTarget.FreeFunction(UndraIds.Functions.ADD_LATER), UndraIds.Functions.ADD_LATER, w2.toByteArray())
    }
    expectEq("the status of a sync call of an async method", ReplyStatus.BAD_REQUEST, refused.status)
    check(!refused.badRequestReason.isNullOrEmpty()) { "the bad request carries no reason" }
    expectEq("add(1, 1) after the refusal", 2, add(1, 1))
}
