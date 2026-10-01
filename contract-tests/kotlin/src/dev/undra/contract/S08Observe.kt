package dev.undra.contract

import dev.undra.playground.core.BigList
import dev.undra.playground.core.Counter
import dev.undra.playground.core.Filter
import dev.undra.playground.core.Item
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.Parity
import dev.undra.playground.core.Todo
import dev.undra.playground.core.Todos
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.decodeAll

/** S08: observing a store delivers its current values, as one change-set, before `observe` returns. */
fun s08Observe(w: World) {
    // 1. Raw: construct, register a mirror callback, observe. Four signals arrive, all as full values.
    val raw = RawStore(w.core, UndraIds.Objects.Todos.TYPE_ID, UndraIds.Objects.Todos.NEW)
    expectEq("entries before observe", 0, raw.entries.size)
    raw.observe()
    expectEq("the signals of the initial change-set", listOf(0u, 1u, 2u, 3u), raw.entries.map { it.signalId })
    check(raw.entries.all { it.op == ChangeOp.FULL }) { "the initial entries are not all full values: ${raw.entries}" }
    val (todos, filter, visible, remaining) = raw.entries
    expectEq("signal 0 todos", emptyList<Todo>(), Codecs.vec(Todo).decodeAll(todos.value))
    expectEq("signal 1 filter", Filter.ALL, Filter.decodeAll(filter.value))
    expectEq("signal 2 visible", emptyList<Todo>(), Codecs.vec(Todo).decodeAll(visible.value))
    expectEq("signal 3 remaining", 0u, Codecs.u32.decodeAll(remaining.value))

    // 4. Turn observation off: a write reaches nobody. Turn it on again: the current values, once.
    raw.observe(false)
    val mark = raw.mark()
    val added = Todo.decodeAll(raw.call(UndraIds.Objects.Todos.ADD, UndraWriter().also { it.writeStr("x") }.toByteArray()))
    holdsFor("the entries delivered while observation is off", 200) { raw.mark() == mark }
    raw.observe(true)
    val again = raw.since(mark)
    expectEq("the signals delivered when observation is turned on again", listOf(0u, 1u, 2u, 3u), again.map { it.signalId })
    expectEq("the list delivered when observation is turned on again", listOf(added), Codecs.vec(Todo).decodeAll(again[0].value))
    expectEq("remaining when observation is turned on again", 1u, Codecs.u32.decodeAll(again[3].value))

    // 5. Releasing a store takes its handle out of the core.
    val handles = w.stats().liveHandles
    raw.close()
    expectEq("live_handles after releasing the store", handles - 1, w.stats().liveHandles)

    // 2. Through the generated classes: the values are there when create() returns, with no await.
    Todos.create().use {
        expectEq("Todos.todos", emptyList<Todo>(), it.todos.value)
        expectEq("Todos.filter", Filter.ALL, it.filter.value)
        expectEq("Todos.visible", emptyList<Todo>(), it.visible.value)
        expectEq("Todos.remaining", 0u, it.remaining.value)
    }

    // 3. The other stores' initial values.
    Counter.create().use {
        expectEq("Counter.count", 0, it.count.value)
        expectEq("Counter.changes", 0u, it.changes.value)
        expectEq("Counter.parity", Parity.EVEN, it.parity.value)
    }
    BigList.create().use {
        expectEq("BigList.items.size", 10_000, it.items.value.size)
        expectEq("BigList.items[0]", Item(1u, "Item 1", 0u), it.items.value[0])
        expectEq("BigList.items[9999]", Item(10_000u, "Item 10000", 0u), it.items.value[9_999])
        expectEq("BigList.count", 10_000u, it.count.value)
    }
}
