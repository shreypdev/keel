package dev.undra.contract

import dev.undra.playground.core.BigList
import dev.undra.playground.core.Counter
import dev.undra.playground.core.Filter
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.Parity
import dev.undra.playground.core.Probe
import dev.undra.playground.core.Todos
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.UndraRestoreException
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import java.util.Random
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.runBlocking

/** S15: a snapshot of every store restores them in place: the handles the app holds stay valid. */
fun s15Snapshot(w: World) {
    val core = w.core
    val handlesAtStart = w.stats().liveHandles

    // 1. Three observed stores with some state, and a fourth that is released before the snapshot.
    val todos = Todos.create()
    val a = runBlocking { todos.add("a") }
    val b = runBlocking { todos.add("b") }
    todos.toggle(b.id)
    val counter = Counter.create()
    counter.add(5)
    val list = BigList.create()
    val released = Counter.create()
    released.add(1)
    released.close()
    awaitEq("Todos.remaining before the snapshot", 1u) { todos.remaining.value }
    awaitEq("Counter.count before the snapshot", 5) { counter.count.value }

    // 2. The snapshot is something, and opaque.
    val snapshot = core.snapshot()
    check(snapshot.isNotEmpty()) { "the snapshot is empty" }

    // 3. Change everything.
    runBlocking { todos.add("c") }
    todos.toggle(a.id)
    todos.setFilter(Filter.DONE)
    counter.add(10)
    list.removeAt(0u)
    awaitEq("Counter.count after the changes", 15) { counter.count.value }
    awaitEq("BigList.items.size after the changes", 9_999) { list.items.value.size }

    // 4. Restore: the same handles show the snapshot's values again.
    core.restore(snapshot)
    awaitEq("Todos.todos after the restore", listOf(a, b.copy(done = true))) { todos.todos.value }
    awaitEq("Todos.filter after the restore", Filter.ALL) { todos.filter.value }
    awaitEq("Todos.visible after the restore", listOf(a, b.copy(done = true))) { todos.visible.value }
    awaitEq("Todos.remaining after the restore", 1u) { todos.remaining.value }
    awaitEq("Counter.count after the restore", 5) { counter.count.value }
    awaitEq("Counter.changes after the restore", 1u) { counter.changes.value }
    awaitEq("Counter.parity after the restore", Parity.ODD) { counter.parity.value }
    awaitEq("BigList.items.size after the restore", 10_000) { list.items.value.size }
    expectEq("BigList.items[0].id after the restore", 1u, list.items.value[0].id)

    // 5. Identities continue above what the snapshot held; the list keeps working.
    val d = runBlocking { todos.add("d") }
    check(d.id.mostSignificantBits > b.id.mostSignificantBits) { "the id of d (${d.id}) is not above the id of b (${b.id})" }
    check(d.id != a.id && d.id != b.id) { "the id of d (${d.id}) collides with one the snapshot held" }
    awaitEq("Todos.todos after adding d", listOf(a, b.copy(done = true), d)) { todos.todos.value }

    // 6. A snapshot that is not one is rejected and leaves every store as it was.
    val noise = ByteArray(16).also { Random(7).nextBytes(it) }
    expectFails<UndraRestoreException>("restore of 16 random bytes") { core.restore(noise) }
    holdsFor("the stores after a rejected restore", 200) {
        todos.todos.value == listOf(a, b.copy(done = true), d) && counter.count.value == 5 && list.items.value.size == 10_000
    }

    // 7. A handle released before the snapshot is not resurrected by the restore.
    val gone = expectFails<UndraReplyException>("a call on the store released before the snapshot") {
        core.callSync(
            CallTarget.ObjectMethod(Handle(released.handle), UndraIds.Objects.Counter.INCREMENT),
            UndraIds.Objects.Counter.INCREMENT,
            ByteArray(0),
        )
    }
    expectEq("the status of a call on a handle released before the snapshot", ReplyStatus.BAD_REQUEST, gone.status)

    // 8. The restore made no handles of its own: what is alive is the three stores that survive.
    expectEq("live_handles after the restore", handlesAtStart + 3, w.stats().liveHandles)

    // 9. A call in flight across a restore ends as cancelled by the core (not as a platform cancellation), and
    // the invalidated object then refuses calls through the bindings.
    val probe = Probe.create()
    val hangEnded = CompletableFuture<Throwable?>()
    CoroutineScope(Dispatchers.Default).async {
        try {
            probe.hang()
            hangEnded.complete(null)
        } catch (e: Throwable) {
            hangEnded.complete(e)
            throw e
        }
    }
    awaitEq("probe.counters().started", 1u) { probe.counters().started }
    core.restore(core.snapshot())
    val hangOutcome = hangEnded.get(WAIT_MS, TimeUnit.MILLISECONDS)
    check(hangOutcome is UndraCallError.CancelledByCore) { "hang() across a restore ended with $hangOutcome, not UndraCallError.CancelledByCore" }
    expectRefused("probe.counters() after the restore", expectFails("probe.counters() after the restore") { probe.counters() })
    w.takeUnhandled()
    probe.reset()
    val resetReports = w.takeUnhandled()
    expectEq("the reports of probe.reset() after the restore", listOf("Probe.reset"), resetReports.map { it.operation })
    check(resetReports.single().error is UndraCallError.Refused) { "probe.reset() was reported as ${resetReports.single().error}, not Refused" }
    probe.close()

    // 10. A stream in flight across a restore ends as cancelled by the core: the core's own "cancelled: ..." String is
    // not read as a typed error, and the collector's iteration is not a platform cancellation.
    val streamed = Probe.create()
    val firstItem = CompletableFuture<UInt>()
    val streamEnded = CompletableFuture<Throwable?>()
    CoroutineScope(Dispatchers.Default).async {
        try {
            streamed.ticks(1_000_000u).collect { if (!firstItem.isDone) firstItem.complete(it) }
            streamEnded.complete(null)
        } catch (e: Throwable) {
            streamEnded.complete(e)
        }
    }
    firstItem.get(WAIT_MS, TimeUnit.MILLISECONDS)
    core.restore(core.snapshot())
    val streamOutcome = streamEnded.get(WAIT_MS, TimeUnit.MILLISECONDS)
    check(streamOutcome is UndraCallError.CancelledByCore) { "ticks() across a restore ended with $streamOutcome, not UndraCallError.CancelledByCore" }
    streamed.close()

    todos.close()
    counter.close()
    list.close()
}
