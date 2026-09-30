package dev.keel.contract

import dev.keel.playground.core.QueryStatus
import dev.keel.playground.core.RemoteError
import dev.keel.playground.core.RemoteTodo
import dev.keel.playground.core.RemoteTodosQueryHandle
import dev.keel.playground.core.createRemoteTodo
import dev.keel.playground.core.setRemoteDone
import dev.keel.runtime.adapters.HttpMethod
import java.util.UUID
import kotlinx.coroutines.runBlocking

/** S13: a mutation shows its effect at once, and takes it back if the server says no. */
fun s13Optimistic(w: World) {
    w.configureRemoteOnce()
    val list = "s13"
    val url = "${World.BASE_URL}/lists/$list/todos"
    val milk = RemoteTodo(1u, "Buy milk", false)
    w.server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"Buy milk","done":false}]""")
    w.server.respond(HttpMethod.POST, url, 500, "no", delayMs = 50)
    w.server.respond(HttpMethod.PATCH, "$url/1", 500, "no", delayMs = 50)

    val handle = RemoteTodosQueryHandle.create(list)
    awaitEq("the data of the list", listOf(milk)) { handle.data.value }
    Recorder(handle.data).use { data ->
        /** The values of `data` after the first [skip], without the list that was there before. */
        fun seen(skip: Int): List<List<RemoteTodo>?> = data.values.drop(skip)

        // 1. A refused creation: the placeholder shows for the 50 ms the server takes, then the list is as it was.
        var start = data.values.size
        val placeholder = RemoteTodo(UInt.MAX_VALUE, "Walk", false)
        val refused = expectFailsAsync<RemoteError.Status>("create_remote_todo while POST answers 500") { createRemoteTodo(list, "Walk") }
        expectEq("the status the creation failed with", 500.toUShort(), refused.code)
        awaitEq("data while a creation is refused", listOf<List<RemoteTodo>?>(listOf(milk, placeholder), listOf(milk))) { seen(start) }
        awaitEq("the query status after the rollback", QueryStatus.SUCCESS) { handle.status.value }

        // 2. The server accepts it: placeholder first, then the server's own list after the refetch.
        val walk = RemoteTodo(2u, "Walk", false)
        w.server.respond(HttpMethod.POST, url, 201, """{"id":2,"title":"Walk","done":false}""", delayMs = 50)
        w.server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"Buy milk","done":false},{"id":2,"title":"Walk","done":false}]""")
        start = data.values.size
        expectEq("the todo create_remote_todo returned", walk, runBlocking { createRemoteTodo(list, "Walk") })
        awaitEq(
            "data while a creation is accepted",
            listOf<List<RemoteTodo>?>(listOf(milk, RemoteTodo(UInt.MAX_VALUE - 1u, "Walk", false)), listOf(milk, walk)),
        ) { seen(start) }
        val post = w.server.requestsFor(HttpMethod.POST, url).last()
        val key = post.header("Idempotency-Key") ?: fail("the POST carried no Idempotency-Key: ${post.headers}")
        check(runCatching { UUID.fromString(key) }.isSuccess) { "the Idempotency-Key is not a UUID: $key" }
        expectEq("the body of the POST", """{"title":"Walk"}""", post.bodyText())

        // 3. A refused toggle: done flips at once and flips back.
        start = data.values.size
        val refusedToggle = expectFailsAsync<RemoteError.Status>("set_remote_done while PATCH answers 500") { setRemoteDone(list, 1u, true) }
        expectEq("the status the toggle failed with", 500.toUShort(), refusedToggle.code)
        awaitEq(
            "data while a toggle is refused",
            listOf<List<RemoteTodo>?>(listOf(milk.copy(done = true), walk), listOf(milk, walk)),
        ) { seen(start) }

        // 4. An accepted toggle: after the refetch the list has done = true.
        w.server.respond(HttpMethod.PATCH, "$url/1", 200, """{"id":1,"title":"Buy milk","done":true}""", delayMs = 50)
        w.server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"Buy milk","done":true},{"id":2,"title":"Walk","done":false}]""")
        expectEq("the todo set_remote_done returned", milk.copy(done = true), runBlocking { setRemoteDone(list, 1u, true) })
        awaitEq("the list after the toggle was accepted", listOf(milk.copy(done = true), walk)) { handle.data.value }
    }
    handle.close()
}
