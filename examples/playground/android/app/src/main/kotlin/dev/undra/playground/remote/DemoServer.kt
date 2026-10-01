package dev.undra.playground.remote

import dev.undra.runtime.UndraPortException
import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.net.URI
import kotlinx.coroutines.delay
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/**
 * The server behind the Remote tab, in memory, answering as the core expects (the routes of `remote.rs`):
 *
 * | Request | Answer |
 * |---|---|
 * | `GET /lists/{list}/todos` | 200, a JSON array of `{"id", "title", "done"}` |
 * | `POST /lists/{list}/todos` with `{"title"}` | 201, the created item; a repeat with the same `Idempotency-Key` returns the same item |
 * | `PATCH /lists/{list}/todos/{id}` with `{"done"}` | 200, the item; 404 if there is none |
 *
 * Every answer takes [LATENCY_MS] milliseconds, so optimistic updates are visible. While [offline] is set the
 * server fails every request with `HttpError.Network`, which is what the core sees on a device without a
 * network. The `inbox` list starts with three items.
 */
class DemoServer {
    /** Whether the network is down: every request then fails with `HttpError.Network("offline")`. */
    @Volatile
    var offline: Boolean = false

    private class Todo(val id: Int, val title: String, var done: Boolean) {
        fun toJson(): JSONObject = JSONObject().put("id", id).put("title", title).put("done", done)
    }

    private val lock = Any()
    private val lists = HashMap<String, MutableList<Todo>>()
    private val created = HashMap<String, Todo>() // by Idempotency-Key
    private var nextId = 1

    init {
        for (title in listOf("Buy milk", "Walk the dog", "Write Undra")) add("inbox", title)
    }

    /** This server as an async `Http` port. */
    fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Http.REQUEST] = { args ->
                try {
                    HttpResponse.encodeToByteArray(answer(HttpRequest.decodeAll(args)))
                } catch (e: HttpError) {
                    // A port fails with its typed error; the core turns it into RemoteError.Http.
                    throw UndraPortException(HttpError.encodeToByteArray(e))
                }
            }
        },
    )

    private suspend fun answer(request: HttpRequest): HttpResponse {
        if (offline) throw HttpError.Network("offline")
        delay(LATENCY_MS)
        if (offline) throw HttpError.Network("offline") // the network went away while the request was in flight
        val route = ROUTE.matchEntire(URI(request.url).path ?: "") ?: return reply(404, "not found")
        val list = route.groupValues[1]
        val id = route.groupValues[2].toIntOrNull()
        return try {
            when {
                request.method == HttpMethod.GET && id == null -> reply(200, listJson(list))
                request.method == HttpMethod.POST && id == null -> create(list, request)
                request.method == HttpMethod.PATCH && id != null -> patch(list, id, request)
                else -> reply(405, "method not allowed")
            }
        } catch (e: JSONException) {
            reply(400, "bad request: ${e.message}")
        }
    }

    private fun listJson(list: String): String = synchronized(lock) {
        JSONArray(lists[list].orEmpty().map { it.toJson() }).toString()
    }

    private fun create(list: String, request: HttpRequest): HttpResponse {
        val title = JSONObject(body(request)).getString("title")
        val key = request.headers.firstOrNull { it.name.equals("Idempotency-Key", ignoreCase = true) }?.value
        val todo = synchronized(lock) {
            // A replay of a request the server already handled (the core queued it while offline) is not a second item.
            key?.let { created[it] } ?: add(list, title).also { if (key != null) created[key] = it }
        }
        return reply(201, todo.toJson().toString())
    }

    private fun patch(list: String, id: Int, request: HttpRequest): HttpResponse {
        val done = JSONObject(body(request)).getBoolean("done")
        val todo = synchronized(lock) { lists[list]?.firstOrNull { it.id == id }?.also { it.done = done } }
        return if (todo == null) reply(404, "no item $id in $list") else reply(200, todo.toJson().toString())
    }

    private fun add(list: String, title: String): Todo = synchronized(lock) {
        Todo(nextId++, title, done = false).also { lists.getOrPut(list) { mutableListOf() }.add(it) }
    }

    private fun body(request: HttpRequest): String = (request.body ?: ByteArray(0)).toString(Charsets.UTF_8)

    private fun reply(status: Int, body: String): HttpResponse =
        HttpResponse(status.toUShort(), listOf(Header("Content-Type", "application/json")), body.toByteArray(Charsets.UTF_8))

    /** Where the server is, and how long it takes to answer. */
    companion object {
        /** The address the app passes to `configureRemote`; nothing is ever sent to it, the port answers in process. */
        const val BASE_URL: String = "https://playground.undra.test"

        /** Every answer takes this long. */
        const val LATENCY_MS: Long = 300L

        private val ROUTE = Regex("^/lists/([^/]+)/todos(?:/(\\d+))?$")
    }
}
