package dev.undra.okhttp

import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import java.io.InterruptedIOException
import java.net.MalformedURLException
import java.net.ProtocolException
import java.net.SocketTimeoutException
import java.util.Locale
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

/**
 * The decisions of the OkHttp adapters that need no network: which URLs and headers are acceptable, how a failure becomes the
 * port's typed error and how response headers are listed. Kept apart so that JVM unit tests can check them. They are the rules
 * of `AndroidHttpAdapter` (`HttpRules`) with OkHttp's exceptions and parser in place of `HttpURLConnection`'s: the contract
 * (ADR-025, ADR-047) is the same, and the shared suites in `adapter-contracts` and `test-support` check that it is.
 */
internal object OkHttpRules {
    /** Headers the connection manages itself, dropped from requests (the same set as the other adapters). */
    val RESTRICTED_HEADERS: Set<String> = setOf("connection", "content-length", "expect", "host", "upgrade")

    /** Headers an upgrade request writes itself: a caller's is refused, never dropped (ADR-047 §5; the default client's set). */
    private val RESERVED_WEBSOCKET_HEADERS: Set<String> = setOf(
        "host", "upgrade", "connection", "sec-websocket-key", "sec-websocket-version", "sec-websocket-extensions",
        "sec-websocket-protocol", "sec-websocket-accept", "content-length", "transfer-encoding",
    )

    /** The most redirects and authentication retries OkHttp makes for one call (`RealCall`'s `MAX_FOLLOW_UPS`). */
    const val MAX_FOLLOW_UPS: Int = 20

    /** The text of the exception OkHttp throws when a call needs more follow-ups than [MAX_FOLLOW_UPS]. */
    private const val TOO_MANY_FOLLOW_UPS = "Too many follow-up requests"

    /**
     * Parses [text] into a URL OkHttp can request.
     *
     * @throws HttpError.InvalidUrl if it is not an `http` or `https` URL with a host.
     */
    fun parseUrl(text: String): HttpUrl =
        text.toHttpUrlOrNull() ?: throw HttpError.InvalidUrl("$text: not an http or https URL with a host")

    /**
     * The headers to send: [headers] without the ones the connection manages ([RESTRICTED_HEADERS]).
     * Order and repeated names are kept.
     */
    fun requestHeaders(headers: List<Header>): List<Header> =
        headers.filter { it.name.lowercase(Locale.ROOT) !in RESTRICTED_HEADERS }

    /**
     * Whether a request with this [method] and [body] can be sent: a body on `GET` or `HEAD` is refused instead of being dropped or
     * turning the request into another.
     *
     * @throws HttpError.InvalidUrl if it cannot.
     */
    fun checkBody(method: HttpMethod, body: ByteArray?) {
        if ((method == HttpMethod.GET || method == HttpMethod.HEAD) && body != null && body.isNotEmpty()) {
            throw HttpError.InvalidUrl("a ${method.name} request cannot carry a body")
        }
    }

    /** Whether a request with [method] sends a body even when the caller gave none (an empty one, `Content-Length: 0`). */
    fun requiresBody(method: HttpMethod): Boolean = method == HttpMethod.POST || method == HttpMethod.PUT || method == HttpMethod.PATCH

    /**
     * The OkHttp request for [request]: the headers as given (without [RESTRICTED_HEADERS]; the caller's `Content-Type` is left
     * as written, the body carries no media type of its own), the body whole.
     *
     * @throws HttpError.InvalidUrl for a header OkHttp refuses (a name that is not a token, a value that is not printable ASCII).
     */
    fun build(url: HttpUrl, request: HttpRequest): Request {
        val builder = Request.Builder().url(url)
        for (header in requestHeaders(request.headers)) {
            try {
                builder.addHeader(header.name, header.value)
            } catch (e: IllegalArgumentException) {
                throw HttpError.InvalidUrl("header '${header.name}' is not allowed: ${e.message}")
            }
        }
        val payload = request.body ?: if (requiresBody(request.method)) ByteArray(0) else null
        try {
            builder.method(request.method.name, payload?.toRequestBody(null))
        } catch (e: IllegalArgumentException) {
            throw HttpError.InvalidUrl("the ${request.method.name} method is not supported: ${e.message}")
        }
        return builder.build()
    }

    /**
     * What a failure of an exchange means to the core (ADR-025): a timeout (the client's connect or read timeout, or its call
     * timeout) is `Timeout`, a call cancelled by someone else [canceled] or an interrupted transfer `Cancelled`, a URL or header
     * OkHttp rejects `InvalidUrl`, and every other failure `Network` with the platform's description. An [HttpError] passes through.
     */
    fun failure(error: Throwable, canceled: Boolean = false): HttpError =
        when {
            error is HttpError -> error
            error is SocketTimeoutException -> HttpError.Timeout // before InterruptedIOException, which it extends
            // OkHttp's call timeout ends the call with an InterruptedIOException saying "timeout"; any other one is an interrupt.
            error is InterruptedIOException -> if (error.message == "timeout") HttpError.Timeout else HttpError.Cancelled
            canceled -> HttpError.Cancelled
            error is ProtocolException && error.message?.startsWith(TOO_MANY_FOLLOW_UPS) == true ->
                HttpError.Network("too many redirects or authentication retries (more than $MAX_FOLLOW_UPS)")
            error is MalformedURLException -> HttpError.InvalidUrl(error.message ?: "malformed URL")
            error is IllegalArgumentException -> HttpError.InvalidUrl(error.message ?: "invalid request")
            else -> HttpError.Network(describe(error)) // IOException (TLS, DNS, refused, reset, cleartext policy), anything unforeseen
        }

    /** The headers of a response as the port reports them: sorted by name (ignoring case), a repeated one once per value in the order sent. */
    fun responseHeaders(headers: okhttp3.Headers): List<Header> =
        headers.map { (name, value) -> Header(name, value) }.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.name })

    /** The text of [error], with the likely fix when the cause is a missing `INTERNET` permission. */
    fun describe(error: Throwable): String {
        val text = error.message?.takeIf { it.isNotBlank() } ?: error.javaClass.simpleName
        val denied = text.contains("EPERM") || text.contains("Permission denied")
        return if (denied) "$text (is android.permission.INTERNET declared in the app's manifest?)" else text
    }

    /**
     * Why [name] and [value] cannot go into a WebSocket upgrade request, or `null` when they can: a line break or a NUL in the
     * value, a header the client writes itself.
     */
    fun webSocketHeaderProblem(name: String, value: String): String? = when {
        value.any { it == '\r' || it == '\n' || it == '\u0000' } -> "the value of header '$name' contains a line break or a NUL"
        name.lowercase(Locale.ROOT) in RESERVED_WEBSOCKET_HEADERS -> "the header '$name' is set by the WebSocket client itself"
        else -> null
    }

    /** Why [protocol] cannot be offered (empty, not an HTTP token), or `null` when it can. */
    fun subprotocolProblem(protocol: String): String? =
        if (protocol.isEmpty() || protocol.any { it.code > 0x7e || it.code <= 0x20 || it in "()<>@,;:\\\"/[]?={}" }) {
            "the subprotocol '$protocol' is not an HTTP token"
        } else {
            null
        }
}
