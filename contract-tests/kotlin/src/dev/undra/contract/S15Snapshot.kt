package dev.undra.contract

import dev.undra.playground.core.BigList
import dev.undra.playground.core.Counter
import dev.undra.playground.core.Filter
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.Parity
import dev.undra.playground.core.Todos
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import java.util.Random
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
    expectFails<UndraException>("restore of 16 random bytes") { core.restore(noise) }
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
    todos.close()
    counter.close()
    list.close()
}
