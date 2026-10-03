package dev.undra.okhttp

import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import java.io.EOFException
import java.io.IOException
import java.io.InterruptedIOException
import java.net.ConnectException
import java.net.MalformedURLException
import java.net.ProtocolException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import javax.net.ssl.SSLHandshakeException
import javax.net.ssl.SSLPeerUnverifiedException
import okhttp3.Headers
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** The decisions of the OkHttp adapters that need no network: URLs, headers, the mapping of failures to the port's typed errors. */
class OkHttpRulesTest {
    private fun invalidUrl(block: () -> Any?): HttpError.InvalidUrl {
        try {
            block()
        } catch (e: HttpError.InvalidUrl) {
            return e
        }
        fail("expected InvalidUrl")
        throw AssertionError()
    }

    // ---- URLs and requests -----------------------------------------------------------------------------------

    @Test
    fun only_http_and_https_urls_with_a_host_are_accepted() {
        assertEquals("http://127.0.0.1:8080/a?b=1", OkHttpRules.parseUrl("http://127.0.0.1:8080/a?b=1").toString())
        assertEquals("https://example.com/", OkHttpRules.parseUrl("HTTPS://example.com").toString())
        for (bad in listOf("ftp://example.com/", "example.com", "http://", "http://exa mple.com/", "file:///etc/passwd", "ws://example.com/", "")) {
            assertTrue(bad, invalidUrl { OkHttpRules.parseUrl(bad) }.reason.contains("not an http or https URL"))
        }
    }

    @Test
    fun the_headers_the_connection_manages_are_dropped_and_the_rest_keep_their_order_and_repeats() {
        val kept = OkHttpRules.requestHeaders(
            listOf(Header("X-Tag", "1"), Header("Host", "evil.example"), Header("X-Tag", "2"), Header("CONTENT-LENGTH", "99"), Header("Connection", "close"), Header("Upgrade", "h2c"), Header("Expect", "100-continue")),
        )
        assertEquals(listOf(Header("X-Tag", "1"), Header("X-Tag", "2")), kept)
    }

    @Test
    fun a_body_on_get_or_head_is_refused_and_an_empty_one_is_not() {
        assertTrue(invalidUrl { OkHttpRules.checkBody(HttpMethod.GET, byteArrayOf(1)) }.reason.contains("cannot carry a body"))
        assertTrue(invalidUrl { OkHttpRules.checkBody(HttpMethod.HEAD, byteArrayOf(1)) }.reason.contains("cannot carry a body"))
        OkHttpRules.checkBody(HttpMethod.GET, ByteArray(0))
        OkHttpRules.checkBody(HttpMethod.GET, null)
        OkHttpRules.checkBody(HttpMethod.POST, byteArrayOf(1))
        OkHttpRules.checkBody(HttpMethod.DELETE, byteArrayOf(1))
    }

    @Test
    fun post_put_and_patch_always_carry_a_body_and_the_callers_content_type_is_left_as_written() {
        val url = OkHttpRules.parseUrl("http://example.com/x")
        for (method in listOf(HttpMethod.POST, HttpMethod.PUT, HttpMethod.PATCH)) {
            val built = OkHttpRules.build(url, HttpRequest(method, url.toString(), listOf(Header("Content-Type", "application/json; charset=utf-8")), null, null))
            assertEquals(method.name, built.method)
            assertEquals(0L, built.body!!.contentLength())
            assertEquals("application/json; charset=utf-8", built.header("Content-Type"))
            assertNull("the body adds no media type of its own", built.body!!.contentType())
        }
        for (method in listOf(HttpMethod.GET, HttpMethod.HEAD, HttpMethod.DELETE, HttpMethod.OPTIONS)) {
            assertNull(method.name, OkHttpRules.build(url, HttpRequest(method, url.toString(), emptyList(), null, null)).body)
        }
    }

    @Test
    fun a_header_okhttp_refuses_is_invalid_and_names_the_header() {
        val url = OkHttpRules.parseUrl("http://example.com/x")
        for ((name, value) in listOf("X-Bad" to "line\nbreak", "X-Nul" to "a\u0000b", "X-Name" to "José", "bad name" to "v", "" to "v")) {
            val e = invalidUrl { OkHttpRules.build(url, HttpRequest(HttpMethod.GET, url.toString(), listOf(Header(name, value)), null, null)) }
            assertTrue(e.toString(), e.reason.contains("header '$name' is not allowed"))
        }
    }

    // ---- failures ----------------------------------------------------------------------------------------------

    @Test
    fun a_timeout_of_the_client_or_of_a_call_is_a_timeout() {
        assertEquals(HttpError.Timeout, OkHttpRules.failure(SocketTimeoutException("timeout")))
        assertEquals(HttpError.Timeout, OkHttpRules.failure(SocketTimeoutException("Read timed out")))
        assertEquals(HttpError.Timeout, OkHttpRules.failure(InterruptedIOException("timeout"))) // OkHttp's call timeout
    }

