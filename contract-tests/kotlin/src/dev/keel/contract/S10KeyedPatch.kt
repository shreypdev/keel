package dev.keel.contract

import dev.keel.playground.core.BigList
import dev.keel.playground.core.Item
import dev.keel.playground.core.KeelIds
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.KeelWriter
import dev.keel.runtime.wire.KeyedPatch
import dev.keel.runtime.wire.PatchOp
import dev.keel.runtime.wire.Payloads.ChangeOp
import dev.keel.runtime.wire.decodeAll

/** S10: a one-row change to a 10,000-row list crosses as a keyed patch of one operation. */
fun s10KeyedPatch(w: World) {
    val list = RawStore(w.core, KeelIds.Objects.BigList.TYPE_ID, KeelIds.Objects.BigList.NEW)
    list.observe()

    // 1. The initial entry for `items` (signal 0) is a full value of 10,000 items.
    val initial = list.entries.single { it.signalId == 0u }
    expectEq("the op of the initial items entry", ChangeOp.FULL, initial.op)
    val shown = ArrayList(Codecs.vec(Item).decodeAll(initial.value)) // what the host holds; patches are applied to it
    expectEq("the initial items", 10_000, shown.size)
    val model = ArrayList(shown) // what the operations should produce, maintained by hand

    /** Runs [call], checks that it delivered one patch with exactly [expected] and that the host list equals the model. */
    fun step(what: String, expected: List<PatchOp<Item>>, call: () -> Unit): RawStore.Entry {
        val mark = list.mark()
        call()
        flushMainThread()
        val patch = list.since(mark).single { it.signalId == 0u }
        expectEq("$what: the op of the items entry", ChangeOp.PATCH, patch.op)
        val ops = KeyedPatch.decodePatch(patch.value, Item)
        expectEq("$what: the operations", expected, ops)
        val patched = KeyedPatch.applyPatch(shown, ops)
        shown.clear()
        shown.addAll(patched)
        expectEq("$what: the list the patch produces", model.toList(), shown.toList())
        return patch
    }

    // 2. insert_at returns the new id; the patch is one Insert, tiny, and the computed count follows.
    val mark = list.mark()
    val fresh = Item(10_001u, "fresh", 0u)
    model.add(5_000, fresh)
    val insert = step("insert_at(5000, \"fresh\")", listOf(PatchOp.Insert(5_000u, fresh))) {
        expectEq("the id insert_at returned", 10_001u, Codecs.u32.decodeAll(list.callSync(KeelIds.Objects.BigList.INSERT_AT, indexAndLabel(5_000u, "fresh"))))
    }
    check(insert.value.size < 100) { "the patch of one insert is ${insert.value.size} bytes" }
    val count = list.since(mark).single { it.signalId == 1u }
    expectEq("the op of the count entry", ChangeOp.FULL, count.op)
    expectEq("the count after the insert", 10_001u, Codecs.u32.decodeAll(count.value))

    // 3. update_at bumps the version.
    model[42] = Item(43u, "renamed", 1u)
    step("update_at(42, \"renamed\")", listOf(PatchOp.Update(42u, Item(43u, "renamed", 1u)))) {
        list.callSync(KeelIds.Objects.BigList.UPDATE_AT, indexAndLabel(42u, "renamed"))
    }

    // 4. move_item: remove at `from`, insert so that it ends at `to` (after the insert the list has 10,001 items).
    model.add(9_000, model.removeAt(10))
    step("move_item(10, 9000)", listOf(PatchOp.Move(10u, 9_000u))) {
        list.callSync(KeelIds.Objects.BigList.MOVE_ITEM, KeelWriter().also { it.writeU32(10u); it.writeU32(9_000u) }.toByteArray())
    }

    // 5. remove_at.
    model.removeAt(0)
    step("remove_at(0)", listOf(PatchOp.Remove(0u))) {
        list.callSync(KeelIds.Objects.BigList.REMOVE_AT, KeelWriter().also { it.writeU32(0u) }.toByteArray())
    }
    list.close()

    // 7. The bench list: one insert into 10,000 rows is also one operation.
    val bench = RawStore(w.core, KeelIds.Objects.Bench.TYPE_ID, KeelIds.Objects.Bench.NEW)
    bench.observe()
    val benchMark = bench.mark()
    bench.callSync(KeelIds.Objects.Bench.BENCH_LIST_INSERT, KeelWriter().also { it.writeU32(123u) }.toByteArray())
    flushMainThread()
    val benchPatch = bench.since(benchMark).single { it.signalId == 0u }
    expectEq("bench_list_insert: the op of the rows entry", ChangeOp.PATCH, benchPatch.op)
    expectEq(
        "bench_list_insert: the operations",
        listOf<PatchOp<Item>>(PatchOp.Insert(123u, Item(10_001u, "Item 10001", 0u))),
        KeyedPatch.decodePatch(benchPatch.value, Item),
    )
    bench.close()

    // 8. reset() after removing the first item restores it with one Insert.
    val again = RawStore(w.core, KeelIds.Objects.BigList.TYPE_ID, KeelIds.Objects.BigList.NEW)
    again.observe()
    again.callSync(KeelIds.Objects.BigList.REMOVE_AT, KeelWriter().also { it.writeU32(0u) }.toByteArray())
    flushMainThread()
    val resetMark = again.mark()
    again.callSync(KeelIds.Objects.BigList.RESET)
    flushMainThread()
    val reset = again.since(resetMark).single { it.signalId == 0u }
    expectEq("reset(): the op of the items entry", ChangeOp.PATCH, reset.op)
    expectEq(
        "reset(): the operations",
        listOf<PatchOp<Item>>(PatchOp.Insert(0u, Item(1u, "Item 1", 0u))),
        KeyedPatch.decodePatch(reset.value, Item),
    )
    again.close()
}

private fun indexAndLabel(index: UInt, label: String): ByteArray = KeelWriter().also {
    it.writeU32(index)
    it.writeStr(label)
}.toByteArray()
