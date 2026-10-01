package dev.undra.android

import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpMethod
import java.net.URL
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The redirect rules of the Http adapter, which are those of `java.net.http` with `Redirect.NORMAL`. */
class RedirectsTest {
    private val here = URL("https://api.example.com/v1/items")
    private val body = byteArrayOf(1, 2, 3)
    private val headers = listOf(Header("Content-Type", "application/json"), Header("Authorization", "Bearer t"), Header("X-Id", "7"))

    private fun follow(status: Int, location: String?, method: HttpMethod = HttpMethod.POST, from: URL = here) =
        Redirects.follow(from, status, location, method, body, headers)

    @Test
    fun only_the_five_redirect_statuses_are_redirects() {
        for (status in listOf(301, 302, 303, 307, 308)) assertTrue("$status", Redirects.isRedirect(status))
        for (status in listOf(200, 204, 300, 304, 305, 306, 309, 404)) assertTrue("$status", !Redirects.isRedirect(status))
        assertNull(follow(300, "/elsewhere"))
        assertNull(follow(304, "/elsewhere"))
    }

    @Test
    fun a_redirect_without_a_usable_location_is_the_answer() {
        assertNull(follow(302, null))
        assertNull(follow(302, ""))
        assertNull(follow(302, "   "))
        assertNull(follow(302, "ftp://example.com/x"))
        assertNull(follow(302, "mailto:x@example.com"))
    }

    @Test
    fun a_301_or_302_turns_a_post_into_a_get_without_its_body() {
        for (status in listOf(301, 302)) {
            val hop = follow(status, "/v2/items", HttpMethod.POST)!!
            assertEquals(HttpMethod.GET, hop.method)
            assertNull(hop.body)
            assertEquals("https://api.example.com/v2/items", hop.url.toString())
            assertEquals(listOf(Header("Authorization", "Bearer t"), Header("X-Id", "7")), hop.headers) // content headers go with the body
        }
    }

    @Test
    fun a_301_or_302_keeps_every_other_method_and_a_body_where_the_method_has_one() {
        for (method in listOf(HttpMethod.PUT, HttpMethod.PATCH, HttpMethod.DELETE)) {
            val hop = follow(302, "/moved", method)!!
            assertEquals(method, hop.method)
            assertTrue(body.contentEquals(hop.body))
        }
        for (method in listOf(HttpMethod.GET, HttpMethod.HEAD)) {
            val hop = follow(302, "/moved", method)!!
            assertEquals(method, hop.method)
            assertNull(hop.body)
        }
    }

    @Test
    fun a_303_turns_everything_but_head_into_a_get() {
        for (method in listOf(HttpMethod.POST, HttpMethod.PUT, HttpMethod.PATCH, HttpMethod.DELETE, HttpMethod.GET)) {
            val hop = follow(303, "/done", method)!!
            assertEquals(HttpMethod.GET, hop.method)
            assertNull(hop.body)
        }
        assertEquals(HttpMethod.HEAD, follow(303, "/done", HttpMethod.HEAD)!!.method)
    }

    @Test
    fun a_307_or_308_repeats_the_request() {
        for (status in listOf(307, 308)) {
            val hop = follow(status, "/again", HttpMethod.POST)!!
            assertEquals(HttpMethod.POST, hop.method)
            assertTrue(body.contentEquals(hop.body))
            assertEquals(headers, hop.headers)
        }
    }

    @Test
    fun a_relative_location_is_resolved_against_the_current_url() {
        assertEquals("https://api.example.com/v1/other", follow(307, "other")!!.url.toString())
        assertEquals("https://api.example.com/top", follow(307, "/top")!!.url.toString())
        assertEquals("https://api.example.com/v1/items?page=2", follow(307, "?page=2")!!.url.toString())
        assertEquals("https://cdn.example.net/x", follow(307, "//cdn.example.net/x")!!.url.toString())
    }

    @Test
    fun a_redirect_from_https_to_http_is_not_followed_but_http_to_https_is() {
        assertNull(follow(302, "http://api.example.com/downgrade"))
        assertNotNull(follow(302, "https://api.example.com/same"))
        val up = Redirects.follow(URL("http://example.com/"), 301, "https://example.com/", HttpMethod.GET, null, emptyList())
        assertEquals("https://example.com/", up!!.url.toString())
        val plain = Redirects.follow(URL("http://example.com/"), 301, "http://example.com/b", HttpMethod.GET, null, emptyList())
        assertEquals("http://example.com/b", plain!!.url.toString())
    }

    @Test
    fun credentials_are_not_sent_to_another_origin() {
        val sameOrigin = follow(307, "/same")!!
        assertTrue(sameOrigin.headers.any { it.name == "Authorization" })
        // another host, another port, and another scheme (an upgrade from http) are other origins
        for (elsewhere in listOf("https://evil.example.com/x", "https://api.example.com:8443/x", "https://plain.example.com/x")) {
            val from = if (elsewhere.contains("plain")) URL("http://plain.example.com/v1/items") else here
            val hop = Redirects.follow(from, 307, elsewhere, HttpMethod.POST, body, headers + Header("Cookie", "s=1") + Header("Proxy-Authorization", "x"))!!
            val names = hop.headers.map { it.name }
            assertTrue(elsewhere, "Authorization" !in names && "Cookie" !in names && "Proxy-Authorization" !in names)
            assertTrue(elsewhere, "X-Id" in names && "Content-Type" in names)
        }
    }

    @Test
    fun the_default_port_counts_as_the_same_origin() {
        val hop = Redirects.follow(URL("https://example.com/a"), 307, "https://example.com:443/b", HttpMethod.GET, null, listOf(Header("Authorization", "x")))!!
        assertEquals(listOf(Header("Authorization", "x")), hop.headers)
    }
}
