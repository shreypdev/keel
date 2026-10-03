package dev.undra.okhttp

import dev.undra.android.TestHttpServer
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test

/** The client of the cookbook recipe "Your network stack" (`NetworkStackRecipe.kt`), run through the Http adapter against a local server. */
class NetworkStackRecipeTest {
    private lateinit var server: TestHttpServer

    @Before
    fun setUp() {
        server = TestHttpServer()
        server.route("/secure") { ex ->
            if (ex.request.header("Authorization") == "Bearer fresh") ex.respond(200, body = "welcome".toByteArray()) else ex.respond(401, listOf("WWW-Authenticate" to "Bearer"))
        }
    }

    @After
    fun tearDown() {
        server.close()
    }

    private fun get(adapter: OkHttpHttpAdapter) = runBlocking {
        adapter.request(HttpRequest(HttpMethod.GET, server.base + "/secure", listOf(Header("Accept", "text/plain")), null, 10_000u))
    }

    @Test
    fun the_token_is_added_a_401_is_refreshed_and_the_request_goes_again() {
        val refreshed = AtomicInteger(0)
        val tokens = TokenStore("stale") { refreshed.incrementAndGet(); "fresh" }
        val answer = get(OkHttpHttpAdapter(appClient(tokens)))
        assertEquals(200.toUShort(), answer.status)
        assertEquals("welcome", String(answer.body))
        assertEquals(1, refreshed.get())
        assertEquals(listOf("Bearer stale", "Bearer fresh"), server.requests.map { it.header("Authorization") })
        // The next request starts with the refreshed token.
        assertEquals(200.toUShort(), get(OkHttpHttpAdapter(appClient(tokens))).status)
        assertEquals("Bearer fresh", server.requests.last().header("Authorization"))
        assertEquals(1, refreshed.get())
    }

    @Test
    fun a_refresh_that_does_not_help_is_not_repeated_and_the_401_is_the_answer() {
        val refreshed = AtomicInteger(0)
        val tokens = TokenStore("stale") { refreshed.incrementAndGet(); "still-wrong" }
        val answer = get(OkHttpHttpAdapter(appClient(tokens)))
        assertEquals(401.toUShort(), answer.status)
        assertEquals(1, refreshed.get())
        assertEquals(2, server.requests.size)
    }

    @Test
    fun an_ended_session_is_a_response_not_a_failure() {
        val answer = get(OkHttpHttpAdapter(appClient(TokenStore("stale") { null })))
        assertEquals(401.toUShort(), answer.status)
        assertEquals(1, server.requests.size) // nothing to retry with
    }
}
