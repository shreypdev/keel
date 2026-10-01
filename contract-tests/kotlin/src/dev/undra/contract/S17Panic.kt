package dev.undra.contract

import dev.undra.playground.core.Counter
import dev.undra.playground.core.add
import dev.undra.playground.core.explode
import dev.undra.playground.core.explodeLater
import dev.undra.playground.core.failLater
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraTransportException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async

/** S17: a panic in the core unwinds to the boundary and arrives as a reply with status panic; nothing else stops. */
fun s17Panic(w: World) {
    val counter = Counter.create()
    val panicsBefore = w.stats().panics

    // 1. A sync panic: status panic, the message and a backtrace, and the process is still here to see it.
    val sync = expectFails<UndraCallError.Panicked>("explode(\"kaboom\")") { explode("kaboom") }
    check("kaboom" in sync.panicMessage) { "the panic message is \"${sync.panicMessage}\"" }

    // 2. An async panic, inside a call that was already running.
    val later = expectFailsAsync<UndraCallError.Panicked>("explode_later(10, \"later\")") { explodeLater(10u, "later") }
    check("later" in later.panicMessage) { "the async panic message is \"${later.panicMessage}\"" }

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

    // 5. Re-entry is refused, not deadlocked or aborted. The core logs the panic of `explode("reenter")` through
    // the runner's Log adapter, a synchronous port called on the thread that holds the core lock; the adapter calls
    // the generated `add(1, 1)` once from there.
    var reentered: Throwable? = null
    var reenteredReturned = false
    w.log.onNextRecord({ it.level >= 4 && it.target == "undra::panic" && "reenter" in it.message }) {
        try {
            add(1, 1)
            reenteredReturned = true
        } catch (e: Throwable) {
            reentered = e
        }
    }
    expectFails<UndraCallError.Panicked>("explode(\"reenter\")") { explode("reenter") }
    check(!reenteredReturned) { "add(1, 1) from inside the Log port returned" }
    // The Kotlin runtime refuses the call itself, before the core can (its `InprocTransport` knows it is inside a
    // callback), with the same bad request the core would answer: `Refused`, reason `E_REENTRANT`.
    val refused = reentered as? UndraCallError.Refused ?: fail("the Log adapter did not run during the panic, or add(1, 1) failed with $reentered")
    check("E_REENTRANT" in refused.reason) { "the refusal does not name E_REENTRANT: ${refused.reason}" }
    expectEq("add(1, 2) after the re-entrant call", 3, add(1, 2))

    // 6. Shutdown with a typed call in flight. This step ends the core, so it is the last of the run (S17 is the
    // last entry of SCENARIOS).
    val inFlight = CompletableFuture<Throwable?>()
    CoroutineScope(Dispatchers.Default).async {
        try {
            failLater(5_000u, 1)
            inFlight.complete(null)
        } catch (e: Throwable) {
            inFlight.complete(e)
            throw e
        }
    }
    Thread.sleep(100)
    val shutdownAt = System.nanoTime()
    w.core.close()
    val ended = inFlight.get(WAIT_MS, TimeUnit.MILLISECONDS)
    check(ended is UndraCallError.Unavailable && ended.transport.reason == UndraTransportException.Reason.CLOSED) {
        "fail_later across a shutdown ended with $ended, not Unavailable (closed)"
    }
    val tookMs = (System.nanoTime() - shutdownAt) / 1_000_000L
    check(tookMs < 1_000L) { "the call in flight took $tookMs ms to fail" }
    val closed = expectFails<UndraCallError.Unavailable>("add(1, 2) on the shut-down core") { add(1, 2, w.core) }
    expectEq("the transport reason of add(1, 2) on the shut-down core", UndraTransportException.Reason.CLOSED, closed.transport.reason)
    w.takeUnhandled()
    counter.increment() // a command: it returns, and reports
    val closedCommand = w.takeUnhandled()
    expectEq("the reports of Counter.increment() on a store of the shut-down core", listOf("Counter.increment"), closedCommand.map { it.operation })
    val unavailable = closedCommand.single().error as? UndraCallError.Unavailable ?: fail("the command was reported as ${closedCommand.single().error}, not Unavailable")
    expectEq("the transport reason of the reported command", UndraTransportException.Reason.CLOSED, unavailable.transport.reason)
    counter.close()
    // The shut-down core is no longer the shared one: a constructor with the default core fails as in S16.5.
    check(UndraCore.current == null) { "the shut-down core is still UndraCore.current" }
    expectFails<UndraCallError.Unavailable>("Counter.create() after the shared core was closed") { Counter.create() }
}
