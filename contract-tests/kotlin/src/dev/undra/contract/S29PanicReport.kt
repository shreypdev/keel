package dev.undra.contract

import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.add
import dev.undra.playground.core.explode
import dev.undra.playground.core.explodeDetached
import dev.undra.playground.core.explodeLater
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraUnhandledError

/**
 * S29, the native variant (scenarios.md): every panic the core contains reaches `LoadOptions.onPanic` as one structured report, on the
 * runtime's main thread, in order, while the call that panicked still fails as in S17 (ADR-046).
 */
fun s29PanicReport(w: World) {
    val panics = w.panics
    val before = panics.size
    val statsBefore = w.core.stats()

    // 1. explode("kaboom"): status 2 as in S17.1, and exactly one report with the fields of scenarios.md.
    val sync = expectFails<UndraCallError.Panicked>("explode(\"kaboom\")") { explode("kaboom") }
    check("kaboom" in sync.panicMessage) { "the panic message is \"${sync.panicMessage}\"" }
    awaitUntil("the report of explode(\"kaboom\")") { panics.size > before }
    holdsFor("no second report for explode(\"kaboom\")", 200) { panics.size == before + 1 }
    val kaboom = panics.since(before).single().report
    check("kaboom" in kaboom.message) { "the report's message is \"${kaboom.message}\"" }
    check("lab.rs:" in kaboom.location && Regex(":\\d+:\\d+$").containsMatchIn(kaboom.location)) {
        "the location is \"${kaboom.location}\", not lab.rs:<line>:<column>"
    }
    expectEq("operation of explode", "explode", kaboom.operation)
    check(kaboom.thread.isNotEmpty()) { "the report names no thread" }
    expectEq("namespace", "playground_core", kaboom.namespace)
    check(kaboom.coreVersion.isNotEmpty()) { "the report carries no core version" }
    expectEq("schemaHash", UndraIds.SCHEMA_HASH.toLong(), kaboom.schemaHash)
    check(Regex("[0-9a-f]*").matches(kaboom.imageId)) { "the imageId is \"${kaboom.imageId}\", neither empty nor lowercase hex" }
    check(kaboom.frames.isNotEmpty()) { "the report carries no frames" }
    check(kaboom.frames.any { it.symbol != null }) { "no frame is named, in a debug build: ${kaboom.frames}" }

    // 2. A panic inside an async call, and one in a detached task that nobody waits on.
    val later = expectFailsAsync<UndraCallError.Panicked>("explode_later(10, \"later\")") { explodeLater(10u, "later") }
    check("later" in later.panicMessage) { "the async panic message is \"${later.panicMessage}\"" }
    explodeDetached("task") // returns at once; the task panics on the core
    awaitUntil("the reports of explode_later and of the detached task") { panics.size >= before + 3 }
    holdsFor("no further report", 200) { panics.size == before + 3 }
    val all = panics.since(before)
    expectEq("operation of explode_later", "explode_later", all[1].report.operation)
    check("later" in all[1].report.message) { "the explode_later report's message is \"${all[1].report.message}\"" }
    expectEq("operation of the detached task", "task", all[2].report.operation)
    check("task" in all[2].report.message) { "the detached task's report message is \"${all[2].report.message}\"" }

    // 3. On the runtime's main thread, once per report, in the order the panics happened.
    for ((i, delivery) in all.withIndex()) {
        check(delivery.onMainThread) { "report $i was delivered on ${delivery.thread}, which is not the runtime's main thread" }
    }
    expectEq("the order of the reports", listOf("explode", "explode_later", "task"), all.map { it.report.operation })

    // 4. The counters grew by 3, and the core keeps working.
    val statsAfter = w.core.stats()
    expectEq("panics grown by the three panics", 3L, statsAfter.panics - statsBefore.panics)
    expectEq("panicReports grown by the three reports", 3L, statsAfter.panicReports - statsBefore.panicReports)
    expectEq("add(1, 2) after the panics", 3, add(1, 2))

    // 4, a reporter that throws: caught, reported to onError, and the next panic is reported all the same.
    w.takeUnhandled()
    panics.throwOnNext()
    expectFails<UndraCallError.Panicked>("explode(\"again\")") { explode("again") }
    awaitUntil("the report whose handler throws") { panics.size >= before + 4 }
    awaitUntil("onError to hear of the handler that threw") { w.unhandled.isNotEmpty() }
    val failure: UndraUnhandledError = w.takeUnhandled().single()
    expectEq("the operation onError was told about", "onPanic", failure.operation)
    check("the crash reporter is down" in failure.message.orEmpty()) { "onError's message is \"${failure.message}\"" }
    expectFails<UndraCallError.Panicked>("explode(\"after\")") { explode("after") }
    awaitUntil("the report after the one whose handler threw") { panics.size >= before + 5 }
    check("after" in panics.since(before).last().report.message) { "the last report is ${panics.since(before).last().report.message}" }
    holdsFor("no stray report", 200) { panics.size == before + 5 }
    expectEq("add(1, 2) after the reporter threw", 3, add(1, 2))
}
