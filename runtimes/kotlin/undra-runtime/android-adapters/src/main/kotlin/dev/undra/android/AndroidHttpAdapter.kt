package dev.undra.android

import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.ProtocolException
import java.net.URL
import java.net.URLConnection
import kotlin.coroutines.coroutineContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withTimeoutOrNull

/**
 * The `Http` port over `java.net.HttpURLConnection` (`java.net.http` does not exist on Android).
 *
 * Every request runs on `Dispatchers.IO`, never on the thread that asked, so it is safe to call from the main
 * thread's coroutines. The behaviour is that of the JVM adapter (`dev.undra.runtime.adapters.HttpAdapter`) and of the
 * Swift and TypeScript ones:
 *
 *  - A response with an error status (404, 500, ...) is still a success; its status and body are in the response.
 *  - Failures are typed ([HttpError], ADR-025): an unusable URL, header or method is `InvalidUrl`, an expired timeout
 *    is `Timeout`, an interrupted transfer is `Cancelled`, everything else (no route, refused or reset connection,
 *    TLS failure, cleartext traffic the app's network security config forbids, ...) is `Network` with the platform's
 *    description. Only `http` and `https` URLs are accepted.
 *  - `HttpRequest.timeoutMs` bounds the whole exchange: connecting, sending, redirects and reading the body. Without
 *    one, connecting may take 30 s and the connection may sit idle for 60 s (the defaults of `URLSession`). The one
 *    step `disconnect()` cannot interrupt is the name lookup, so a request to a host whose resolution hangs returns
 *    `Timeout` only when the resolver gives up (a few seconds on Android), not at the millisecond asked for.
 *  - **Cancelling the calling coroutine aborts the connection**: the socket is closed from the cancelling thread, so a
 *    blocked read ends at once, and the caller sees `CancellationException`, as with every suspend call. Each request
 *    holds one `Dispatchers.IO` thread while it is in flight, which the Kv, Fs and SecureStore adapters share.
 *  - The response body is read in chunks and held in memory; one larger than [maxResponseBytes] fails with `Network`
 *    instead of exhausting the heap. A request body over 256 KiB is streamed with a fixed length instead of being
 *    buffered by the connection.
 *  - Redirects are followed by this class (see `Redirects`): `301` and `302` turn a `POST` into a `GET`, `303` turns
 *    everything but `HEAD` into one, `307` and `308` repeat the request, at most 20 times, never from `https` to `http`,
 *    and never with credentials to another origin. Headers the connection manages itself (`Host`, `Content-Length`,
 *    `Connection`, `Expect`, `Upgrade`) are dropped from requests.
 *  - There is no cookie jar and no HTTP cache: the core has its own query cache and decides what to persist.
 *
 * Since Android 9 cleartext (`http://`) traffic is off unless the app's network security config allows the host; the
 * `INTERNET` permission is declared by this library's manifest.
 *
 * @param connectTimeoutMs how long connecting may take when the request has no timeout of its own.
 * @param idleTimeoutMs how long the connection may stay silent while reading when the request has no timeout.
 * @param maxResponseBytes the largest response body accepted.
 */
