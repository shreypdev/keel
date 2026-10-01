package dev.undra.contract

import dev.undra.playground.core.Filter
import dev.undra.playground.core.Todo
import dev.undra.playground.core.Todos
import dev.undra.playground.core.UndraIds
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.KeyedPatch
import dev.undra.runtime.wire.PatchOp
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.decodeAll
import kotlinx.coroutines.runBlocking
import java.io.File
import java.util.UUID

/**
 * S19: `Todos.visible` is a derived list (ADR-039): it reaches the platform as keyed patches, never as a
 * whole list after the first; and 60,000 recorded operations over three derived views replay through this
 * runtime's decoder and applier to exactly the views the core computed.
 */
fun s19DerivedKeyedList(w: World) {
    val ids = UndraIds.Objects.Todos
    val raw = RawStore(w.core, ids.TYPE_ID, ids.NEW)
    var host: List<Todo> = emptyList()
    fun apply(entry: RawStore.Entry) {
        host = if (entry.op == ChangeOp.FULL) Codecs.vec(Todo).decodeAll(entry.value) else KeyedPatch.applyPatch(host, KeyedPatch.decodePatch(entry.value, Todo))
    }

    /** Runs [call] and returns the single `visible` (signal 2) entry and every entry it delivered. */
    fun visibleOf(call: () -> Unit): Pair<RawStore.Entry, List<RawStore.Entry>> {
        val mark = raw.mark()
        call()
        flushMainThread()
        val entries = raw.since(mark)
        val visible = entries.filter { it.signalId == 2u }
        expectEq("visible entries in the change-set", 1, visible.size)
        return visible[0] to entries
    }

    fun patch(entry: RawStore.Entry): List<PatchOp<Todo>> {
        expectEq("the op of the visible entry", ChangeOp.PATCH, entry.op)
        return KeyedPatch.decodePatch(entry.value, Todo)
    }

    fun remaining(entries: List<RawStore.Entry>): UInt = Codecs.u32.decodeAll(entries.single { it.signalId == 3u }.value)
    fun add(title: String): Todo = Todo.decodeAll(raw.call(ids.ADD, UndraWriter().also { it.writeStr(title) }.toByteArray()))
    fun toggle(todo: Todo) {
        raw.callSync(ids.TOGGLE, UndraWriter().also { Codecs.uuid.encode(it, todo.id) }.toByteArray())
    }
    fun setFilter(filter: Filter) {
        raw.callSync(ids.SET_FILTER, UndraWriter().also { Filter.encode(it, filter) }.toByteArray())
    }

    // 1. The initial visible entry is a full value, [].
    val mark = raw.mark()
    raw.observe()
    val initial = raw.since(mark).single { it.signalId == 2u }
    expectEq("the op of the initial visible entry", ChangeOp.FULL, initial.op)
    apply(initial)
    expectEq("the initial visible", emptyList<Todo>(), host)

    // 2. add a, b, c: one Insert at 0, 1, 2 each; remaining 1, 2, 3.
    val todos = ArrayList<Todo>()
    for ((index, title) in listOf("a", "b", "c").withIndex()) {
        var added: Todo? = null
        val (visible, entries) = visibleOf { added = add(title) }
        val todo = added!!
        todos.add(todo)
        expectEq("add($title): visible", listOf<PatchOp<Todo>>(PatchOp.Insert(index.toUInt(), todo)), patch(visible))
        apply(visible)
        expectEq("add($title): remaining", (index + 1).toUInt(), remaining(entries))
    }
    val (a, b, _) = todos

    // 3. toggle(b) under All: one Update at 1; remaining 2.
    val (toggled, afterToggle) = visibleOf { toggle(b) }
    expectEq("toggle(b): visible", listOf<PatchOp<Todo>>(PatchOp.Update(1u, b.copy(done = true))), patch(toggled))
    apply(toggled)
    expectEq("toggle(b): remaining", 2u, remaining(afterToggle))

    // 4. set_filter(Active): one Remove at 1; set_filter(All): one Insert of b at 1.
    var (entry, _) = visibleOf { setFilter(Filter.ACTIVE) }
    expectEq("set_filter(Active): visible", listOf<PatchOp<Todo>>(PatchOp.Remove(1u)), patch(entry))
    apply(entry)
    entry = visibleOf { setFilter(Filter.ALL) }.first
    expectEq("set_filter(All): visible", listOf<PatchOp<Todo>>(PatchOp.Insert(1u, b.copy(done = true))), patch(entry))
    apply(entry)

    // 5. Under Active, toggle(a): one Remove at 0; again: one Insert of a at 0.
    apply(visibleOf { setFilter(Filter.ACTIVE) }.first)
    entry = visibleOf { toggle(a) }.first
    expectEq("toggle(a) under Active: visible", listOf<PatchOp<Todo>>(PatchOp.Remove(0u)), patch(entry))
    apply(entry)
    entry = visibleOf { toggle(a) }.first
    expectEq("toggle(a) again: visible", listOf<PatchOp<Todo>>(PatchOp.Insert(0u, a)), patch(entry))
    apply(entry)
    apply(visibleOf { setFilter(Filter.ALL) }.first)

    // 6. fill(10000), then a toggle of a visible item: one op, under 100 bytes.
    apply(visibleOf { raw.callSync(ids.FILL, UndraWriter().also { it.writeU32(10_000u) }.toByteArray()) }.first)
    expectEq("visible after fill(10000)", 10_003, host.size)
    val target = host[5_000]
    check(!target.done) { "the target is open" }
    entry = visibleOf { toggle(target) }.first
    expectEq("toggle in 10,003 rows: visible", listOf<PatchOp<Todo>>(PatchOp.Update(5_000u, target.copy(done = true))), patch(entry))
    check(entry.value.size < 100) { "one op, not the view: ${entry.value.size} bytes" }
    apply(entry)
    raw.close()

    // 7. Through the generated class, visible equals the model after every step (calls on the main thread,
    //    where a synchronous call drains the mirror before it returns: read-your-writes).
    onMain(timeoutMs = 60_000L) {
        Todos(w.core).use { store ->
            val model = ArrayList<Todo>()
            var filter = Filter.ALL
            fun check(what: String) {
                val shown = model.filter { filter == Filter.ALL || (filter == Filter.ACTIVE) != it.done }
                expectEq("$what: visible", shown, store.visible.value)
                expectEq("$what: remaining", model.count { !it.done }.toUInt(), store.remaining.value)
            }
            for (title in listOf("a", "b", "c")) {
                model.add(runBlocking { store.add(title) })
            }
            // A synchronous call drains the mirror before it returns (on the main thread).
            store.setFilter(Filter.ALL)
            check("add a, b, c")
            fun flip(i: Int) {
                store.toggle(model[i].id)
                model[i] = model[i].copy(done = !model[i].done)
                check("toggle($i)")
            }
            flip(1)
            for (next in listOf(Filter.ACTIVE, Filter.ALL, Filter.ACTIVE)) {
                store.setFilter(next)
                filter = next
                check("set_filter($next)")
            }
            flip(0)
            flip(0)
            store.setFilter(Filter.ALL)
            filter = Filter.ALL
            check("set_filter(All)")
            store.fill(10_000u)
            // Identities count up from the store's counter: the nth is n in the first eight bytes.
            for (n in 0 until 10_000) model.add(Todo(UUID(4L + n, 0L), "Item ${n + 1}", n % 4 == 3))
            check("fill(10000)")
            flip(5_003)
            store.remove(model[0].id)
            model.removeAt(0)
            check("remove")
            store.clearDone()
            model.removeAll { it.done }
            check("clear_done")
        }
    }

    // 8. Reads never cross: 1,000 reads of visible leave crossings.calls unchanged.
    Todos.create().use { store ->
        runBlocking { store.add("x") }
        awaitEq("visible after add(x)", 1) { store.visible.value.size }
        val before = w.stats().calls
        var sink = 0
        repeat(1_000) { sink += store.visible.value.size }
        expectEq("crossings.calls after 1,000 reads of visible", 0L, w.stats().calls - before)
        expectEq("what the reads saw", 1_000, sink)
    }

    // 9. 60,000 recorded operations over three views replay, change-set by change-set, to the core's views.
    replayDerivedVectors()
}

