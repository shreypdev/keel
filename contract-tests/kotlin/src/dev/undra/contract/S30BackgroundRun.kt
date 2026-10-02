package dev.undra.contract

import dev.undra.playground.core.QueryStatus
import dev.undra.playground.core.RemoteTodo
import dev.undra.playground.core.RemoteTodosQueryHandle
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.add
import dev.undra.playground.core.createRemoteTodo
import dev.undra.playground.core.storageStatus
import dev.undra.runtime.UndraCore
import dev.undra.runtime.adapters.AppState
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.LifecycleEvents
import dev.undra.runtime.adapters.NetKind
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking

/**
 * S30, `core.runInBackground(deadlineMs)` over the standard function `run_background` (scenarios.md, ADR-046): offline work is pending, a run
 * offline does not wait, a run online drains the queue, a run cut at its deadline leaves the work intact, a host that cancels the call loses
 * nothing, and the counters say what happened.
 */
fun s30BackgroundRun(w: World) {
    w.configureRemoteOnce()
    val core = w.core
    val list = "s30"
    val url = "${World.BASE_URL}/lists/$list/todos"
    val scope = CoroutineScope(Dispatchers.Default)
    val runsBefore = core.stats().background.runs
    val finishedBefore = core.stats().background.finished
    val replayedBefore = core.stats().background.replayed
    // The harness failed the first read of the offline queue (S20 step 4); the client reads it again after a backoff, and only a queue it
    // could read is persisted and replayed.
    awaitUntil("the offline queue to be readable") { storageStatus().queueReadable }
    w.server.respond(HttpMethod.GET, url, 200, "[]")
    val handle = RemoteTodosQueryHandle.create(list)
    try {
        awaitUntil("the first fetch of the list") { handle.status.value == QueryStatus.SUCCESS }

        // 1. Offline work is pending: a queued creation, and the stats say a background window is worth asking for.
        w.connectivity.changed(online = false, kind = NetKind.NONE)
        Thread.sleep(50)
        w.server.fail(HttpMethod.POST, url, HttpError.Network("offline"))
        val queued = CompletableFuture<Result<RemoteTodo>>()
        scope.launch { queued.complete(runCatching { createRemoteTodo(list, "Queued") }) }
        awaitEq("storage_status().pending after the offline creation", 1u) { storageStatus().pending }
        holdsFor("the creation, which is queued and must not finish", 200) { !queued.isDone }
        val background = core.stats().background
        check(background.tasks >= 3) { "the core registers ${background.tasks} background tasks, not the 3 of the query runtime" }
        check(background.pending >= 1) { "stats().background.pending is ${background.pending} with a mutation queued" }
        check(core.stats().background.hasPendingWork) { "hasPendingWork is false with pending ${core.stats().background.pending}" }
        backgroundFlushesAtOnce(w, handle, url, list)

        // 2. Still offline: the run says so and does not wait. A run that waited would return at its deadline less the half second it keeps
        // for the host (4.5 s); under half the deadline tells the two apart on a machine that stalls for a second.
        val offlineStarted = System.nanoTime()
        val offline = runBlocking { core.runInBackground(5_000L) }
        val offlineMs = (System.nanoTime() - offlineStarted) / 1_000_000L
        check(offlineMs < 2_500L) { "runInBackground(5 s) offline took $offlineMs ms, not under half its deadline" }
        expectEq("finished of a run offline", false, offline.finished)
        expectEq("replayed of a run offline", 0, offline.replayed)
        check(offline.stillPending >= 1) {
            "stillPending of a run offline is ${offline.stillPending} (the report is $offline, storage_status().pending ${storageStatus().pending}, " +
                "stats().background ${core.stats().background})"
        }
        expectEq("the item is still queued", 1u, storageStatus().pending)

        // 3. Online: the run drains the queue.
        w.server.respond(HttpMethod.POST, url, 201, """{"id":9,"title":"Queued","done":false}""", delayMs = 400)
        w.server.respond(HttpMethod.GET, url, 200, """[{"id":9,"title":"Queued","done":false}]""")
        w.connectivity.changed(online = true, kind = NetKind.WIFI)
        val drained = runBlocking { core.runInBackground(10_000L) }
        check(drained.finished && drained.replayed == 1 && drained.stillPending == 0) {
            "the run that drained the queue reported $drained (storage_status().pending ${storageStatus().pending}, stats().background ${core.stats().background})"
        }
        expectEq("the creation resolved to the server's item", RemoteTodo(9u, "Queued", false), queued.get(WAIT_MS, TimeUnit.MILLISECONDS).getOrThrow())
        val posts = w.server.requestsFor(HttpMethod.POST, url)
        expectEq("POSTs the server saw", 2, posts.size)
        val keys = posts.map { it.header("Idempotency-Key") }
        check(keys[0] != null && keys[0] == keys[1]) { "the two POSTs did not carry the same Idempotency-Key: $keys" }
        expectEq("pending after the drain", 0u, storageStatus().pending)

        // 4. A run cut at its deadline leaves the work intact: back at about half a second before the deadline, nothing sent twice.
        w.connectivity.changed(online = false, kind = NetKind.NONE)
        Thread.sleep(50)
        w.server.fail(HttpMethod.POST, url, HttpError.Network("offline"))
        val slow = CompletableFuture<Result<RemoteTodo>>()
        scope.launch { slow.complete(runCatching { createRemoteTodo(list, "Slow") }) }
        awaitEq("storage_status().pending with the slow creation queued", 1u) { storageStatus().pending }
        w.server.respond(HttpMethod.POST, url, 201, """{"id":10,"title":"Slow","done":false}""", delayMs = 5_000)
        w.connectivity.changed(online = true, kind = NetKind.WIFI)
        // The run is cut at its deadline less the half second it keeps for the host: measured against a timer of that length armed beside it,
        // which a slow machine fires as late as the core's, and not against 900 ms of wall clock. A run that kept no half second, or waited for
        // the POST, ends 500 ms or more after the timer.
        val cutStarted = System.nanoTime()
        val reference = scope.async { delay(500L); System.nanoTime() }
        val cut = runBlocking { core.runInBackground(1_000L) }
        val cutEnded = System.nanoTime()
        val cutMs = (cutEnded - cutStarted) / 1_000_000L
        val lateMs = (cutEnded - runBlocking { reference.await() }) / 1_000_000L
        check(lateMs < 400L) { "runInBackground(1 s) returned $lateMs ms after a 500 ms timer armed beside it (it took $cutMs ms)" }
        check(cutMs >= 300L) { "runInBackground(1 s) returned after $cutMs ms: it did not use its window" }
        expectEq("finished of the run cut at its deadline", false, cut.finished)
        expectEq("replayed of the run cut at its deadline", 0, cut.replayed)
        expectEq("stillPending of the run cut at its deadline", 1, cut.stillPending)
        expectEq("the item is still queued after the cut run", 1u, storageStatus().pending)
        expectEq("the persisted queue still holds the item", 1L, w.kv.value(Persisted.QUEUE_KEY)?.let(Persisted::queueCount))
        awaitEq("storage_status().pending once the POST answers", 0u, timeoutMs = 10_000L) { storageStatus().pending }
        val slowPosts = w.server.requestsFor(HttpMethod.POST, url).filter { "Slow" in it.bodyText() }
        expectEq("POSTs of the slow item (the failed attempt and one replay)", 2, slowPosts.size)
        val slowKeys = slowPosts.map { it.header("Idempotency-Key") }
        check(slowKeys[0] != null && slowKeys[0] == slowKeys[1]) { "the slow item's POSTs did not carry the same Idempotency-Key: $slowKeys" }
        expectEq("the slow creation resolved", RemoteTodo(10u, "Slow", false), slow.get(WAIT_MS, TimeUnit.MILLISECONDS).getOrThrow())

        // 5. A host that cancels the call: the coroutine is cancelled, the core works on, the queue is intact.
        w.connectivity.changed(online = false, kind = NetKind.NONE)
        Thread.sleep(50)
        w.server.fail(HttpMethod.POST, url, HttpError.Network("offline"))
        val held = CompletableFuture<Result<RemoteTodo>>()
        scope.launch { held.complete(runCatching { createRemoteTodo(list, "Held") }) }
        awaitEq("storage_status().pending with the held creation queued", 1u) { storageStatus().pending }
        w.server.respond(HttpMethod.POST, url, 201, """{"id":11,"title":"Held","done":false}""", delayMs = 3_000)
        w.connectivity.changed(online = true, kind = NetKind.WIFI)
        val ended = CompletableFuture<Throwable?>()
        val run = scope.launch {
            try {
                core.runInBackground(30_000L)
                ended.complete(null)
            } catch (e: Throwable) {
                ended.complete(e)
                throw e
            }
        }
        Thread.sleep(100)
        val cancelStarted = System.nanoTime()
        runBlocking { run.cancelAndJoin() }
        val cancelMs = (System.nanoTime() - cancelStarted) / 1_000_000L
        // The cancel did not wait for the replay the run had started: its POST answers 3 s after it was sent, and the creation is still
        // waiting for it. (An order, not the 1 s of wall clock this was: a machine that stalls that long does not make the cancel wait.)
        val replayStillOut = !held.isDone
        val outcome = ended.get(WAIT_MS, TimeUnit.MILLISECONDS)
        check(outcome is CancellationException) { "the cancelled run ended with $outcome, not a CancellationException" }
        check(replayStillOut) { "cancelling the run waited $cancelMs ms, until the replay it had started answered" }
        expectEq("add(1, 2) after the run was cancelled", 3, add(1, 2))
        expectEq("the queue is intact after the cancel", 1u, storageStatus().pending)
        awaitEq("storage_status().pending once the held POST answers", 0u, timeoutMs = 10_000L) { storageStatus().pending }
        expectEq("the held creation resolved", RemoteTodo(11u, "Held", false), held.get(WAIT_MS, TimeUnit.MILLISECONDS).getOrThrow())

        // 6. Counters: four runs in this scenario, one of them finished, one item replayed by a run.
        val counters = core.stats().background
        expectEq("background.runs grown by the scenario", 4L, counters.runs - runsBefore)
        expectEq("background.finished grown by the scenario", 1L, counters.finished - finishedBefore)
        expectEq("background.replayed grown by the scenario", 1L, counters.replayed - replayedBefore)
    } finally {
        // Leave the core online with an empty queue, whatever happened.
        w.connectivity.changed(online = true, kind = NetKind.WIFI)
        LifecycleEvents(core).changed(AppState.ACTIVE)
        handle.close()
    }
    awaitEq("storage_status().pending at the end of S30", 0u) { storageStatus().pending }
}

