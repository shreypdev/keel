package dev.undra.okhttp

import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.io.ByteArrayOutputStream
import java.io.IOException
import kotlin.coroutines.coroutineContext
import kotlin.coroutines.resume
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.Call
import okhttp3.Callback
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody
import okio.BufferedSource

/**
 * The `Http` port over the app's own [OkHttpClient] (ADR-060): every request the core makes goes through [client], so its
 * interceptors, `Authenticator` (token refresh), event listeners (tracing), certificate pinner, `Dns`, proxy, cookie jar, cache,
 * connection pool and dispatcher apply to it with nothing Undra-specific configured. This is the adapter to use when the app has
 * a network stack of its own; `AndroidHttpAdapter` (`HttpURLConnection`, the platform's own OkHttp, which an app's client cannot
 * see) stays the default for apps that have none.
 *
 * It keeps the contract of the other Http adapters, and the shared suite that checks it (`HttpAdapterContract`) runs on this one:
 *
 *  - A response with an error status (404, 500, ...) is still a success; its status and body are in the response.
 *  - Failures are typed ([HttpError], ADR-025): an unusable URL, header or method is `InvalidUrl`, an expired timeout (the
 *    request's, or the client's own connect, read or call timeout) is `Timeout`, a call cancelled by anyone but the caller is
 *    `Cancelled`, everything else (no route, refused or reset connection, TLS failure or a pin that does not match, cleartext
 *    traffic the client's `ConnectionSpec` or the app's network security config forbids, too many redirects, ...) is `Network` with
 *    OkHttp's description. Only `http` and `https` URLs are accepted.
 *  - `HttpRequest.timeoutMs` bounds the whole exchange: connecting, sending, every redirect and reading the body (a coroutine
 *    timeout over the call, not a `callTimeout` of the client, so a client with one of its own keeps it). Without one the client's
 *    timeouts apply (OkHttp's default: 10 s each for connect, read and write, no call timeout).
 *  - **Cancelling the calling coroutine cancels the call**: OkHttp closes the connection from the cancelling thread, so a blocked
 *    read ends at once, and the caller sees `CancellationException`, as with every suspend call.
 *  - The response body is read in chunks and held in memory; one larger than [maxResponseBytes] fails with `Network` instead of
 *    exhausting the heap. A request body is sent whole with its length (`Content-Length`).
 *  - Headers the connection manages itself (`Host`, `Content-Length`, `Connection`, `Expect`, `Upgrade`) are dropped from requests.
 *    A header value must be printable ASCII (OkHttp's rule): anything else is `InvalidUrl`, naming the header.
 *
 * What is the client's, and so the app's, not this adapter's:
 *
 *  - **Redirects**: OkHttp's own (`followRedirects`, `followSslRedirects`; at most 20, then `Network("too many redirects ...")`; `301`
 *    and `302` turn a `POST` into a `GET`, `303` turns everything but `HEAD` into one, `307` and `308` repeat the request; the
 *    `Authorization` header is not sent to another host). The default client follows `https` to `http`; the other adapters never
 *    do: `followSslRedirects(false)` on the app's client gives the same rule here.
 *  - **Retries**: `retryOnConnectionFailure`, and the `Authenticator` on `401` and `407`.
 *  - **Compression and caching**: transparent gzip (OkHttp adds `Accept-Encoding: gzip` and decodes, unless the request names an
 *    encoding itself), and the client's `Cache` and `CookieJar` if it has them (the other adapters have neither).
 *  - **Concurrency**: the client's `Dispatcher` limits (OkHttp's default: 64 calls, 5 per host) apply to the core's requests, which
 *    wait their turn with the app's.
 *
 * Needs `android.permission.INTERNET`, which this library's manifest declares. A request runs on OkHttp's own threads, never the
 * calling one.
 *
 * @param client the app's client, asked for on every request, so an app that replaces its client (after a login, say) is followed.
 * @param maxResponseBytes the largest response body accepted.
 */
