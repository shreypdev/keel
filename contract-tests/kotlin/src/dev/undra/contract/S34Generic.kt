package dev.undra.contract

import dev.undra.playground.core.Note
import dev.undra.playground.core.NoteSelection
import dev.undra.playground.core.RecentTodos
import dev.undra.playground.core.Todo
import dev.undra.playground.core.TodoSelection
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.draft
import dev.undra.playground.core.newest
import dev.undra.playground.core.recentTodos
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.wire.Fnv
import dev.undra.runtime.wire.KeyedPatch
import dev.undra.runtime.wire.PatchOp
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.encodeToByteArray
import java.util.UUID

/** A to-do whose identity carries the counter [n]: the first eight bytes of its `UUID`, big-endian, which is what the core orders by. */
private fun todo(n: Long, title: String) = Todo(UUID(n, 0L), title, false)

/** A note whose id is the counter [n]. */
private fun note(n: Long, title: String) = Note(n, title, false)

/**
 * S34 (ADR-058): a generic function is one function per listed type (`newest(rows)`, `draft(Todo::class, title)`), a generic store is
 * one store per alias (`TodoSelection` and `NoteSelection`: two type ids, two handles, two signals) and a generic object is a class of
 * the alias's name (`RecentTodos`). The `selection` module of the playground core, through the generated API.
 */
fun s34Generic(w: World) {
    functions()
    twoStores(w)
    snapshotAndRestore(w)
    unknownInstantiation(w)
    objectFromFunction()
}

/** 1 and 2. One function per type; a type no argument names is named by the caller. */
private fun functions() {
    expectEq("newest of three to-dos", todo(9, "nine"), newest(listOf(todo(5, "five"), todo(9, "nine"), todo(7, "seven"))))
    expectEq("newest of two notes", note(2, "two"), newest(listOf(note(2, "two"), note(1, "one"))))
    expectEq("newest of no to-dos", null, newest(emptyList<Todo>()))
    expectEq("newest of no notes", null, newest(emptyList<Note>()))

    val first = draft(Todo::class, "first")
    val second = draft(Todo::class, "second")
    expectEq("the title of a draft", "first", first.title)
    check(!first.done) { "a draft is done" }
    check(first.id != second.id) { "two drafts share the identity ${first.id}" }
    expectEq("the title of a drafted note", "a note", draft(Note::class, "a note").title)
    expectEq("newest of two drafts", "second", newest(listOf(first, second))?.title)
}

/** The keyed-patch operations of the `rows` entry (signal 0) among [entries]. */
private fun <T> rowOps(what: String, entries: List<RawStore.Entry>, codec: UndraCodec<T>): List<PatchOp<T>> {
    val rows = entries.filter { it.signalId == 0u }
    expectEq("$what: entries for rows", 1, rows.size)
    expectEq("$what: the op of the rows entry", ChangeOp.PATCH, rows[0].op)
    return KeyedPatch.decodePatch(rows[0].value, codec)
}

/** 3. Two instantiations are two stores: one keyed Insert to the one that was ticked, nothing to the other. */
private fun twoStores(w: World) {
    val todoType = UndraIds.Objects.TodoSelection
    val noteType = UndraIds.Objects.NoteSelection
    check(todoType.TYPE_ID != noteType.TYPE_ID) { "the two selections share the type id ${todoType.TYPE_ID}" }
    val todos = RawStore(w.core, todoType.TYPE_ID, todoType.NEW)
    val notes = RawStore(w.core, noteType.TYPE_ID, noteType.NEW)
    todos.observe()
    notes.observe()
    check(todos.handle != notes.handle) { "the two selections share the handle ${todos.handle}" }

    fun toggle(store: RawStore, method: UInt, bytes: ByteArray): Unit = store.callSync(method, bytes).let { flushMainThread() }
    val todoMark = todos.mark()
    val noteMark = notes.mark()
    toggle(todos, todoType.TOGGLE, Todo.encodeToByteArray(todo(3, "three")))
    val inserted = todos.since(todoMark)
    expectEq("ticking a to-do: the operations", listOf<PatchOp<Todo>>(PatchOp.Insert(0u, todo(3, "three"))), rowOps("ticking a to-do", inserted, Todo))
    expectEq("ticking a to-do: rows and count arrive together", setOf(0u, 1u), inserted.map { it.signalId }.toSet())
    expectEq("the note selection heard nothing", emptyList<String>(), notes.since(noteMark).map { it.toString() })

    val todoMark2 = todos.mark()
    toggle(todos, todoType.TOGGLE, Todo.encodeToByteArray(todo(3, "three")))
    expectEq("unticking: the operations", listOf<PatchOp<Todo>>(PatchOp.Remove(0u)), rowOps("unticking", todos.since(todoMark2), Todo))
    expectEq("the note selection still heard nothing", emptyList<String>(), notes.since(noteMark).map { it.toString() })

    val todoMark3 = todos.mark()
    toggle(notes, noteType.TOGGLE, Note.encodeToByteArray(note(4, "four")))
    expectEq(
        "ticking a note: the operations",
        listOf<PatchOp<Note>>(PatchOp.Insert(0u, note(4, "four"))),
        rowOps("ticking a note", notes.since(noteMark), Note),
    )
    expectEq("the to-do selection heard nothing", emptyList<String>(), todos.since(todoMark3).map { it.toString() })
    todos.close()
    notes.close()
}

