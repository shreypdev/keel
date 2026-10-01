package dev.undra.android

import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import java.io.InterruptedIOException
import java.net.MalformedURLException
import java.net.SocketTimeoutException
import java.net.URI
import java.net.URISyntaxException
import java.net.URL
import java.util.Locale

/**
 * The decisions of [AndroidHttpAdapter] that need no network: which URLs and headers are acceptable, how a failure
 * becomes an [HttpError] and how response headers are listed. Kept apart so that JVM unit tests can check them.
 */
internal object HttpRules {
    /** Headers the connection manages itself, dropped from requests (the same set as the JVM adapter). */
    val RESTRICTED_HEADERS: Set<String> = setOf("connection", "content-length", "expect", "host", "upgrade")

    /** Response headers that Android's `HttpURLConnection` adds about itself; the server did not send them. */
    private const val ANDROID_HEADER_PREFIX = "x-android-"

    /**
     * Parses [text] into a URL the adapter can open.
     *
     * @throws HttpError.InvalidUrl if it is not an `http` or `https` URL with a host.
     */
    fun parseUrl(text: String): URL {
        val uri = try {
            URI(text)
        } catch (e: URISyntaxException) {
            throw HttpError.InvalidUrl("$text: ${e.reason}")
        }
        val scheme = uri.scheme?.lowercase(Locale.ROOT)
        if (scheme != "http" && scheme != "https") throw HttpError.InvalidUrl("$text: only http and https URLs are supported")
        if (uri.host == null) throw HttpError.InvalidUrl("$text: the URL has no host")
        return try {
            uri.toURL()
        } catch (e: MalformedURLException) {
            throw HttpError.InvalidUrl("$text: ${e.message}")
        }
    }

    /**
     * The headers to send: [headers] without the ones the connection manages ([RESTRICTED_HEADERS]).
     * Order and repeated names are kept.
     */
    fun requestHeaders(headers: List<Header>): List<Header> =
        headers.filter { it.name.lowercase(Locale.ROOT) !in RESTRICTED_HEADERS }

    /**
     * Whether a request with this [method] and [body] can be sent: `HttpURLConnection` turns a `GET` that has a body
     * into a `POST`, so a body on `GET` or `HEAD` is refused instead of silently changing the request.
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
     * The response headers of `HttpURLConnection.getHeaderFields()`, as the port reports them: sorted by name (ignoring
     * case), a repeated header once per value in the order the server sent them, without the status line (the entry with
     * a `null` name) and without Android's own `X-Android-*` bookkeeping.
     */
    fun responseHeaders(fields: Map<String?, List<String?>?>): List<Header> {
        val named = fields.entries.mapNotNull { (name, values) -> if (name == null || values == null) null else name to values }
        val result = ArrayList<Header>()
        for ((name, values) in named.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.first })) {
            if (name.lowercase(Locale.ROOT).startsWith(ANDROID_HEADER_PREFIX)) continue
            for (value in values) if (value != null) result.add(Header(name, value))
        }
        return result
    }

    /**
     * What a failure of the exchange means to the core (ADR-025): a timeout is `Timeout`, an interrupted transfer
     * `Cancelled`, a URL or header the platform rejects `InvalidUrl`, and every other failure `Network` with the
     * platform's description. An [HttpError] passes through.
     */
    fun failure(error: Throwable): HttpError =
        when (error) {
            is HttpError -> error
            is SocketTimeoutException -> HttpError.Timeout // before InterruptedIOException, which it extends
            is InterruptedIOException -> HttpError.Cancelled
            is MalformedURLException -> HttpError.InvalidUrl(error.message ?: "malformed URL")
            is IllegalArgumentException -> HttpError.InvalidUrl(error.message ?: "invalid request")
            else -> HttpError.Network(describe(error)) // IOException, SecurityException, anything unforeseen
        }

    /** The text of [error], with the likely fix when the cause is a missing `INTERNET` permission. */
    private fun describe(error: Throwable): String {
        val text = error.message ?: error.javaClass.simpleName
        val denied = text.contains("EPERM") || text.contains("Permission denied")
        return if (denied) "$text (is android.permission.INTERNET declared in the app's manifest?)" else text
    }
}

/** What to do about a redirect response: where to go next, and with what. */
internal class Hop(val url: URL, val method: HttpMethod, val body: ByteArray?, val headers: List<Header>)

/**
 * Redirect following, written out because `HttpURLConnection` does not follow a redirect from `http` to `https`
 * and a Kotlin core cannot tell which of two Android versions it is talking to. The rules are those of the JVM
 * adapter (`java.net.http`, `Redirect.NORMAL`): `301` and `302` turn a `POST` into a `GET`, `303` turns everything
 * but `HEAD` into a `GET`, `307` and `308` repeat the request; a redirect from `https` to `http` is not followed (the
 * caller gets the redirect response); credentials are not sent to another origin.
 */
internal object Redirects {
    /** The most redirects one request follows before it fails. */
    const val MAX_HOPS: Int = 20

    private val CREDENTIAL_HEADERS = setOf("authorization", "cookie", "proxy-authorization")
    private val BODY_HEADERS = setOf("content-type", "content-length", "content-encoding", "transfer-encoding")

    /** Whether [status] is a redirect this adapter knows how to follow. */
    fun isRedirect(status: Int): Boolean = status == 301 || status == 302 || status == 303 || status == 307 || status == 308

    /**
     * The request to make after a [status] response with this [location], or `null` when the response itself is the
     * answer (no usable `Location`, a scheme that is not http or https, or a downgrade from https to http).
     */
    fun follow(current: URL, status: Int, location: String?, method: HttpMethod, body: ByteArray?, headers: List<Header>): Hop? {
        if (!isRedirect(status) || location.isNullOrBlank()) return null
        val spec = location.trim()
        val target = try {
            // `URL` still resolves a query-only reference the old way (RFC 2396, dropping the last path segment); RFC 3986 keeps the path.
            if (spec.startsWith("?")) URL(current.protocol, current.host, current.port, current.path + spec) else URL(current, spec)
        } catch (e: MalformedURLException) {
            return null
        }
        val scheme = target.protocol.lowercase(Locale.ROOT)
        if (scheme != "http" && scheme != "https") return null
        if (current.protocol.equals("https", ignoreCase = true) && scheme == "http") return null
        if (target.host.isNullOrEmpty()) return null

        val next = when (status) {
            301, 302 -> if (method == HttpMethod.POST) HttpMethod.GET else method
            303 -> if (method == HttpMethod.HEAD) HttpMethod.HEAD else HttpMethod.GET
            else -> method
        }
        val keepsBody = next == method && next != HttpMethod.GET && next != HttpMethod.HEAD
        var forwarded = headers
        if (!keepsBody) forwarded = forwarded.filter { it.name.lowercase(Locale.ROOT) !in BODY_HEADERS }
        if (!sameOrigin(current, target)) forwarded = forwarded.filter { it.name.lowercase(Locale.ROOT) !in CREDENTIAL_HEADERS }
        return Hop(target, next, if (keepsBody) body else null, forwarded)
    }

    private fun sameOrigin(a: URL, b: URL): Boolean =
        a.protocol.equals(b.protocol, ignoreCase = true) && a.host.equals(b.host, ignoreCase = true) && effectivePort(a) == effectivePort(b)

    private fun effectivePort(url: URL): Int = if (url.port != -1) url.port else url.defaultPort
}