public class AndroidHttpAdapter internal constructor(
    private val connectTimeoutMs: Int,
    private val idleTimeoutMs: Int,
    private val maxResponseBytes: Int,
    private val opener: (URL) -> URLConnection,
) {
    /** An adapter with the given limits; see the class documentation. */
    public constructor(
        connectTimeoutMs: Int = DEFAULT_CONNECT_TIMEOUT_MS,
        idleTimeoutMs: Int = DEFAULT_IDLE_TIMEOUT_MS,
        maxResponseBytes: Int = DEFAULT_MAX_RESPONSE_BYTES,
    ) : this(connectTimeoutMs, idleTimeoutMs, maxResponseBytes, { url -> url.openConnection() })

    init {
        require(connectTimeoutMs > 0) { "connectTimeoutMs must be positive, got $connectTimeoutMs" }
        require(idleTimeoutMs > 0) { "idleTimeoutMs must be positive, got $idleTimeoutMs" }
        require(maxResponseBytes > 0) { "maxResponseBytes must be positive, got $maxResponseBytes" }
    }

    /**
     * Performs [request].
     *
     * @throws HttpError if the request fails.
     * @throws CancellationException if the calling coroutine is cancelled (the connection is aborted first).
     */
    public suspend fun request(request: HttpRequest): HttpResponse {
        val url = HttpRules.parseUrl(request.url)
        HttpRules.checkBody(request.method, request.body)
        val headers = HttpRules.requestHeaders(request.headers)
        val exchange = Exchange()
        val answer = try {
            val total = request.timeoutMs
            // withTimeoutOrNull answers null only for its own timeout; a caller's cancellation stays a cancellation.
            if (total == null) {
                exchange.run(url, request, headers)
            } else {
                withTimeoutOrNull(total.toLong()) { exchange.run(url, request, headers) } ?: throw HttpError.Timeout
            }
        } catch (e: CancellationException) {
            // Our own coroutine being cancelled must stay a cancellation; anyone else cancelling the exchange is `Cancelled`.
            coroutineContext.ensureActive()
            throw HttpError.Cancelled
        } catch (e: HttpError) {
            throw e
        } catch (e: Exception) {
            coroutineContext.ensureActive() // a failure our own cancellation caused is the cancellation
            throw HttpRules.failure(e)
        }
        return answer
    }

    /** This adapter as an async [PortImpl] for [StandardPorts.Http]; a failure answers with the typed [HttpError]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Http.REQUEST] = { args ->
                val decoded = HttpRequest.decodeAll(args)
                try {
                    HttpResponse.encodeToByteArray(this@AndroidHttpAdapter.request(decoded))
                } catch (e: HttpError) {
                    throw UndraPortException(HttpError.encodeToByteArray(e))
                }
            }
        },
    )

    /** How the blocking part of an exchange ended. */
    private sealed interface Outcome {
        class Done(val response: HttpResponse) : Outcome

        class Failed(val error: Exception) : Outcome
    }

    /** One call of [request]: the connection currently in use, so that cancelling the call can close it. */
    private inner class Exchange {
        @Volatile
        private var connection: HttpURLConnection? = null

        @Volatile
        private var aborted = false

        /**
         * Runs the exchange off the calling thread; cancelling the caller closes the connection and so ends a blocked read.
         * The blocking part reports its failure as a value: a worker that failed *after* the caller was cancelled (the read
         * we aborted ends in an `IOException`) would otherwise replace the cancellation as the scope's exception.
         */
        suspend fun run(url: URL, request: HttpRequest, headers: List<Header>): HttpResponse = coroutineScope {
            val work = async(Dispatchers.IO) {
                try {
                    Outcome.Done(perform(url, request, headers))
                } catch (e: Exception) {
                    Outcome.Failed(e)
                }
            }
            val outcome = try {
                work.await()
            } catch (e: CancellationException) {
                abort()
                throw e
            }
            when (outcome) {
                is Outcome.Done -> outcome.response
                is Outcome.Failed -> throw outcome.error
            }
        }

        private fun abort() {
            aborted = true
            connection?.disconnect()
        }

        private fun attach(opened: HttpURLConnection) {
            connection = opened
            if (aborted) opened.disconnect()
        }

        private fun checkNotAborted() {
            if (aborted) throw CancellationException("the HTTP exchange was aborted")
        }

        /** The blocking part: connect, send, follow redirects, read. Runs on an IO thread. */
        private fun perform(first: URL, request: HttpRequest, firstHeaders: List<Header>): HttpResponse {
            var url = first
            var method = request.method
            var body = request.body
            var headers = firstHeaders
            var hops = 0
            while (true) {
                checkNotAborted()
                val opened = open(url, method, headers, body, request.timeoutMs)
                var finished = false
                try {
                    val status = opened.responseCode
                    if (status == -1) throw HttpError.Network("the server did not answer with an HTTP response")
                    val hop = Redirects.follow(url, status, opened.getHeaderField("Location"), method, body, headers)
                    if (hop != null) {
                        if (++hops > Redirects.MAX_HOPS) throw HttpError.Network("too many redirects (more than ${Redirects.MAX_HOPS})")
                        url = hop.url
                        method = hop.method
                        body = hop.body
                        headers = hop.headers
                        continue
                    }
                    val response = read(opened, status, method)
                    finished = true
                    return response
                } finally {
                    // A fully read response leaves its connection to the pool (closing the stream returned it); anything else is abandoned.
                    if (!finished) opened.disconnect()
                }
            }
        }

        private fun open(url: URL, method: HttpMethod, headers: List<Header>, body: ByteArray?, timeoutMs: UInt?): HttpURLConnection {
            val opened = opener(url) as? HttpURLConnection
                ?: throw HttpError.InvalidUrl("${url}: only http and https URLs are supported")
            attach(opened)
            try {
                opened.requestMethod = method.name
            } catch (e: ProtocolException) {
                throw HttpError.InvalidUrl("the ${method.name} method is not supported: ${e.message}")
            }
            opened.connectTimeout = socketTimeout(timeoutMs, connectTimeoutMs)
            opened.readTimeout = socketTimeout(timeoutMs, idleTimeoutMs)
            opened.useCaches = false
            opened.instanceFollowRedirects = false
            for (header in headers) {
                try {
                    opened.addRequestProperty(header.name, header.value)
                } catch (e: IllegalArgumentException) {
                    throw HttpError.InvalidUrl("header '${header.name}' is not allowed: ${e.message}")
                }
            }
            val payload = body ?: if (HttpRules.requiresBody(method)) NO_BODY else null
            if (payload != null) {
                opened.doOutput = true
                if (payload.size > STREAMING_THRESHOLD) opened.setFixedLengthStreamingMode(payload.size)
                opened.outputStream.use { out ->
                    var offset = 0
                    while (offset < payload.size) {
                        checkNotAborted()
                        val chunk = minOf(CHUNK, payload.size - offset)
                        out.write(payload, offset, chunk)
                        offset += chunk
                    }
                }
            }
            return opened
        }

        private fun read(opened: HttpURLConnection, status: Int, method: HttpMethod): HttpResponse {
            val headers = HttpRules.responseHeaders(opened.headerFields)
            val stream = if (status >= 400) opened.errorStream else opened.inputStream
            // A HEAD answer has no body to read, but its stream is closed all the same so that the connection is released.
            val body = when {
                stream == null -> NO_BODY
                method == HttpMethod.HEAD -> stream.use { NO_BODY }
                else -> stream.use { readBounded(it, opened.contentLengthLong) }
            }
            return HttpResponse(status.toUShort(), headers, body)
        }

        /** Reads [stream] to its end in chunks, failing as soon as it is known to exceed [maxResponseBytes]. */
        private fun readBounded(stream: InputStream, declared: Long): ByteArray {
            if (declared > maxResponseBytes) throw HttpError.Network("the response body ($declared bytes) is larger than the limit of $maxResponseBytes bytes")
            val out = ByteArrayOutputStream(if (declared in 1..INITIAL_BUFFER_LIMIT) declared.toInt() else CHUNK)
            val buffer = ByteArray(CHUNK)
            var total = 0L
            while (true) {
                checkNotAborted()
                val n = stream.read(buffer)
                if (n < 0) break
                total += n
                if (total > maxResponseBytes) throw HttpError.Network("the response body is larger than the limit of $maxResponseBytes bytes")
                out.write(buffer, 0, n)
            }
            return out.toByteArray()
        }
    }

    /** Defaults and limits. */
    public companion object {
        /** How long connecting may take when the request has no timeout: 30 s. */
        public const val DEFAULT_CONNECT_TIMEOUT_MS: Int = 30_000

        /** How long a connection may stay silent while reading when the request has no timeout: 60 s. */
        public const val DEFAULT_IDLE_TIMEOUT_MS: Int = 60_000

        /** The largest response body accepted by default: 64 MiB. */
        public const val DEFAULT_MAX_RESPONSE_BYTES: Int = 64 * 1024 * 1024

        private const val STREAMING_THRESHOLD = 256 * 1024
        private const val CHUNK = 8 * 1024
        private const val INITIAL_BUFFER_LIMIT = 1L * 1024 * 1024
        private val NO_BODY = ByteArray(0)

        /** The socket timeout for a request: its own `timeoutMs` when it has one (at least 1, as 0 means "forever"), else [default]. */
        private fun socketTimeout(timeoutMs: UInt?, default: Int): Int =
            if (timeoutMs == null) default else timeoutMs.toLong().coerceIn(1L, Int.MAX_VALUE.toLong()).toInt()
    }
}
