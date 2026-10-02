// Runs the generated Kotlin of the `generic_functions` golden case (ADR-058): the instantiations of a generic function
// are overloads of its own name, each with a JVM name of its own, taking the type as a `KClass` when no argument fixes
// it; each calls the id of its own instantiation with the arguments of a hand-written function.

package golden.generic_functions

import golden.support.FakeCore
import golden.support.expect
import golden.support.expectEq
import kotlinx.coroutines.runBlocking

fun main() {
    val core = FakeCore()
    val todo = Todo(3u, "t")
    val note = Note(4u, "n")

    // The element type of the argument picks the overload; each one calls its own id. The fake core answers with no
    // bytes, so the decoding of the reply fails after the call was recorded: the call is what is checked here.
    runCatching { newest(listOf(todo), core) }
    expectEq(core.calls.last().methodId, UndraIds.Functions.NEWEST_TODO, "newest(List<Todo>)")
    expectEq(core.calls.last().args, "01000000" + "03000000" + "0100000074", "the arguments of newest")
    runCatching { newest(listOf(note), core) }
    expectEq(core.calls.last().methodId, UndraIds.Functions.NEWEST_NOTE, "newest(List<Note>)")
    expect(UndraIds.Functions.NEWEST_TODO != UndraIds.Functions.NEWEST_NOTE, "one id per instantiation")

    // A type parameter no argument fixes is a `KClass`, which is never encoded.
    runCatching { draft(Todo::class, core) }
    expectEq(core.calls.last().methodId, UndraIds.Functions.DRAFT_TODO, "draft(Todo::class)")
    expectEq(core.calls.last().args, "", "the token is not an argument")
    runCatching { make(Note::class, "kind", core) }
    expectEq(core.calls.last().methodId, UndraIds.Functions.MAKE_NOTE, "make(Note::class, ..)")
    expectEq(core.calls.last().args, "04000000" + "6b696e64", "a parameter called `type` beside the token")

    // `suspend` functions, a stream and a command.
    runBlocking { runCatching { load(Note::class, 9u, core) } }
    expectEq(core.calls.last().methodId, UndraIds.Functions.LOAD_NOTE, "load")
    expectEq(core.calls.last().args, "09000000", "the arguments of load")
    runBlocking { runCatching { save(todo, core) } }
    expectEq(core.calls.last().methodId, UndraIds.Functions.SAVE_TODO, "save")
    rows(Todo::class, 2u, core)
    forget(note, core)
    expectEq(core.calls.last().methodId, UndraIds.Functions.FORGET_NOTE, "forget")
    expectEq(core.reports.size, 0, "a command that reached the core reports nothing")

    // A function that is not generic stands beside the families.
    runCatching { countRows(core) }
    expectEq(core.calls.last().methodId, UndraIds.Functions.COUNT_ROWS, "countRows")

    // Methods of an object.
    val library = Library::class.java.getDeclaredConstructor(
        dev.undra.runtime.UndraCore::class.java,
        Long::class.javaPrimitiveType,
    ).apply { isAccessible = true }.newInstance(core, 7L)
    runCatching { library.pinned(Todo::class) }
    expectEq(core.calls.last().methodId, UndraIds.Objects.Library.PINNED_TODO, "pinned(Todo::class)")
    runBlocking { runCatching { library.remember(note) } }
    expectEq(core.calls.last().methodId, UndraIds.Objects.Library.REMEMBER_NOTE, "remember(note)")

    // Every instantiation has a JVM name of its own, so two with one erasure (`List<Todo>`, `List<Note>`) can live in
    // one file.
    val file = Class.forName("golden.generic_functions.ObjectsKt").declaredMethods.map { it.name }.toSet()
    for (name in listOf("newestTodo", "newestNote", "draftTodo", "draftNote", "loadTodo", "loadNote", "rowsTodo", "forgetNote")) {
        expect(name in file, "the JVM name $name")
    }
    expect("newest" !in file, "the generic function's own name is not a JVM name")
    val members = Library::class.java.declaredMethods.map { it.name }.toSet()
    expect("pinnedTodo" in members && "pinnedNote" in members && "rememberTodo" in members && "rememberNote" in members, "the JVM names of the methods: $members")
}
