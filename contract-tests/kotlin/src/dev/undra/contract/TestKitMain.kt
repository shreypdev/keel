package dev.undra.contract

import dev.undra.playground.core.QueryStatus
import dev.undra.playground.core.RemoteConfig
import dev.undra.playground.core.RemoteTodosQueryHandle
import dev.undra.playground.core.Todos
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.configureRemote
import dev.undra.playground.core.fileRead
import dev.undra.playground.core.fileWrite
import dev.undra.playground.core.kvGet
import dev.undra.playground.core.kvPut
import dev.undra.playground.core.secretGet
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

private fun t3SeededPortsAreWhatTheCoreReadsAndWhatItWritesLandsInTheFakes() {
    PreviewCore.load(UndraIds.SCHEMA_HASH, Seed.fromJson(SEED)).use { preview ->
        val core = preview.core
        runBlocking {
            check(kvGet("greeting", core)?.decodeToString() == "hello") { "the seeded Kv value" }
            check(kvGet("absent", core) == null) { "an absent key" }
            kvPut("saved", byteArrayOf(1, 2, 3), core)
            check(preview.fakes.kv.value("saved")?.toList() == listOf<Byte>(1, 2, 3)) { "the core's write reached the Kv fake" }
            check(secretGet("token", core)?.decodeToString() == "t-123") { "the seeded SecureStore value" }
            check(fileRead("notes/a.txt", core).decodeToString() == "hello") { "the seeded file" }
            fileWrite("out/b.txt", "written".toByteArray(), core)
            check(preview.fakes.fs.contents("out/b.txt")?.decodeToString() == "written") { "the core's file landed in the Fs fake" }
            val missing = try {
                fileRead("nope", core)
                null
            } catch (e: Exception) {
                e
            }
            check(missing != null && missing.message?.contains("not found") == true || missing is dev.undra.runtime.adapters.FsError) { "a missing file is a typed error: $missing" }
        }
        check(preview.fakes.kv.ops.count { it.op == "set" } == 1) { "Kv ops ${preview.fakes.kv.ops}" }
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
        "T3 preview: the seeded ports are what the core reads, and its writes land in the fakes" to ::t3SeededPortsAreWhatTheCoreReadsAndWhatItWritesLandsInTheFakes,
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
