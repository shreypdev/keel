package dev.undra.android

import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import java.io.FileNotFoundException
import java.io.IOException
import java.io.InterruptedIOException
import java.net.ConnectException
import java.net.MalformedURLException
import java.net.SocketException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import javax.net.ssl.SSLHandshakeException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/** URL handling, header rules and error mapping of the Http adapter: the parts that need no network. */
class HttpRulesTest {
    // ---- URLs ----------------------------------------------------------------------------------------------

    @Test
    fun http_and_https_urls_with_a_host_parse() {
        assertEquals("http://example.com/a?b=c", HttpRules.parseUrl("http://example.com/a?b=c").toString())
        assertEquals("https://example.com:8443/", HttpRules.parseUrl("https://example.com:8443/").toString())
        assertEquals("http://127.0.0.1:8080/x", HttpRules.parseUrl("http://127.0.0.1:8080/x").toString())
        assertEquals("http://[::1]:80/", HttpRules.parseUrl("http://[::1]:80/").toString())
    }

    @Test
    fun the_scheme_is_matched_without_regard_to_case() {
        assertEquals("example.com", HttpRules.parseUrl("HTTPS://example.com/").host)
    }

    @Test
    fun other_schemes_are_invalid() {
        for (url in listOf("ftp://example.com/", "file:///etc/hosts", "content://media/x", "ws://example.com/", "mailto:a@b.c", "javascript:alert(1)")) {
            val e = assertThrows(url, HttpError.InvalidUrl::class.java) { HttpRules.parseUrl(url) }
            assertTrue(e.reason, e.reason.contains("only http and https"))
        }
    }

    @Test
    fun a_url_without_a_scheme_or_a_host_is_invalid() {
        assertThrows(HttpError.InvalidUrl::class.java) { HttpRules.parseUrl("example.com/path") }
        assertThrows(HttpError.InvalidUrl::class.java) { HttpRules.parseUrl("/relative/path") }
        assertThrows(HttpError.InvalidUrl::class.java) { HttpRules.parseUrl("") }
        val noHost = assertThrows(HttpError.InvalidUrl::class.java) { HttpRules.parseUrl("http:///path") }
        assertTrue(noHost.reason, noHost.reason.contains("no host"))
    }

    @Test
    fun a_syntactically_broken_url_is_invalid_and_the_reason_names_the_url() {
        val e = assertThrows(HttpError.InvalidUrl::class.java) { HttpRules.parseUrl("http://exa mple.com/") }
        assertTrue(e.reason, e.reason.startsWith("http://exa mple.com/"))
        assertThrows(HttpError.InvalidUrl::class.java) { HttpRules.parseUrl("http://example.com/a b") }
    }

    // ---- request headers and bodies ---------------------------------------------------------------------------

    @Test
    fun headers_the_connection_manages_are_dropped_and_the_rest_keep_their_order_and_repeats() {
        val given = listOf(
            Header("Accept", "a"),
            Header("Host", "evil.example"),
            Header("content-length", "5"),
            Header("X-Tag", "1"),
            Header("CONNECTION", "close"),
            Header("Expect", "100-continue"),
            Header("Upgrade", "websocket"),
            Header("X-Tag", "2"),
        )
        assertEquals(listOf(Header("Accept", "a"), Header("X-Tag", "1"), Header("X-Tag", "2")), HttpRules.requestHeaders(given))
    }

    @Test
    fun a_get_or_head_with_a_body_is_refused_but_an_empty_body_is_not() {
        assertThrows(HttpError.InvalidUrl::class.java) { HttpRules.checkBody(HttpMethod.GET, byteArrayOf(1)) }
        assertThrows(HttpError.InvalidUrl::class.java) { HttpRules.checkBody(HttpMethod.HEAD, byteArrayOf(1)) }
        HttpRules.checkBody(HttpMethod.GET, null)
        HttpRules.checkBody(HttpMethod.GET, ByteArray(0))
        HttpRules.checkBody(HttpMethod.POST, byteArrayOf(1))
        HttpRules.checkBody(HttpMethod.DELETE, byteArrayOf(1))
    }