/**
 * Step 1, the second half: going to the background writes a cache entry that waits out its 250 ms persistence debounce at once, rather than
 * leaving it to the debounce. Shown against the debounce's own clock, not a deadline of the machine's (100 ms was one, and a hosted runner
 * stalled past it): the list is fetched again with new contents, which makes its cache entry dirty and arms a debounce only after `armed` was
 * read, so a write of it seen less than 240 ms after `armed` (10 ms short, for the clocks' granularity) cannot be that debounce's. (The first fetch's own debounce is waited out first, so
 * it cannot be either.) A trial in which the machine stalled past that (the entry written before Background, or seen 240 ms or more after
 * `armed`) says nothing and is repeated with new contents, up to five times; a core that leaves the entry to its debounce writes it only once
 * the debounce could have fired, and fails all five.
 */
private fun backgroundFlushesAtOnce(w: World, handle: RemoteTodosQueryHandle, url: String, list: String) {
    val key = Persisted.cacheKey(UndraIds.Queries.REMOTE_TODOS, Codecs.string.encodeToByteArray(list))
    val writes = { w.kv.operations.count { it.isSet && it.key == key } }
    awaitUntil("the first fetch's cache entry to be written by its debounce") { writes() > 0 }
    val seen = ArrayList<String>()
    for (trial in 1..5) {
        val title = "Server $trial"
        w.server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"$title","done":false}]""")
        val armed = System.nanoTime()
        val atArmed = writes()
        handle.refetch()
        awaitUntil("the refetched list") { handle.data.value?.any { it.title == title } == true }
        val before = writes()
        if (before > atArmed) {
            // Its debounce wrote it already: the machine stalled 250 ms between the refetch and here.
            seen.add("before Background, ${(System.nanoTime() - armed) / 1_000_000L} ms")
            continue
        }
        LifecycleEvents(w.core).changed(AppState.BACKGROUND)
        val atMs = try {
            awaitUntil("the cache entry of the list to be written after Background") { writes() > before }
            (System.nanoTime() - armed) / 1_000_000L
        } finally {
            // Back to where the scenario's steps expect the app to be.
            LifecycleEvents(w.core).changed(AppState.ACTIVE)
        }
        // 10 ms short of the debounce: the clocks the core's timer and this read can disagree by their granularity (a millisecond).
        if (atMs < 240L) {
            w.server.respond(HttpMethod.GET, url, 200, "[]")
            return
        }
        seen.add("$atMs ms")
    }
    fail(
        "in five trials the cache entry was written only after the refetch that made it dirty ($seen), when its 250 ms debounce could have " +
            "fired: Background did not write it at once",
    )
}
