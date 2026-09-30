package dev.keel.contract

import dev.keel.playground.core.QueryStatus
import dev.keel.playground.core.RemoteError
import dev.keel.playground.core.RemoteTodo
import dev.keel.playground.core.RemoteTodosQueryHandle
import dev.keel.runtime.wire.Timestamp
import dev.keel.runtime.adapters.HttpMethod

/** S12: one fetch is shared, fresh for 30 s, refetched on demand, and its failures are typed values. */
fun s12Query(w: World) {
    w.configureRemoteOnce()
    val list = "s12"
    val url = "${World.BASE_URL}/lists/$list/todos"
    val milk = RemoteTodo(1u, "Buy milk", false)
    val walk = RemoteTodo(2u, "Walk the dog", false)
    val gets = { w.server.count(HttpMethod.GET, url) }
    w.server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"Buy milk","done":false}]""", delayMs = 50)
    val handles = w.stats().liveHandles

    // 1. The first handle starts a fetch: fetching first, then success with the list and the time of the fetch.
    val first = RemoteTodosQueryHandle.create(list)
    val fetchedAt = w.clock.nowMs
    Recorder(first.status).use { statuses ->
        awaitEq("the status after the first fetch", QueryStatus.SUCCESS) { first.status.value }
        check(statuses.values.first() in listOf(QueryStatus.IDLE, QueryStatus.FETCHING)) {
            "the first status was ${statuses.values.first()}, not idle or fetching"
        }
        check(QueryStatus.ERROR !in statuses.values) { "the first fetch went through an error: ${statuses.values}" }
    }
    awaitEq("the data of the first fetch", listOf(milk)) { first.data.value }
    expectEq("the error after a success", null, first.error.value)
    awaitEq("fetching after the first fetch", false) { first.fetching.value }
    expectEq("updatedAt after the first fetch", Timestamp(fetchedAt), first.updatedAt.value)
    expectEq("GET count after the first fetch", 1, gets())

    // 2. Ten seconds later a second handle finds the data at once, and no request is made (the window is 30 s).
    w.clock.advance(10_000)
    val second = RemoteTodosQueryHandle.create(list)
    expectEq("the data of a second handle, at once", listOf(milk), second.data.value)
    expectEq("the status of a second handle, at once", QueryStatus.SUCCESS, second.status.value)
    holdsFor("the GET count inside the fresh window", 200) { gets() == 1 }
    expectEq("the data of the first handle", second.data.value, first.data.value)

    // 3. 41 seconds after the fetch the data is stale: a third handle fetches, and all three see the new list.
    w.clock.advance(31_000)
    w.server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"Buy milk","done":false},{"id":2,"title":"Walk the dog","done":false}]""", delayMs = 50)
    val third = RemoteTodosQueryHandle.create(list)
    val secondFetchAt = w.clock.nowMs
    val all = listOf(first, second, third)
    for ((i, handle) in all.withIndex()) {
        awaitEq("the data of handle ${i + 1} after the stale fetch", listOf(milk, walk)) { handle.data.value }
        awaitEq("updatedAt of handle ${i + 1} after the stale fetch", Timestamp(secondFetchAt)) { handle.updatedAt.value }
    }
    expectEq("GET count after the stale fetch", 2, gets())
    check(Timestamp(secondFetchAt) > Timestamp(fetchedAt)) { "updatedAt did not move forward" }
    awaitEq("fetching after the stale fetch", false) { third.fetching.value }

    // 4. refetch() goes to the server although the data is fresh.
    second.refetch()
    awaitUntil("GET count 3 after refetch()") { gets() == 3 }
    awaitEq("the status after refetch()", QueryStatus.SUCCESS) { first.status.value }
    awaitEq("fetching after refetch()", false) { first.fetching.value }

    // 5. invalidate() marks the entry stale and, because it is observed, refetches it.
    third.invalidate()
    awaitUntil("GET count 4 after invalidate()") { gets() == 4 }
    awaitEq("fetching after invalidate()", false) { first.fetching.value }

    // 6. A 503: after the core's one retry (about a second of backoff on a real timer) the error is typed and the old data stays.
    w.server.respond(HttpMethod.GET, url, 503, "down")
    first.refetch()
    awaitEq("the error after a 503", RemoteError.Status(503u)) { first.error.value }
    awaitEq("the status after a 503", QueryStatus.ERROR) { first.status.value }
    expectEq("the data after a 503", listOf(milk, walk), first.data.value)

    // 7. A body that is not JSON.
    w.server.respond(HttpMethod.GET, url, 200, "not json")
    first.refetch()
    awaitUntil("a BadBody error after a body that is not JSON") { first.error.value is RemoteError.BadBody }
    expectEq("the data after a bad body", listOf(milk, walk), first.data.value)

    // 8. Releasing every handle takes them out of the core.
    all.forEach { it.close() }
    expectEq("live_handles after releasing the three handles", handles, w.stats().liveHandles)
}