/** 4. Snapshot and restore keep both stores and their handles. */
private fun snapshotAndRestore(w: World) {
    val ticked = TodoSelection.create()
    val marked = NoteSelection.create()
    ticked.toggle(todo(1, "one"))
    ticked.toggle(todo(2, "two"))
    marked.toggle(note(8, "eight"))
    awaitEq("the to-do selection's count", 2u) { ticked.count.value }
    awaitEq("the note selection's count", 1u) { marked.count.value }
    val handles = ticked.handle to marked.handle
    val snapshot = w.core.snapshot()
    ticked.clear()
    marked.toggle(note(9, "nine"))
    awaitEq("the cleared to-do selection", emptyList<Todo>()) { ticked.rows.value }
    awaitEq("the note selection's count after another tick", 2u) { marked.count.value }

    w.core.restore(snapshot)
    awaitEq("the to-do selection after the restore", listOf(todo(1, "one"), todo(2, "two"))) { ticked.rows.value }
    awaitEq("the note selection after the restore", listOf(note(8, "eight"))) { marked.rows.value }
    expectEq("the restore kept both handles", handles, ticked.handle to marked.handle)
    expectEq("the to-do selection's count after the restore", 2u, ticked.count.value)
    expectEq("the note selection's count after the restore", 1u, marked.count.value)
    ticked.toggle(todo(1, "one"))
    awaitEq("the restored to-do selection keeps working", listOf(todo(2, "two"))) { ticked.rows.value }
    expectEq("the note selection is untouched by it", listOf(note(8, "eight")), marked.rows.value)
    ticked.close()
    marked.close()
}

/** 5. An id that names no instantiation is refused as a bad request. */
private fun unknownInstantiation(w: World) {
    val missing = Fnv.fnv1a32("fn.newest<Draft>")
    val refused = expectFails<UndraReplyException>("a function id that names no instantiation") {
        w.core.callSync(CallTarget.FreeFunction(missing), missing, ByteArray(4))
    }
    expectBadRequest("fn.newest<Draft>", refused)
    expectEq("the id of newest<Todo>", Fnv.fnv1a32("fn.newest<Todo>"), UndraIds.Functions.NEWEST_TODO)
    expectEq("the id of newest<Note>", Fnv.fnv1a32("fn.newest<Note>"), UndraIds.Functions.NEWEST_NOTE)
    expectEq("the core after the refusal", "one", newest(listOf(todo(1, "one")))?.title)
}

/** 6. A generic object a function returns is the alias's class. */
private fun objectFromFunction() {
    val recent: RecentTodos = recentTodos(listOf(todo(1, "one"), todo(2, "two"), todo(3, "three")), 2u)
    expectEq("the rows of a RecentTodos", listOf(todo(3, "three"), todo(2, "two")), recent.rows())
    expectEq("the latest of a RecentTodos", todo(3, "three"), recent.latest())
    recent.open(todo(2, "two"))
    expectEq("the rows after opening one", listOf(todo(2, "two"), todo(3, "three")), recent.rows())
    val other = recentTodos(emptyList(), 5u)
    check(other !== recent) { "a second object is the same wrapper" }
    expectEq("a second object has rows of its own", null, other.latest())
    other.close()
    expectEq("closing one object leaves the other", todo(2, "two"), recent.latest())
    recent.close()
}
