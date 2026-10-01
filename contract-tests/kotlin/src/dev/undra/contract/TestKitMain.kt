package dev.undra.contract

import dev.undra.playground.core.QueryStatus
import dev.undra.playground.core.RemoteConfig
import dev.undra.playground.core.RemoteTodosQueryHandle
import dev.undra.playground.core.Todos
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.configureRemote
import dev.undra.runtime.UndraNative
import dev.undra.testkit.PreviewCore
import dev.undra.testkit.RecordedCore
import dev.undra.testkit.Seed
import dev.undra.testkit.httpResponse
import java.io.File
import java.nio.file.Files
import kotlin.system.exitProcess
import kotlinx.coroutines.runBlocking

// The Kotlin testing kit (docs/TESTING.md) against the real playground core over JNI: PreviewCore loads the app's own core with the
// deterministic fakes as its ports and a manual clock; RecordedCore plays a recorded session under the generated store. Not numbered
// scenarios (the grid is the platforms agreeing on the boundary); this is the kit's own proof that its fakes and the real core work together.

private val ROOT = generateSequence(File(System.getProperty("user.dir")).absoluteFile) { it.parentFile }
    .first { File(it, "testkit/fixtures").isDirectory }
private val SEED = File(ROOT, "testkit/fixtures/seed.json").readText()
private val SESSION = File(ROOT, "testkit/fixtures/session-todos.json").readText()

private fun <T> eventually(what: String, read: () -> T, done: (T) -> Boolean): T {
    val deadline = System.nanoTime() + 5_000_000_000L
    var v = read()
    while (!done(v)) {
        check(System.nanoTime() < deadline) { "timed out waiting for $what (last: $v)" }
        Thread.sleep(5)
        v = read()
    }
    return v
}

private fun t1StoreRunsTheRealLogicOnTheFakes() {
    PreviewCore.load(UndraIds.SCHEMA_HASH, Seed.fromJson(SEED)).use { preview ->
        val todos = Todos(preview.core)
        runBlocking {
            todos.add("Buy milk")
            val walk = todos.add("Walk the dog")
            todos.toggle(walk.id)
        }
        preview.settle()
        check(todos.visible.value.map { it.title } == listOf("Buy milk", "Walk the dog")) { "visible: ${todos.visible.value}" }
        check(todos.remaining.value == 1u) { "remaining: ${todos.remaining.value}" }
    }
}

private fun t2SeedAnswersTheQueryAndTheManualClockMakesItStale() {
    PreviewCore.load(UndraIds.SCHEMA_HASH, Seed.fromJson(SEED)).use { preview ->
        configureRemote(RemoteConfig("https://api.test"), preview.core)
        val first = RemoteTodosQueryHandle.create("inbox", preview.core)
        preview.settle()
        check(first.status.value == QueryStatus.SUCCESS) { "status ${first.status.value}" }
        check(first.data.value?.map { it.title } == listOf("Buy milk", "Walk the dog")) { "data ${first.data.value}" }
        check(preview.fakes.http.calls.map { it.url } == listOf("https://api.test/lists/inbox/todos")) { "calls ${preview.fakes.http.calls.map { it.url }}" }

        // Fresh for 30 s: a second observer is served from the cache.
        preview.advance(10_000)
        RemoteTodosQueryHandle.create("inbox", preview.core)
        check(preview.fakes.http.calls.size == 1) { "a fresh entry was fetched again" }

        // Past it, the next observer fetches again, and everyone sees the new list.
        preview.fakes.http.reset()
        preview.fakes.http.respond("https://api.test/lists/inbox/todos", httpResponse(200, """[{"id":3,"title":"Third","done":false}]"""))
        preview.advance(31_000)
        RemoteTodosQueryHandle.create("inbox", preview.core)
        preview.settle()
        eventually("the refreshed list", { first.data.value?.map { it.title } }) { it == listOf("Third") }
        check(preview.fakes.http.calls.size == 1) { "calls after the stale fetch: ${preview.fakes.http.calls.size}" }
        check(preview.clock.nowMs == 1_700_000_041_000L) { "clock ${preview.clock.nowMs}" }
    }
}

private fun t3KvIsTheQueryCachesPersistence() {
    PreviewCore.load(UndraIds.SCHEMA_HASH, Seed.fromJson("""{"now_ms": 5000}""")).use { preview ->
        configureRemote(RemoteConfig("https://api.test"), preview.core)
        preview.fakes.http.respond("https://api.test/lists/inbox/todos", httpResponse(200, "[]"))
        RemoteTodosQueryHandle.create("inbox", preview.core)
        // The query is persisted: a quarter of a second after the fetch the core writes its cache through the Kv port. The core's `ctx.sleep` runs on
        // the runtime's own timer thread in a native core (the manual clock moves the `Clock` port, not that thread), so the write is waited for.
        preview.settle()
        eventually("the cache write", { preview.fakes.kv.ops.map { it.op } }) { "set" in it }
        check(preview.fakes.kv.keys().isNotEmpty()) { "the kv fake holds nothing" }
    }
}

private fun t4RecordedSessionUnderTheGeneratedStore() {
    val recorded = RecordedCore.load(SESSION, UndraIds.SCHEMA_HASH, makeShared = false)
    try {
        val todos = Todos(recorded.core)
        check(todos.todos.value.isEmpty() && todos.remaining.value == 0u) { "the first state is the empty list" }
        recorded.advance(100)
        check(todos.todos.value.map { it.title } == listOf("Buy milk")) { "after 100 ms: ${todos.todos.value.map { it.title }}" }
        recorded.advance(200)
        check(todos.todos.value.map { it.title } == listOf("Buy milk", "Walk the dog", "Write the docs")) { "after 300 ms" }
        check(todos.remaining.value == 3u) { "remaining ${todos.remaining.value}" }
        recorded.playAll()
        check(todos.todos.value.map { it.done } == listOf(true, false, false)) { "done flags ${todos.todos.value.map { it.done }}" }
        check(todos.remaining.value == 2u) { "remaining at the end ${todos.remaining.value}" }
    } finally {
        recorded.close()
    }
}

fun main() {
    System.setProperty("undra.data.dir", Files.createTempDirectory("undra-testkit-kotlin").toString())
    if (!UndraNative.isAvailable) {
        println("the native core library could not be loaded: ${UndraNative.unavailableReason}")
        exitProcess(2)
    }
    val cases = listOf(
        "T1 preview: a store runs the real logic on the fakes" to ::t1StoreRunsTheRealLogicOnTheFakes,
        "T2 preview: the seed answers the query and the manual clock makes it stale" to ::t2SeedAnswersTheQueryAndTheManualClockMakesItStale,
        "T3 preview: the kv fake is the query cache's persistence" to ::t3KvIsTheQueryCachesPersistence,
        "T4 recorded: a recorded session plays under the generated store" to ::t4RecordedSessionUnderTheGeneratedStore,
    )
    var failures = 0
    for ((name, body) in cases) {
        try {
            body()
            println("TESTKIT PASS $name")
        } catch (e: Throwable) {
            failures++
            println("TESTKIT FAIL $name: ${e.message}")
            e.stackTrace.take(8).forEach { println("    at $it") }
        }
        // One in-process core per process: a preview closes its core, and the next one starts fresh.
    }
    println("---- ${cases.size - failures} of ${cases.size} testkit checks passed")
    exitProcess(if (failures == 0) 0 else 1)
}