public class OkHttpHttpAdapter(
    private val client: () -> OkHttpClient,
    private val maxResponseBytes: Int = DEFAULT_MAX_RESPONSE_BYTES,
) {
    /** An adapter over [client] with the given size limit; see the class documentation. */
    public constructor(client: OkHttpClient, maxResponseBytes: Int = DEFAULT_MAX_RESPONSE_BYTES) : this({ client }, maxResponseBytes)

    init {
        require(maxResponseBytes > 0) { "maxResponseBytes must be positive, got $maxResponseBytes" }
    }

    /**
     * Performs [request].
     *
     * @throws HttpError if the request fails.
     * @throws CancellationException if the calling coroutine is cancelled (the call is cancelled first).
     */
    public suspend fun request(request: HttpRequest): HttpResponse {
        val url = OkHttpRules.parseUrl(request.url)
        OkHttpRules.checkBody(request.method, request.body)
        val built = OkHttpRules.build(url, request)
        val outcome = try {
            val total = request.timeoutMs
            // withTimeoutOrNull answers null only for its own timeout; a caller's cancellation stays a cancellation.
            if (total == null) {
                exchange(built, request.method)
            } else {
                withTimeoutOrNull(total.toLong()) { exchange(built, request.method) } ?: throw HttpError.Timeout
            }
        } catch (e: CancellationException) {
            // Our own coroutine being cancelled must stay a cancellation; anyone else cancelling the exchange is `Cancelled`.
            coroutineContext.ensureActive()
            throw HttpError.Cancelled
        } catch (e: HttpError) {
            throw e
        } catch (e: Exception) {
            coroutineContext.ensureActive() // a failure our own cancellation caused is the cancellation
            throw OkHttpRules.failure(e)
        }
        return when (outcome) {
            is Outcome.Done -> outcome.response
            is Outcome.Failed -> throw outcome.error
        }
    }

    /** This adapter as an async [PortImpl] for [StandardPorts.Http]; a failure answers with the typed [HttpError]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = mapOf<UInt, suspend (ByteArray) -> ByteArray>(
            StandardPorts.Http.REQUEST to { args ->
                val decoded = HttpRequest.decodeAll(args)
                try {
                    HttpResponse.encodeToByteArray(this@OkHttpHttpAdapter.request(decoded))
                } catch (e: HttpError) {
                    throw UndraPortException(HttpError.encodeToByteArray(e))
                }
            },
        ),
    )

    /** How a call ended. A failure travels as a value and is thrown by [request]: an exception resumed through a coroutine can be
     *  rebuilt by stack trace recovery (under `-ea`), which would put its own text into the typed error's reason. */
    private sealed interface Outcome {
        class Done(val response: HttpResponse) : Outcome

        class Failed(val error: HttpError) : Outcome
    }

    /**
     * One call through the client. `enqueue`, so that the client's dispatcher decides when it runs; the body is read in the
     * callback, on the thread OkHttp gave the call, so one thread serves the call from its start to the last byte. Cancelling the
     * caller cancels the call, which closes the connection and ends a blocked read.
     */
    private suspend fun exchange(request: Request, method: HttpMethod): Outcome =
        suspendCancellableCoroutine { continuation ->
            val call = try {
                client().newCall(request)
            } catch (e: Exception) {
                continuation.resume(Outcome.Failed(OkHttpRules.failure(e))) // the app's own client provider failed
                return@suspendCancellableCoroutine
            }
            continuation.invokeOnCancellation { call.cancel() }
            call.enqueue(object : Callback {
                override fun onFailure(call: Call, e: IOException) {
                    // A resume after the caller's cancellation is ignored.
                    continuation.resume(Outcome.Failed(OkHttpRules.failure(e, call.isCanceled())))
                }

                override fun onResponse(call: Call, response: Response) {
                    val outcome = try {
                        Outcome.Done(response.use { read(it, method) })
                    } catch (e: Exception) {
                        Outcome.Failed(OkHttpRules.failure(e, call.isCanceled()))
                    }
                    continuation.resume(outcome)
                }
            })
        }

    private fun read(response: Response, method: HttpMethod): HttpResponse {
        val headers = OkHttpRules.responseHeaders(response.headers)
        // Nullable by declaration: OkHttp 4's `body` is, 5's is not (a response of a call always has one).
        val body: ResponseBody? = response.body
        val bytes = when {
            body == null || method == HttpMethod.HEAD -> NO_BODY
            else -> readBounded(body.source(), body.contentLength())
        }
        return HttpResponse(response.code.toUShort(), headers, bytes)
    }

    /** Reads [source] to its end in chunks, failing as soon as it is known to exceed [maxResponseBytes]. */
    private fun readBounded(source: BufferedSource, declared: Long): ByteArray {
        if (declared > maxResponseBytes) throw HttpError.Network("the response body ($declared bytes) is larger than the limit of $maxResponseBytes bytes")
        val out = ByteArrayOutputStream(if (declared in 1..INITIAL_BUFFER_LIMIT) declared.toInt() else CHUNK)
        val buffer = ByteArray(CHUNK)
        var total = 0L
        while (true) {
            val n = source.read(buffer)
            if (n < 0) break
            total += n
            if (total > maxResponseBytes) throw HttpError.Network("the response body is larger than the limit of $maxResponseBytes bytes")
            out.write(buffer, 0, n)
        }
        return out.toByteArray()
    }

    /** Defaults and limits. */
    public companion object {
        /** The largest response body accepted by default: 64 MiB. */
        public const val DEFAULT_MAX_RESPONSE_BYTES: Int = 64 * 1024 * 1024

        private const val CHUNK = 8 * 1024
        private const val INITIAL_BUFFER_LIMIT = 1L * 1024 * 1024
        private val NO_BODY = ByteArray(0)
    }
}
