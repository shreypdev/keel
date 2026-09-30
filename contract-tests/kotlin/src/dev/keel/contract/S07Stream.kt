package dev.keel.contract

import dev.keel.playground.core.Probe
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking

/**
 * S07: the core sends an item only against credit the collector granted, so a collector that stops
 * reading holds the producer near its window instead of letting it run to a thousand.
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
    probe.close()
}
