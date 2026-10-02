package dev.undra.contract

import dev.undra.playground.core.LabError
import dev.undra.playground.core.Probe
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.wire.WireException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking

/**
 * S07: the core sends an item only against credit the collector granted, so a collector that stops
 * reading holds the producer near its window instead of letting it run to a thousand. A stream ends with
 * its own typed error part-way (step 6), or as cancelled by the core when a restore invalidates its receiver
 * (step 7, ADR-036).
 */
fun s07Stream(w: World) {
    val probe = Probe.create()
    val scope = CoroutineScope(Dispatchers.Default)

    // 1. Read five items, then stop reading (the collector blocks inside the flow, it does not cancel).
    probe.reset()
    val seen = CopyOnWriteArrayList<UInt>()
    val resume = CompletableDeferred<Unit>()
    val reading = scope.launch {
        probe.ticks(1_000u).collect { item ->
            seen.add(item)
            if (seen.size == 5) resume.await()
        }
    }
    awaitEq("the first five items", listOf(0u, 1u, 2u, 3u, 4u)) { seen.toList() }

    // 2. For 200 ms nothing more is read; the core produced a window's worth and stopped.
    holdsFor("the items read while the collector is paused", 200) { seen.size == 5 }
    val produced = probe.counters().produced
    check(produced in 5u..69u) { "after reading 5 items the core had produced $produced (the credit window allows 5 to 69)" }

    // 3. Resume: the rest arrives in order, the stream ends, and the core produced exactly a thousand.
    resume.complete(Unit)
    runBlocking { reading.join() }
    expectEq("every item", (0u until 1_000u).toList(), seen.toList())
    expectEq("items produced after the stream ended", 1_000u, probe.counters().produced)

    // 4. Early termination: read three items of a million and stop; the stream closes in the core.
    val streamsBefore = w.stats().openStreams
    probe.reset()
    val first = runBlocking { probe.ticks(1_000_000u).take(3).toList() }
    expectEq("the three items read", listOf(0u, 1u, 2u), first)
    awaitEq("open_streams back to its earlier value", streamsBefore) { w.stats().openStreams }
    val after = probe.counters().produced
    check(after < 200u) { "the stream of a million had produced $after items after it was abandoned" }

    // 5. A short stream ends by itself; an empty one ends without an item.
    expectEq("ticks(3)", listOf(0u, 1u, 2u), runBlocking { probe.ticks(3u).toList() })
    expectEq("ticks(0)", emptyList<UInt>(), runBlocking { probe.ticks(0u).toList() })

    // 6. A typed error part-way (ADR-036, flag 2 = the stream's own E): the items before it, then LabError.Rejected
    // through the generated fromReply, never a wire error. With fail_at past the end the stream just completes.
    val beforeError = CopyOnWriteArrayList<UInt>()
    val rejected = expectFailsAsync<LabError.Rejected>("ticksThenFail(5, 3, 7)") {
        probe.ticksThenFail(5u, 3u, 7).collect { beforeError.add(it) }
    }
    expectEq("the items of ticksThenFail(5, 3, 7) before its error", listOf(0u, 1u, 2u), beforeError.toList())
    expectEq("the error of ticksThenFail(5, 3, 7)", LabError.Rejected(code = 7, reason = "stopped at 3"), rejected)
    expectEq("ticksThenFail(3, 9, 7)", listOf(0u, 1u, 2u), runBlocking { probe.ticksThenFail(3u, 9u, 7).toList() })

    // 7. A stream the core cancels (ADR-036, flag 3 status 3): a restore invalidates the probe, which is not a store,
    // and ends its stream as cancelled by the core: UndraCallError.CancelledByCore, not LabError, not WireException.
    awaitEq("open_streams once the streams of steps 5 and 6 ended", streamsBefore) { w.stats().openStreams }
    val streamsBeforeRestore = w.stats().openStreams
    val snapshot = w.core.snapshot()
    val read = CopyOnWriteArrayList<UInt>()
    val restored = CompletableDeferred<Unit>()
    val ended = CompletableFuture<Throwable?>()
    scope.launch {
        try {
            probe.ticksThenFail(1_000_000u, 999_999u, 1).collect { item ->
                read.add(item)
                // Two items read, then nothing more until the restore is done (the credit window holds the core).
                if (read.size == 2) restored.await()
            }
            ended.complete(null)
        } catch (e: Throwable) {
            ended.complete(e)
        }
    }
    awaitEq("the first two items of ticksThenFail(1000000, 999999, 1)", listOf(0u, 1u)) { read.toList() }
    val restoreAt = System.nanoTime()
    w.core.restore(snapshot)
    restored.complete(Unit)
    val outcome = try {
        ended.get(WAIT_MS, TimeUnit.MILLISECONDS)
    } catch (e: TimeoutException) {
        fail("ticksThenFail(1000000, 999999, 1) did not end within $WAIT_MS ms of the restore")
    }
    val tookMs = (System.nanoTime() - restoreAt) / 1_000_000L
    check(outcome !is LabError) { "the stream the restore cancelled ended with the stream's own error $outcome" }
    check(outcome !is WireException) { "the stream the restore cancelled ended with a wire error: $outcome" }
    check(outcome is UndraCallError.CancelledByCore) {
        "the stream the restore cancelled ended with $outcome, not UndraCallError.CancelledByCore"
    }
    // Bounded by WAIT_MS, a hang detector: the restore ends the stream while it runs, on no timer, so a core that did not would
    // leave it open (or end it at item 999,999 with its own error, which the checks above refuse); how soon after the restore
    // the failure is seen is the machine's.
    check(tookMs < WAIT_MS) { "the stream the restore cancelled took $tookMs ms to end, past the $WAIT_MS ms wait" }
    awaitEq("open_streams after the core cancelled the stream", streamsBeforeRestore) { w.stats().openStreams }
    probe.close()
}
