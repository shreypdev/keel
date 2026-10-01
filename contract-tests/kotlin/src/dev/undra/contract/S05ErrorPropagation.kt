package dev.undra.contract

import dev.undra.playground.core.BigList
import dev.undra.playground.core.Counter
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.LabError
import dev.undra.playground.core.ListError
import dev.undra.playground.core.Probe
import dev.undra.playground.core.TodoError
import dev.undra.playground.core.Todos
import dev.undra.playground.core.add
import dev.undra.playground.core.failLater
import dev.undra.playground.core.parseCount
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus

/** S05: a failure arrives on the same path as a success, as the typed error of the method or as a reply status. */
fun s05ErrorPropagation(w: World) {
    val badRequestsAtStart = w.stats().badRequests

    // 1. A sync typed error is thrown where a result would have been returned.
    val notANumber = expectFails<LabError.NotANumber>("parse_count(\"x\")") { parseCount("x") }
    expectEq("the text of the failure", "x", notANumber.value)

    // 2. An async typed error.
    val rejected = expectFailsAsync<LabError.Rejected>("fail_later(10, 7)") { failLater(10u, 7) }
    expectEq("the code of the failure", 7, rejected.code)
    expectEq("the reason of the failure", "on purpose", rejected.reason)

    // 3. Store errors: a refused command changes nothing, and does not consume an identity.
    Todos.create().use { todos ->
        val before = w.stats().transactions
        expectFailsAsync<TodoError.EmptyTitle>("Todos.add(\"   \")") { todos.add("   ") }
        holdsFor("the change-set count after a refused add", 100) { w.stats().transactions == before }
        check(todos.todos.value.isEmpty()) { "a refused add changed the list: ${todos.todos.value}" }
    }
    BigList.create().use { list ->
        val remove = expectFails<ListError.OutOfRange>("BigList.remove_at(10000)") { list.removeAt(10_000u) }
        expectEq("the position of remove_at(10000)", 10_000u, remove.index)
        expectEq("the length of remove_at(10000)", 10_000u, remove.len)
        val insert = expectFails<ListError.OutOfRange>("BigList.insert_at(10001, \"x\")") { list.insertAt(10_001u, "x") }
        expectEq("the position of insert_at(10001)", 10_001u, insert.index)
        expectEq("the length of insert_at(10001)", 10_000u, insert.len)
        expectEq("the id of the next insert_at(0, \"y\")", 10_001u, list.insertAt(0u, "y"))
    }

    // 4. Reply statuses the typed errors do not cover. Each is a bad request with a reason.
    val unknown = expectFails<UndraReplyException>("an unknown method id") {
        w.core.callSync(CallTarget.FreeFunction(0xDEADBEEFu), 0xDEADBEEFu, ByteArray(0))
    }
    expectBadRequest("an unknown method id", unknown)

    val released = Probe.create()
    released.close()
    expectBadRequest("a call on a released handle", expectFails("a call on a released handle") { released.counters() })

    val undecodable = expectFails<UndraReplyException>("a constructor with undecodable arguments") {
        w.core.construct(UndraIds.Objects.RemoteTodosQueryHandle.TYPE_ID, UndraIds.Objects.RemoteTodosQueryHandle.NEW, ByteArray(0))
    }
    expectBadRequest("a constructor with undecodable arguments", undecodable)

    // 5. The core is unharmed, and exactly those three requests were counted as bad.
    expectEq("add(1, 2) afterwards", 3, add(1, 2))
    expectEq("bad_requests grown by the three bad requests", 3L, w.stats().badRequests - badRequestsAtStart)

    // 6. Through the generated bindings, on closed objects: both calls throw a bad request (Kotlin throws from
    // every shape; Swift's command `Counter.increment()` reports to `onError` instead, ADR-032).
    val badRequestsBeforeClosed = w.stats().badRequests
    val closedList = BigList.create()
    closedList.close()
    expectBadRequest("BigList.remove_at(0) on a closed list", expectFails("BigList.remove_at(0) on a closed list") { closedList.removeAt(0u) })
    val closedCounter = Counter.create()
    closedCounter.close()
    expectBadRequest("Counter.increment() on a closed counter", expectFails("Counter.increment() on a closed counter") { closedCounter.increment() })
    expectEq("bad_requests grown by the two closed calls", 2L, w.stats().badRequests - badRequestsBeforeClosed)
    expectEq("add(1, 2) after the closed calls", 3, add(1, 2))
}

/** Checks that [e] is a bad-request reply that says why. */
internal fun expectBadRequest(what: String, e: UndraReplyException) {
    expectEq("$what: the reply status", ReplyStatus.BAD_REQUEST, e.status)
    check(!e.badRequestReason.isNullOrBlank()) { "$what: the bad request carries no reason" }
}
