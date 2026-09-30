package dev.undra.contract

import dev.undra.playground.core.Probe
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.runBlocking

/** S06: cancelling the coroutine drops the future in the core, which the probe's drop guard counts. */
fun s06Cancellation(w: World) {
    val probe = Probe.create()
    val scope = CoroutineScope(Dispatchers.Default)
    val activeBefore = w.stats().activeCalls
    val cancelledBefore = w.stats().cancelled

    // 1. A call that never ends, running in the core.
    val hung = CompletableFuture<Throwable?>()
    val hang = scope.async {
        try {
            probe.hang()
            hung.complete(null)
        } catch (e: Throwable) {
            hung.complete(e)
            throw e
        }
    }
    awaitEq("probe.counters().started", 1u) { probe.counters().started }

    // 2. Cancel it: the caller sees a cancellation at once.
    runBlocking { hang.cancelAndJoin() }
    val outcome = hung.get(WAIT_MS, TimeUnit.MILLISECONDS)
    check(outcome is CancellationException) { "the cancelled call ended with $outcome, not a CancellationException" }

    // 3. And the core dropped the future: cancelled is 1 and completed 0, the call is no longer active.
    awaitEq("probe.counters().cancelled", 1u) { probe.counters().cancelled }
    expectEq("probe.counters().completed", 0u, probe.counters().completed)
    awaitEq("active_calls back to its earlier value", activeBefore) { w.stats().activeCalls }
    expectEq("crossings.cancelled grown by the cancelled call", 1L, w.stats().cancelled - cancelledBefore)

    // 4. Independence: cancelling one of two calls in flight leaves the other alone.
    val waited = scope.async { probe.wait(100u) }
    val hangAgain = scope.async { probe.hang() }
    awaitEq("probe.counters().started", 3u) { probe.counters().started }
    runBlocking { hangAgain.cancelAndJoin() }
    expectEq("the call that was not cancelled", 100u, runBlocking { waited.await() })
    awaitEq("probe.counters().cancelled", 2u) { probe.counters().cancelled }
    expectEq("probe.counters().completed", 1u, probe.counters().completed)

    // 5. Cancelling after completion is a no-op.
    val done = scope.async { probe.wait(1u) }
    runBlocking { done.await() }
    runBlocking { done.cancelAndJoin() }
    holdsFor("probe.counters().cancelled after cancelling a finished call", 100) { probe.counters().cancelled == 2u }
    probe.close()
}