    @Test
    fun post_put_and_patch_always_send_a_body() {
        assertTrue(HttpRules.requiresBody(HttpMethod.POST))
        assertTrue(HttpRules.requiresBody(HttpMethod.PUT))
        assertTrue(HttpRules.requiresBody(HttpMethod.PATCH))
        assertTrue(!HttpRules.requiresBody(HttpMethod.GET))
        assertTrue(!HttpRules.requiresBody(HttpMethod.DELETE))
        assertTrue(!HttpRules.requiresBody(HttpMethod.OPTIONS))
    }

    // ---- response headers -------------------------------------------------------------------------------------

    @Test
    fun response_headers_are_sorted_by_name_without_the_status_line_and_android_bookkeeping() {
        val fields = linkedMapOf<String?, List<String?>?>(
            null to listOf("HTTP/1.1 200 OK"),
            "X-Android-Sent-Millis" to listOf("1"),
            "x-b" to listOf("2"),
            "Set-Cookie" to listOf("a=1", "b=2"),
            "Content-Type" to listOf("text/plain"),
            "X-Android-Selected-Protocol" to listOf("http/1.1"),
            "X-A" to listOf("1"),
            "Empty" to null,
        )
        assertEquals(
            listOf(
                Header("Content-Type", "text/plain"),
                Header("Set-Cookie", "a=1"),
                Header("Set-Cookie", "b=2"),
                Header("X-A", "1"),
                Header("x-b", "2"),
            ),
            HttpRules.responseHeaders(fields),
        )
    }

    // ---- failures -----------------------------------------------------------------------------------------

    @Test
    fun a_socket_timeout_is_a_timeout_even_though_it_is_an_interrupted_io_exception() {
        assertSame(HttpError.Timeout, HttpRules.failure(SocketTimeoutException("failed to connect to /10.0.0.1 after 1000ms")))
        assertSame(HttpError.Cancelled, HttpRules.failure(InterruptedIOException("interrupted")))
    }

    @Test
    fun an_http_error_passes_through() {
        val error = HttpError.Network("already typed")
        assertSame(error, HttpRules.failure(error))
    }

    @Test
    fun other_io_failures_are_network_errors_with_the_platforms_words() {
        assertEquals(HttpError.Network("Unable to resolve host \"nope.invalid\""), HttpRules.failure(UnknownHostException("Unable to resolve host \"nope.invalid\"")))
        assertEquals(HttpError.Network("Connection refused"), HttpRules.failure(ConnectException("Connection refused")))
        assertEquals(HttpError.Network("Connection reset"), HttpRules.failure(SocketException("Connection reset")))
        assertEquals(HttpError.Network("handshake failed"), HttpRules.failure(SSLHandshakeException("handshake failed")))
        assertEquals(HttpError.Network("x"), HttpRules.failure(FileNotFoundException("x")))
        assertEquals(HttpError.Network("IOException"), HttpRules.failure(IOException()))
        assertEquals(HttpError.Network("not permitted"), HttpRules.failure(SecurityException("not permitted")))
        assertEquals(HttpError.Network("IllegalStateException"), HttpRules.failure(IllegalStateException()))
    }

    @Test
    fun a_cleartext_refusal_keeps_the_platforms_explanation() {
        val e = HttpRules.failure(java.net.UnknownServiceException("CLEARTEXT communication to example.com not permitted by network security policy"))
        assertTrue(e is HttpError.Network && e.reason.contains("not permitted by network security policy"))
    }

    @Test
    fun a_permission_failure_says_to_declare_internet() {
        val e = HttpRules.failure(SocketException("socket failed: EPERM (Operation not permitted)"))
        assertTrue(e is HttpError.Network && e.reason.contains("android.permission.INTERNET"))
    }

    @Test
    fun a_bad_url_or_argument_is_invalid() {
        assertEquals(HttpError.InvalidUrl("no protocol: x"), HttpRules.failure(MalformedURLException("no protocol: x")))
        assertEquals(HttpError.InvalidUrl("Unexpected char 0x0a"), HttpRules.failure(IllegalArgumentException("Unexpected char 0x0a")))
    }
}
