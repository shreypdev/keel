package dev.keel.contract

import dev.keel.playground.core.RemoteError
import dev.keel.playground.core.RemoteTodo
import dev.keel.playground.core.RemoteTodosQueryHandle
import dev.keel.playground.core.createRemoteTodo
import dev.keel.playground.core.setRemoteDone
import dev.keel.runtime.adapters.HttpError
import dev.keel.runtime.adapters.HttpMethod
import dev.keel.runtime.adapters.NetKind
import dev.keel.runtime.wire.KeelReader
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/** S14: an idempotent mutation made offline is queued, persisted and replayed, once, when the network returns. */
fun s14Offline(w: World) {
    w.configureRemoteOnce()
    val list = "s14"
    val url = "${World.BASE_URL}/lists/$list/todos"
    val offline = HttpError.Network("offline")
    w.server.respond(HttpMethod.GET, url, 200, "[]")
    val handle = RemoteTodosQueryHandle.create(list)
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

    // 6. The queue was persisted while offline, and emptied after the replay.
    val queueWrites = w.kv.writes.filter { it.key == QUEUE_KEY }
    check(queueWrites.any { write -> write.value?.let { queuedCount(it) } == 1 }) {
        "the core never wrote a queue of one mutation under $QUEUE_KEY: ${queueWrites.map { it.value?.size }}"
    }
    awaitUntil("the last write of $QUEUE_KEY to be an empty queue") {
        val last = w.kv.writes.lastOrNull { it.key == QUEUE_KEY }
        last != null && (last.value?.let { queuedCount(it) } ?: 0) == 0
    }
    handle.close()
}

/** The `Kv` key of the offline queue (`keel_query::QUEUE_KEY`). */
private const val QUEUE_KEY = "keel.query.queue"

/** How many mutations a persisted queue holds: `schema_hash u64, count u32, ...`. */
private fun queuedCount(queue: ByteArray): Int {
    val r = KeelReader(queue)
    r.readU64()
    return r.readU32().toInt()
}
