package dev.keel.contract

import dev.keel.playground.core.KeelIds
import dev.keel.playground.core.Probe
import dev.keel.playground.core.addLater
import dev.keel.runtime.wire.Handle
import dev.keel.runtime.wire.Payloads.CallTarget
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.runBlocking

/** S04: async calls wait on the core's timer and resume the caller off the core's threads. */
fun s04AsyncCall(w: World) {
    // 1. One call: the right value, after roughly the delay.
    val started = System.nanoTime()
    val sum = runBlocking { addLater(20, 22, 50u) }
    val elapsedMs = (System.nanoTime() - started) / 1_000_000L
    expectEq("add_later(20, 22, 50)", 42, sum)
    check(elapsedMs >= 45) { "add_later(.., 50 ms) answered after $elapsedMs ms" }
    check(elapsedMs < 2_000) { "add_later(.., 50 ms) took $elapsedMs ms" }

    // 2. Three at once resolve in the order of their delays, each with its own value.
    val order = CopyOnWriteArrayList<Int>()
    val values = runBlocking(Dispatchers.Default) {
        listOf(1 to 60u, 2 to 20u, 3 to 40u)
            .map { (i, delayMs) -> async { addLater(i, 0, delayMs).also { order.add(i) } } }
            .awaitAll()
    }
    expectEq("the values of the three calls", listOf(1, 2, 3), values)
    expectEq("the order the three calls resolved in", listOf(2, 3, 1), order.toList())

    // 3. A method of an object, asynchronously.
    Probe.create().use { probe -> expectEq("Probe.wait(10)", 10u, runBlocking { probe.wait(10u) }) }

    // 4a. The continuation of a call calls again. The runtime must not resume it on the core's thread under the
    // core's lock, or the second call would wait for a lock its own thread holds.
    val twice = runBlocking(Dispatchers.Unconfined) { addLater(1, 1, 10u) to addLater(2, 2, 10u) }
    expectEq("two add_later calls, the second made where the first resumed", 2 to 4, twice)

    // 4b. A change observer calls the core again: a counter observed through the raw mirror, whose callback
    // (which runs on the main thread, never under the core's lock) increments it once more and starts an
    // async call, both from inside the callback.
    val counter = w.core.construct(KeelIds.Objects.Counter.TYPE_ID, KeelIds.Objects.Counter.NEW, ByteArray(0))
    val secondCall = CompletableFuture<Int>()
    val reachedTwo = CompletableFuture<Unit>()
    val incremented = AtomicBoolean(false)
    w.core.mirror.register(counter) { signalId, _, reader ->
        if (signalId == 0u) {
            val count = reader.readI32()
            if (count == 1 && incremented.compareAndSet(false, true)) {
                w.core.callSync(CallTarget.ObjectMethod(Handle(counter), KeelIds.Objects.Counter.INCREMENT), KeelIds.Objects.Counter.INCREMENT, ByteArray(0))
                secondCall.complete(runBlocking { addLater(3, 4, 10u) })
            }
            if (count == 2) reachedTwo.complete(Unit)
        }
    }
    w.core.observe(counter, RawStore.ALL_SIGNALS, true)
    w.core.callSync(CallTarget.ObjectMethod(Handle(counter), KeelIds.Objects.Counter.INCREMENT), KeelIds.Objects.Counter.INCREMENT, ByteArray(0))
    expectEq("an async call started inside a change observer", 7, secondCall.get(WAIT_MS, TimeUnit.MILLISECONDS))
    reachedTwo.get(WAIT_MS, TimeUnit.MILLISECONDS)
    w.core.release(counter)
}