    @Test
    fun an_interrupted_transfer_or_a_call_cancelled_by_someone_else_is_cancelled() {
        assertEquals(HttpError.Cancelled, OkHttpRules.failure(InterruptedIOException("interrupted")))
        assertEquals(HttpError.Cancelled, OkHttpRules.failure(IOException("Canceled"), canceled = true))
        assertEquals(HttpError.Cancelled, OkHttpRules.failure(java.net.SocketException("Socket closed"), canceled = true))
        // The same exceptions with no cancellation behind them are what they say.
        assertTrue(OkHttpRules.failure(IOException("Canceled"), canceled = false) is HttpError.Network)
    }

    @Test
    fun too_many_follow_ups_is_a_network_error_that_says_redirects() {
        val e = OkHttpRules.failure(ProtocolException("Too many follow-up requests: 21"))
        assertTrue(e.toString(), e is HttpError.Network && e.reason == "too many redirects or authentication retries (more than 20)")
        // Any other protocol failure keeps OkHttp's text.
        val other = OkHttpRules.failure(ProtocolException("Unexpected status line: this is not http"))
        assertTrue(other.toString(), other is HttpError.Network && other.reason == "Unexpected status line: this is not http")
    }

    @Test
    fun a_pin_that_does_not_match_a_tls_failure_and_an_unknown_host_are_network_errors_with_the_platforms_text() {
        val pin = OkHttpRules.failure(SSLPeerUnverifiedException("Certificate pinning failure!\n  Peer certificate chain:\n    sha256/AAAA: CN=example.com"))
        assertTrue(pin.toString(), pin is HttpError.Network && pin.reason.startsWith("Certificate pinning failure!"))
        val tls = OkHttpRules.failure(SSLHandshakeException("PKIX path building failed"))
        assertTrue(tls.toString(), tls is HttpError.Network && tls.reason == "PKIX path building failed")
        val dns = OkHttpRules.failure(UnknownHostException("Unable to resolve host \"nope.invalid\""))
        assertTrue(dns.toString(), dns is HttpError.Network && dns.reason.contains("nope.invalid"))
        assertTrue(OkHttpRules.failure(ConnectException("Failed to connect to /127.0.0.1:1")) is HttpError.Network)
        assertTrue(OkHttpRules.failure(EOFException("\\n not found: limit=0")) is HttpError.Network)
    }

    @Test
    fun a_rejected_url_or_request_is_invalid_and_anything_unforeseen_is_a_network_error() {
        assertTrue(OkHttpRules.failure(IllegalArgumentException("unexpected url")) is HttpError.InvalidUrl)
        assertTrue(OkHttpRules.failure(MalformedURLException("no protocol")) is HttpError.InvalidUrl)
        val odd = OkHttpRules.failure(IllegalStateException("closed"))
        assertTrue(odd.toString(), odd is HttpError.Network && odd.reason == "closed")
        assertEquals(HttpError.Network("x"), OkHttpRules.failure(HttpError.Network("x")))
        assertTrue((OkHttpRules.failure(RuntimeException()) as HttpError.Network).reason.contains("RuntimeException"))
    }

    @Test
    fun a_permission_failure_suggests_the_internet_permission() {
        val e = OkHttpRules.failure(java.net.SocketException("socket failed: EPERM (Operation not permitted)"))
        assertTrue(e.toString(), e is HttpError.Network && e.reason.contains("android.permission.INTERNET"))
    }

    // ---- responses -----------------------------------------------------------------------------------------------

    @Test
    fun response_headers_are_sorted_by_name_ignoring_case_and_repeats_keep_the_servers_order() {
        val headers = Headers.Builder()
            .add("Set-Cookie", "a=1")
            .add("x-z", "z")
            .add("Set-Cookie", "b=2")
            .add("Content-Type", "text/plain")
            .add("X-A", "a")
            .build()
        assertEquals(
            listOf(Header("Content-Type", "text/plain"), Header("Set-Cookie", "a=1"), Header("Set-Cookie", "b=2"), Header("X-A", "a"), Header("x-z", "z")),
            OkHttpRules.responseHeaders(headers),
        )
    }

    // ---- WebSocket ------------------------------------------------------------------------------------------------

    @Test
    fun the_headers_an_upgrade_writes_itself_and_the_ones_that_inject_are_refused() {
        for (name in listOf("Host", "Upgrade", "connection", "Sec-WebSocket-Key", "Sec-WebSocket-Protocol", "Sec-WebSocket-Extensions", "Content-Length", "Transfer-Encoding")) {
            assertTrue(name, OkHttpRules.webSocketHeaderProblem(name, "v")!!.contains("set by the WebSocket client itself"))
        }
        assertTrue(OkHttpRules.webSocketHeaderProblem("X", "a\r\nInjected: 1")!!.contains("line break"))
        assertTrue(OkHttpRules.webSocketHeaderProblem("X", "a\u0000")!!.contains("NUL"))
        assertNull(OkHttpRules.webSocketHeaderProblem("Authorization", "Bearer x"))
    }

    @Test
    fun a_subprotocol_must_be_an_http_token() {
        assertNull(OkHttpRules.subprotocolProblem("graphql-transport-ws"))
        assertNull(OkHttpRules.subprotocolProblem("v1.chat"))
        for (bad in listOf("", "bad protocol", "a,b", "ü", "a;b", "a/b")) {
            assertTrue(bad, OkHttpRules.subprotocolProblem(bad)!!.contains("not an HTTP token"))
        }
    }
}
