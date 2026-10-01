package dev.undra.contract

import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.Parity
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.decodeAll

/** S09: everything a transaction writes crosses the boundary as one change-set. */
fun s09Transaction(w: World) {
    val counter = RawStore(w.core, UndraIds.Objects.Counter.TYPE_ID, UndraIds.Objects.Counter.NEW)
    counter.observe()

    // 1. add(5) writes two signals and moves a computed one: three entries, one transaction.
    var before = w.stats()
    var mark = counter.mark()
    counter.callSync(UndraIds.Objects.Counter.ADD, int(5))
    flushMainThread()
    expectEq("the entries of add(5)", listOf(0u to 5, 1u to 1, 2u to 1), counterEntries(counter, mark))
    expectEq("transactions grown by add(5)", 1L, w.stats().transactions - before.transactions)
    expectEq("crossings.change_sets grown by add(5)", 1L, w.stats().changeSets - before.changeSets)

    // 2. Three calls are three transactions.
    before = w.stats()
    counter.callSync(UndraIds.Objects.Counter.INCREMENT)
    counter.callSync(UndraIds.Objects.Counter.INCREMENT)
    counter.callSync(UndraIds.Objects.Counter.DECREMENT)
    expectEq("transactions grown by three calls", 3L, w.stats().transactions - before.transactions)

    // 3. reset() writes two signals in one txn; the computed one follows.
    before = w.stats()
    mark = counter.mark()
    counter.callSync(UndraIds.Objects.Counter.RESET)
    flushMainThread()
    expectEq("the entries of reset()", listOf(0u to 0, 1u to 0, 2u to 0), counterEntries(counter, mark))
    expectEq("transactions grown by reset()", 1L, w.stats().transactions - before.transactions)
    counter.close()

    // 4. Bench: k dirty signals are still one change-set with k entries (signal 0 is the list, 1..128 the counters).
    val bench = RawStore(w.core, UndraIds.Objects.Bench.TYPE_ID, UndraIds.Objects.Bench.NEW)
    bench.observe()
    for ((touched, expected) in listOf(100 to 100, 1 to 1, 1_000 to 128)) {
        before = w.stats()
        mark = bench.mark()
        bench.callSync(UndraIds.Objects.Bench.BENCH_TOUCH_SIGNALS, UndraWriter().also { it.writeU32(touched.toUInt()) }.toByteArray())
        flushMainThread()
        val delivered = bench.since(mark)
        expectEq("entries delivered by bench_touch_signals($touched)", expected, delivered.size)
        expectEq("the signals of bench_touch_signals($touched)", (1u..expected.toUInt()).toList(), delivered.map { it.signalId }.sorted())
        check(delivered.all { it.op == ChangeOp.FULL }) { "bench_touch_signals($touched) delivered an entry that is not a full value" }
        expectEq("transactions grown by bench_touch_signals($touched)", 1L, w.stats().transactions - before.transactions)
        if (touched == 100) {
            // The counters were all zero: each one is now a full value 1.
            expectEq("the values of the first touch", List(100) { 1u }, delivered.sortedBy { it.signalId }.map { Codecs.u32.decodeAll(it.value) })
        }
    }
    bench.close()

    // 5. A store nobody observes delivers nothing; observing it afterwards delivers the current values once.
    val quiet = RawStore(w.core, UndraIds.Objects.Counter.TYPE_ID, UndraIds.Objects.Counter.NEW)
    before = w.stats()
    quiet.callSync(UndraIds.Objects.Counter.ADD, int(1))
    expectEq("transactions grown by a write to an unobserved store", 0L, w.stats().transactions - before.transactions)
    expectEq("entries delivered to an unobserved store", 0, quiet.entries.size)
    quiet.observe()
    expectEq("the entries delivered when the store is observed", listOf(0u to 1, 1u to 1, 2u to 1), counterEntries(quiet, 0))
    quiet.close()
}

/** The entries of a counter since [mark] as (signal id, number) by signal id; parity is 0 for even and 1 for odd. */
private fun counterEntries(counter: RawStore, mark: Int): List<Pair<UInt, Int>> =
    counter.since(mark).sortedBy { it.signalId }.map { entry ->
        check(entry.op == ChangeOp.FULL) { "counter entry ${entry.signalId} is not a full value" }
        entry.signalId to when (entry.signalId) {
            0u -> Codecs.i32.decodeAll(entry.value)
            1u -> Codecs.u32.decodeAll(entry.value).toInt()
            2u -> if (Parity.decodeAll(entry.value) == Parity.EVEN) 0 else 1
            else -> fail("a counter has no signal ${entry.signalId}")
        }
    }

private fun int(value: Int): ByteArray = UndraWriter().also { it.writeI32(value) }.toByteArray()