private class Row(val id: UInt, val title: String, val done: Boolean, val rank: UInt)

private class Label(val id: UInt, val text: String)

private object RowCodec : UndraCodec<Row> {
    override fun encode(w: UndraWriter, v: Row) {
        w.writeU32(v.id)
        w.writeStr(v.title)
        w.writeBool(v.done)
        w.writeU32(v.rank)
    }

    override fun decode(r: UndraReader): Row = Row(r.readU32(), r.readStr(), r.readBool(), r.readU32())
}

private object LabelCodec : UndraCodec<Label> {
    override fun encode(w: UndraWriter, v: Label) {
        w.writeU32(v.id)
        w.writeStr(v.text)
    }

    override fun decode(r: UndraReader): Label = Label(r.readU32(), r.readStr())
}

/** FNV-1a 64 of [bytes]: what the recording hashes each view's encoding with. */
private fun fnv1a64(bytes: ByteArray, length: Int): ULong {
    var hash = 0xcbf29ce484222325uL
    for (i in 0 until length) {
        hash = (hash xor bytes[i].toUByte().toULong()) * 0x100000001b3uL
    }
    return hash
}

private fun <T> hashOf(list: List<T>, codec: UndraCodec<T>): ULong {
    val w = UndraWriter()
    Codecs.vec(codec).encode(w, list)
    val bytes = w.toByteArray()
    return fnv1a64(bytes, bytes.size)
}

