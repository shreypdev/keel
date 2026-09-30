package dev.keel.contract

import dev.keel.runtime.KeelPortException
import dev.keel.runtime.PortImpl
import dev.keel.runtime.adapters.Header
import dev.keel.runtime.adapters.HttpError
import dev.keel.runtime.adapters.HttpMethod
import dev.keel.runtime.adapters.HttpRequest
import dev.keel.runtime.adapters.HttpResponse
import dev.keel.runtime.adapters.StandardPorts
import dev.keel.runtime.wire.decodeAll
import dev.keel.runtime.wire.encodeToByteArray
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.delay

/**
 * The in-memory server behind the `Http` port (scenarios.md, "Adapters"): routes by method and exact
 * URL, records every request, can hold a reply back for some milliseconds and can answer with a
 * network error. An unknown route answers 404.
 *
 * A route keeps answering until it is replaced (the last [respond] or [fail] for a method and URL wins),
 * so a scenario changes what the server says by scripting the route again.
 */
class FakeServer {
    /** A request the server received, as the core sent it. */
    class Request(val method: HttpMethod, val url: String, val headers: List<Header>, val body: ByteArray?) {
        /** The value of the header [name], compared without regard to case, or `null`. */
        fun header(name: String): String? = headers.firstOrNull { it.name.equals(name, ignoreCase = true) }?.value

        /** The body as text. */
        fun bodyText(): String = body?.toString(Charsets.UTF_8) ?: ""
    }

    private sealed interface Reply {
        val delayMs: Long

        class Answer(val status: Int, val body: ByteArray, override val delayMs: Long) : Reply

        class Failure(val error: HttpError, override val delayMs: Long) : Reply
    }

    private val routes = ConcurrentHashMap<String, Reply>()

    /** Every request so far, oldest first. */
    val requests = CopyOnWriteArrayList<Request>()

    /** Answers [method] [url] with [status] and [body], after [delayMs] milliseconds. */
    fun respond(method: HttpMethod, url: String, status: Int, body: String = "", delayMs: Long = 0L) {
        routes[key(method, url)] = Reply.Answer(status, body.toByteArray(Charsets.UTF_8), delayMs)
    }

    /** Makes [method] [url] fail with [error] (the port's typed failure, such as `HttpError.Network("offline")`). */
    fun fail(method: HttpMethod, url: String, error: HttpError, delayMs: Long = 0L) {
        routes[key(method, url)] = Reply.Failure(error, delayMs)
    }

    /** How many requests with [method] arrived for [url]. */
    fun count(method: HttpMethod, url: String): Int = requests.count { it.method == method && it.url == url }

    /** The requests with [method] for [url], oldest first. */
    fun requestsFor(method: HttpMethod, url: String): List<Request> = requests.filter { it.method == method && it.url == url }

    /** This server as an async `Http` port. */
    fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Http.REQUEST] = { args ->
                val request = HttpRequest.decodeAll(args)
                requests.add(Request(request.method, request.url, request.headers, request.body))
                when (val reply = routes[key(request.method, request.url)]) {
                    null -> HttpResponse.encodeToByteArray(HttpResponse(404u, emptyList(), ByteArray(0)))
                    is Reply.Answer -> {
                        if (reply.delayMs > 0) delay(reply.delayMs)
                        HttpResponse.encodeToByteArray(HttpResponse(reply.status.toUShort(), emptyList(), reply.body))
                    }
                    is Reply.Failure -> {
                        if (reply.delayMs > 0) delay(reply.delayMs)
                        throw KeelPortException(HttpError.encodeToByteArray(reply.error))
                    }
                }
            }
        },
    )

    private fun key(method: HttpMethod, url: String) = "${method.name} $url"
}
