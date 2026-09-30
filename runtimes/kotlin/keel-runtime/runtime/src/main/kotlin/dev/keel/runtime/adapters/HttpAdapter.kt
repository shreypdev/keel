package dev.keel.runtime.adapters

import dev.keel.runtime.KeelPortException
import dev.keel.runtime.PortImpl
import dev.keel.runtime.wire.KeelReader
import dev.keel.runtime.wire.encodeToByteArray
import java.io.IOException
import java.net.URI
import java.net.URISyntaxException
import java.net.http.HttpClient
import java.net.http.HttpTimeoutException
import java.time.Duration
import java.util.Locale
import java.util.concurrent.CancellationException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CompletionException
import kotlin.coroutines.coroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.suspendCancellableCoroutine
import java.net.http.HttpRequest as JdkRequest
import java.net.http.HttpResponse as JdkResponse

/**
 * The `Http` port over `java.net.http.HttpClient` (JDK 11+; Android uses `android-adapters`).
 *
 * Redirects are followed (not from `https` to `http`). Headers the JDK manages itself (`Host`,
 * `Content-Length`, `Connection`, `Expect`, `Upgrade`) are dropped from requests. The request's
 * `timeoutMs` bounds the whole exchange. Failures map to [HttpError]: an unusable URL or header is
 * `InvalidUrl`, a timeout is `Timeout`, a cancelled call is `Cancelled`, anything else `Network`. A response
 * with an error status (404, 500, ...) is still a success: its status is in the response.
 *
 * @param client the client to use; the default is created on first use.
 */
public class HttpAdapter(client: HttpClient? = null) {
    private val client: HttpClient by lazy {
        client ?: HttpClient.newBuilder()
            .followRedirects(HttpClient.Redirect.NORMAL)
            .connectTimeout(Duration.ofSeconds(30))
            .build()
    }

    /**
     * Performs [request].
     *
     * @throws HttpError if the request fails.
     */
    public suspend fun request(request: HttpRequest): HttpResponse {
        val jdkRequest = build(request)
        val response = try {
            client.sendAsync(jdkRequest, JdkResponse.BodyHandlers.ofByteArray()).await()
        } catch (e: HttpTimeoutException) {
            throw HttpError.Timeout
        } catch (e: CancellationException) {
            // Our own coroutine being cancelled must stay a cancellation; anyone else cancelling the exchange is `Cancelled`.
            coroutineContext.ensureActive()
            throw HttpError.Cancelled
        } catch (e: IOException) {
            throw HttpError.Network(e.message ?: e.javaClass.simpleName)
        } catch (e: IllegalArgumentException) {
            throw HttpError.InvalidUrl(e.message ?: "invalid request")
        } catch (e: SecurityException) {
            throw HttpError.Network(e.message ?: "not permitted")
        }
        val headers = ArrayList<Header>()
        for ((name, values) in response.headers().map().toSortedMap(String.CASE_INSENSITIVE_ORDER)) {
            if (name.startsWith(":")) continue // HTTP/2 pseudo headers
            for (value in values) headers.add(Header(name, value))
        }
        return HttpResponse(response.statusCode().toUShort(), headers, response.body())
    }

    /** This adapter as an async [PortImpl] for [StandardPorts.Http]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Http.REQUEST] = { args ->
                val r = KeelReader(args)
                val decoded = HttpRequest.decode(r)
                r.finish()
                try {
                    HttpResponse.encodeToByteArray(request(decoded))
                } catch (e: HttpError) {
                    throw KeelPortException(HttpError.encodeToByteArray(e))
                }
            }
        },
    )

    private fun build(request: HttpRequest): JdkRequest {
        val uri = try {
            URI(request.url)
        } catch (e: URISyntaxException) {
            throw HttpError.InvalidUrl("${request.url}: ${e.reason}")
        }
        val scheme = uri.scheme?.lowercase(Locale.ROOT)
        if (scheme != "http" && scheme != "https") throw HttpError.InvalidUrl("${request.url}: only http and https URLs are supported")
        if (uri.host == null) throw HttpError.InvalidUrl("${request.url}: the URL has no host")
        val builder = JdkRequest.newBuilder(uri)
        request.timeoutMs?.let { builder.timeout(Duration.ofMillis(it.toLong())) }
        for (header in request.headers) {
            if (header.name.lowercase(Locale.ROOT) in RESTRICTED_HEADERS) continue
            try {
                builder.header(header.name, header.value)
            } catch (e: IllegalArgumentException) {
                throw HttpError.InvalidUrl("header '${header.name}' is not allowed: ${e.message}")
            }
        }
        val body = request.body
        val publisher = if (body == null) JdkRequest.BodyPublishers.noBody() else JdkRequest.BodyPublishers.ofByteArray(body)
        return builder.method(request.method.name, publisher).build()
    }

    private companion object {
        /** Headers `java.net.http` refuses to let callers set. */
        val RESTRICTED_HEADERS = setOf("connection", "content-length", "expect", "host", "upgrade")
    }
}

/** Suspends until this future completes; cancelling the coroutine cancels the future (and so the HTTP exchange). */
private suspend fun <T> CompletableFuture<T>.await(): T =
    suspendCancellableCoroutine { continuation ->
        whenComplete { value, failure ->
            if (failure == null) {
                continuation.resumeWith(Result.success(value))
            } else {
                continuation.resumeWith(Result.failure(if (failure is CompletionException && failure.cause != null) failure.cause!! else failure))
            }
        }
        continuation.invokeOnCancellation { cancel(true) }
    }
