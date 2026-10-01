package dev.undra.contract

import dev.undra.playground.core.RemoteError
import dev.undra.playground.core.RemoteTodo
import dev.undra.playground.core.RemoteTodosQueryHandle
import dev.undra.playground.core.createRemoteTodo
import dev.undra.playground.core.saveNote
import dev.undra.playground.core.setRemoteDone
import dev.undra.playground.core.storageStatus
import dev.undra.playground.core.tagNote
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.NetKind
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/**
 * S14: an idempotent mutation made offline is queued, persisted (format 2, ADR-037) and replayed, once, when the network
 * returns; then build A queues two notes that build B changes and hands the `Kv` over to the build-B process
 * ([migrationBuildB], steps 8 and 9).
 */
fun s14Offline(w: World) {
    w.configureRemoteOnce()
    Handover.discard("s14")
    // The harness failed the first read of the queue (S19 step 4); the client reads it again after a backoff.
    awaitUntil("the offline queue to be readable") { storageStatus().queueReadable }
    val list = "s14"
    w.server.respond(HttpMethod.GET, "${World.BASE_URL}/lists/$list/todos", 200, "[]")
    val handle = RemoteTodosQueryHandle.create(list)
    // Closed however the steps end, so that a failure here does not leave a handle that later counts (S15.8) see.
    try {
        queueAndReplay(w, list, handle)
    } finally {
        handle.close()
    }
    notesForBuildB(w)
}

/** Steps 1 to 6: a creation queued offline, persisted, replayed once online with the same idempotency key. */
private fun queueAndReplay(w: World, list: String, handle: RemoteTodosQueryHandle) {
    val url = "${World.BASE_URL}/lists/$list/todos"
    val offline = HttpError.Network("offline")
    awaitEq("the data of the empty list", emptyList<RemoteTodo>()) { handle.data.value }

    // 1. The network goes away.
    w.connectivity.changed(online = false, kind = NetKind.NONE)
    Thread.sleep(50)

    // 2. A creation fails on the wire, is queued, and keeps the caller waiting with the placeholder showing.
    w.server.fail(HttpMethod.POST, url, offline)
    val created = CompletableFuture<Result<RemoteTodo>>()
    val scope = CoroutineScope(Dispatchers.Default)
    scope.launch { created.complete(runCatching { createRemoteTodo(list, "Offline item") }) }
    awaitEq("the placeholder of the offline item", listOf("Offline item")) { handle.data.value?.map { it.title } }
    holdsFor("the creation, which is queued and must not finish", 200) { !created.isDone }
    expectEq("POSTs the server saw while the creation waits", 1, w.server.count(HttpMethod.POST, url))

    // 3. A mutation that is not idempotent is not queued: it fails at once with the network error.
    w.server.fail(HttpMethod.PATCH, "$url/1", offline)
    val started = System.nanoTime()
    val refused = expectFailsAsync<RemoteError.Http>("set_remote_done while offline") { setRemoteDone(list, 1u, true) }
    expectEq("the network error of a non-idempotent mutation", offline, refused.cause)
    check((System.nanoTime() - started) / 1_000_000L < 2_000) { "the non-idempotent mutation did not fail at once" }

    // 4. The network returns and the server accepts the replay.
    val whileOffline = w.kv.operations
    w.server.respond(HttpMethod.POST, url, 201, """{"id":9,"title":"Offline item","done":false}""")
    w.server.respond(HttpMethod.GET, url, 200, """[{"id":9,"title":"Offline item","done":false}]""")
    w.connectivity.changed(online = true, kind = NetKind.WIFI)

    // 5. The waiting call resolves to the server's item; the request was sent twice with the same key.
    val item = created.get(WAIT_MS, TimeUnit.MILLISECONDS).getOrThrow()
    expectEq("the todo the queued creation resolved to", RemoteTodo(9u, "Offline item", false), item)
    val posts = w.server.requestsFor(HttpMethod.POST, url)
    expectEq("POSTs the server saw in all", 2, posts.size)
    val keys = posts.map { it.header("Idempotency-Key") }
    check(keys[0] != null && keys[0] == keys[1]) { "the two POSTs did not carry the same Idempotency-Key: $keys" }
    awaitEq("the data after the replay", listOf(RemoteTodo(9u, "Offline item", false))) { handle.data.value }

    // 6. The queue was persisted while offline (format 2, after the description of `create`'s input), and emptied
    // after the replay (the core deletes the key; an empty queue would do too).
    val queueKey = Persisted.QUEUE_KEY
    val firstWrite = whileOffline.indexOfFirst { it.isSet && it.key == queueKey && (it.value?.let(Persisted::queueCount) ?: 0L) > 0L }
    check(firstWrite >= 0) { "the core never wrote a queue under $queueKey while offline: ${whileOffline.filter { it.key == queueKey }}" }
    val queued = whileOffline[firstWrite].value!!
    expectEq("the count of the queue written while offline (format 2: the u32 at offset 10)", 1L, Persisted.queueCount(queued))
    // The item: `mutation_id u32, fingerprint u64, ..` after `format u16, schema_hash u64, count u32`.
    val fingerprint = Persisted.fingerprint(queued, 18) ?: fail("the queued item has no fingerprint: ${queued.size} bytes")
    val typesKey = Persisted.typesKey(fingerprint)
    check(whileOffline.subList(0, firstWrite).any { it.isSet && it.key == typesKey }) {
        "$typesKey was not written before the queue that needs it: ${whileOffline.subList(0, firstWrite)}"
    }
    awaitUntil("the queue to be emptied") {
        val last = w.kv.operations.lastOrNull { it.key == queueKey && it.failure == null && it.kind != MemoryKv.Kind.GET }
        last != null && Persisted.emptiesQueue(last)
    }
}

