// The Kotlin JVM consumer of the Bazel example (ADR-061): the generated bindings (`undra_bindings`), the Kotlin runtime and the
// host build of the core (`undra_core`), all built by Bazel, and the real core answering through JNI. Every check prints one
// `bazel kotlin:` line; any failure exits 1, which is what makes `bazel test //kotlin:hello_test` fail.
package dev.undra.bazel.hello.test

import dev.undra.bazel.hello.Todos
import dev.undra.bazel.hello.UndraHelloCore
import dev.undra.bazel.hello.TodoError
import dev.undra.bazel.hello.greeting
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.UndraDispatchers
import java.io.File
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.system.exitProcess
import kotlinx.coroutines.runBlocking

private var failed = false

private fun check(what: String, ok: Boolean) {
    println("bazel kotlin: ${if (ok) "ok  " else "FAIL"} $what")
    if (!ok) failed = true
}

/** Runs [block] on the runtime's main thread, where a call's change-sets are applied before it returns. */
private fun <T> onMain(block: () -> T): T {
    val result = CompletableFuture<T>()
    UndraDispatchers.main.dispatch(kotlin.coroutines.EmptyCoroutineContext, Runnable {
        try {
            result.complete(block())
        } catch (e: Throwable) {
            result.completeExceptionally(e)
        }
    })
    return result.get(10, TimeUnit.SECONDS)
}

fun main() {
    // The library `undra_core` built, as Bazel put it in this test's runfiles; the runtime loads it by this property.
    val library = File(requireNotNull(System.getProperty("undra.test.library")) { "no undra.test.library" })
    check("the host library is in the runfiles: ${library.name}", library.isFile)
    System.setProperty("undra.native.${UndraHelloCore.NAMESPACE}.path", library.absolutePath)

    val core = UndraHelloCore.load(LoadOptions())
    check("the core loaded through JNI: ${UndraHelloCore.NAMESPACE}", UndraHelloCore.core === core)

    val greeting = greeting("Bazel")
    check("a function crosses the boundary: \"$greeting\"", greeting == "Hello, Bazel, from the bazel-hello core")

    val todos = onMain { Todos() }
    runBlocking { todos.add("Write the rules") }
    runBlocking { todos.add("Run bazel test") }
    val remaining = onMain { todos.remaining.value }
    check("a store's signals mirror the core: 2 items remain, the mirror says $remaining", remaining == 2u)

    val failure = try {
        runBlocking { todos.add("   ") }
        null
    } catch (e: TodoError) {
        e
    }
    check("a typed error from Rust arrives as a Kotlin exception: $failure", failure === TodoError.EmptyTitle)

    val first = onMain { todos.todos.value.first().id }
    onMain { todos.toggle(first) }
    val afterToggle = onMain { todos.remaining.value }
    check("a command changes the signals the UI observes: $afterToggle remaining", afterToggle == 1u)

    core.close()
    println("bazel kotlin: ${if (failed) "FAILED" else "passed"}")
    exitProcess(if (failed) 1 else 0)
}