/** The view of one signal of the recording, applied with this runtime's decoder and applier. */
private class View<T>(val codec: UndraCodec<T>) {
    var rows: List<T> = emptyList()

    fun apply(op: ChangeOp, value: ByteArray): Boolean {
        if (op == ChangeOp.FULL) {
            rows = Codecs.vec(codec).decodeAll(value)
            return false
        }
        rows = KeyedPatch.applyPatch(rows, KeyedPatch.decodePatch(value, codec))
        return true
    }

    fun hash(): ULong = hashOf(rows, codec)
}

/** Reads the `UDV1` recording (contract-tests/derived-vectors.sh) and checks every view after every change-set. */
private fun replayDerivedVectors() {
    val path = System.getenv("UNDRA_DERIVED_VECTORS")
        ?: fail("UNDRA_DERIVED_VECTORS is not set (contract-tests/kotlin/run.sh sets it after writing the recording)")
    val file = File(path)
    if (!file.isFile) fail("no derived-list recording at $path (contract-tests/derived-vectors.sh writes it)")
    val r = UndraReader(file.readBytes())
    val magic = String(byteArrayOf(r.readI8(), r.readI8(), r.readI8(), r.readI8()))
    expectEq("the recording's magic", "UDV1", magic)
    val viewCount = r.readU32().toInt()
    val records = r.readU32().toInt()
    check(records > 20_000) { "the recording has $records change-sets" }
    val views = listOf<View<*>>(View(RowCodec), View(RowCodec), View(LabelCodec))
    var patches = 0
    for (i in 0 until records) {
        val payload = r.readRaw(r.readU32().toInt())
        for (e in Payloads.ChangeSet.decode(payload).entries) {
            if (views[e.signalId.toInt()].apply(e.op, e.value)) patches++
        }
        for (v in 0 until viewCount) {
            val expected = r.readU64()
            if (views[v].hash() != expected) fail("view $v differs from the core's after change-set $i")
        }
    }
    r.finish()
    check(patches > 50_000) { "only $patches patches were replayed" }
}