/**
 * Step 7: build A queues what build B changes. Two notes wait offline, and what they left in the `Kv` is handed over
 * to the build-B process with the failed POST's idempotency key.
 */
private fun notesForBuildB(w: World) {
    val offline = HttpError.Network("offline")
    val queueKey = Persisted.QUEUE_KEY
    val scope = CoroutineScope(Dispatchers.Default)
    val notes = "${World.BASE_URL}/lists/s14m/notes"
    w.connectivity.changed(online = false, kind = NetKind.NONE)
    Thread.sleep(50)
    w.server.fail(HttpMethod.POST, notes, offline)
    val saved = CompletableFuture<Result<Boolean>>()
    val tagged = CompletableFuture<Result<Boolean>>()
    scope.launch { saved.complete(runCatching { saveNote("s14m", "a") }) }
    scope.launch { tagged.complete(runCatching { tagNote("s14m", 7u) }) }
    awaitUntil("both notes to wait in the queue") { storageStatus().pending == 2u }
    awaitUntil("the queue of two to be persisted") { w.kv.value(queueKey)?.let(Persisted::queueCount) == 2L }
    check(!saved.isDone && !tagged.isDone) { "a queued note finished while offline: save_note $saved, tag_note $tagged" }
    val attempt = w.server.requestsFor(HttpMethod.POST, notes).firstOrNull { it.bodyText() == "save:a" }
        ?: fail("the failed POST of save_note: ${w.server.requestsFor(HttpMethod.POST, notes).map { it.bodyText() }}")
    val idempotencyKey = attempt.header("Idempotency-Key") ?: fail("save_note's POST carried no Idempotency-Key")
    Handover.write(Handover.Queue(w.kv.entries, idempotencyKey))

    // Build A's own run is left clean: the notes replay here too, so later scenarios (and S17's reload) find an empty
    // queue. Build B starts from the contents kept above.
    w.server.respond(HttpMethod.POST, notes, 201, "{}")
    w.connectivity.changed(online = true, kind = NetKind.WIFI)
    expectEq("save_note after the replay in build A", true, saved.get(WAIT_MS, TimeUnit.MILLISECONDS).getOrThrow())
    expectEq("tag_note after the replay in build A", true, tagged.get(WAIT_MS, TimeUnit.MILLISECONDS).getOrThrow())
    awaitUntil("build A's queue to be empty") { storageStatus().pending == 0u }
}
