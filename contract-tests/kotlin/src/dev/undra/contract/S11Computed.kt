package dev.undra.contract

import dev.undra.playground.core.BigList
import dev.undra.playground.core.Counter
import dev.undra.playground.core.Filter
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.Parity
import dev.undra.playground.core.Todo
import dev.undra.playground.core.Todos
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.decodeAll
import kotlinx.coroutines.runBlocking

/** S11: computed values are recomputed in the core and arrive with the write that changed them. */
fun s11Computed(w: World) {
    Todos.create().use { todos ->
        // 1. Three items, one finished.
        val a = runBlocking { todos.add("a") }
        val b = runBlocking { todos.add("b") }
        val c = runBlocking { todos.add("c") }
        check(a.id != b.id && b.id != c.id && a.id != c.id) { "the ids of a, b and c are not distinct: ${a.id} ${b.id} ${c.id}" }
        check(a.id.mostSignificantBits < b.id.mostSignificantBits && b.id.mostSignificantBits < c.id.mostSignificantBits) {
            "the ids do not count up: ${a.id} ${b.id} ${c.id}"
        }
        todos.toggle(b.id)
        val doneB = b.copy(done = true)
        awaitEq("Todos.visible after toggle(b)", listOf(a, doneB, c)) { todos.visible.value }
        awaitEq("Todos.remaining after toggle(b)", 2u) { todos.remaining.value }

        // 2. A filter change recomputes `visible` in the core: one transaction carries both.
        val before = w.stats().transactions
        todos.setFilter(Filter.DONE)
        expectEq("transactions grown by set_filter(Done)", 1L, w.stats().transactions - before)
        awaitEq("Todos.filter", Filter.DONE) { todos.filter.value }
        awaitEq("Todos.visible under Done", listOf(doneB)) { todos.visible.value }
        expectEq("Todos.remaining under Done", 2u, todos.remaining.value)

        // 3. Active, then All again.
        todos.setFilter(Filter.ACTIVE)
        awaitEq("Todos.visible under Active", listOf(a, c)) { todos.visible.value }
        todos.setFilter(Filter.ALL)
        awaitEq("Todos.visible under All", listOf(a, doneB, c)) { todos.visible.value }

        // 4. Removing and clearing follow through visible and remaining.
        todos.remove(a.id)
        awaitEq("Todos.visible after remove(a)", listOf(doneB, c)) { todos.visible.value }
        awaitEq("Todos.remaining after remove(a)", 1u) { todos.remaining.value }
        todos.clearDone()
        awaitEq("Todos.visible after clear_done()", listOf(c)) { todos.visible.value }
        awaitEq("Todos.remaining after clear_done()", 1u) { todos.remaining.value }
    }

    // 2 (raw). The same filter change seen entry by entry: filter and visible together, remaining not at all.
    val raw = RawStore(w.core, UndraIds.Objects.Todos.TYPE_ID, UndraIds.Objects.Todos.NEW)
    raw.observe()
    val doneTodo = Todo.decodeAll(raw.call(UndraIds.Objects.Todos.ADD, UndraWriter().also { it.writeStr("a") }.toByteArray()))
    raw.call(UndraIds.Objects.Todos.ADD, UndraWriter().also { it.writeStr("b") }.toByteArray())
    raw.callSync(UndraIds.Objects.Todos.TOGGLE, UndraWriter().also { Codecs.uuid.encode(it, doneTodo.id) }.toByteArray())
    flushMainThread()
    val mark = raw.mark()
    raw.callSync(UndraIds.Objects.Todos.SET_FILTER, UndraWriter().also { Filter.encode(it, Filter.DONE) }.toByteArray())
    flushMainThread()
    val changed = raw.since(mark)
    expectEq("the signals set_filter(Done) delivered", listOf(1u, 2u), changed.map { it.signalId }.sorted())
    check(changed.all { it.op == ChangeOp.FULL }) { "set_filter delivered an entry that is not a full value: $changed" }
    expectEq("the delivered filter", Filter.DONE, Filter.decodeAll(changed.single { it.signalId == 1u }.value))
    expectEq("the delivered visible", listOf(doneTodo.copy(done = true)), Codecs.vec(Todo).decodeAll(changed.single { it.signalId == 2u }.value))
    raw.close()

    // 5. The parity is the computed of the count, always consistent with it.
    Counter.create().use { counter ->
        for ((amount, parity) in listOf(3 to Parity.ODD, 1 to Parity.EVEN, -1 to Parity.ODD, 0 to Parity.ODD)) {
            counter.add(amount)
            awaitEq("Counter.parity after add($amount)", parity) { counter.parity.value }
            val count = counter.count.value
            expectEq("Counter.parity consistent with count $count", if (count % 2 == 0) Parity.EVEN else Parity.ODD, counter.parity.value)
        }
    }

    // 6. Reads never cross the boundary: a thousand reads of each signal leave crossings.calls where it was.
    Todos.create().use { todos ->
        Counter.create().use { counter ->
            BigList.create().use { list ->
                val calls = w.stats().calls
                var checksum = 0L
                repeat(1_000) {
                    checksum += todos.visible.value.size + todos.remaining.value.toLong()
                    checksum += counter.parity.value.index.toLong() + list.items.value.size
                }
                check(checksum > 0) { "the reads produced nothing" }
                expectEq("crossings.calls after 4,000 reads", calls, w.stats().calls)
            }
        }
    }
}
