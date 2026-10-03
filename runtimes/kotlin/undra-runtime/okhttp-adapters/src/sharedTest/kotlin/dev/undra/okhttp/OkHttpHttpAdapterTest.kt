package dev.undra.okhttp

import dev.undra.android.HttpAdapterContract
import dev.undra.android.HttpUnderTest
import dev.undra.android.call
import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.support.eventually
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.zip.GZIPOutputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.runBlocking
import okhttp3.Authenticator
import okhttp3.Call
import okhttp3.EventListener
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * `OkHttpHttpAdapter` against the contract every Http adapter of the Kotlin runtime meets (`HttpAdapterContract`, shared with
 * `AndroidHttpAdapterTest`: ADR-060), on a plain `OkHttpClient`, and then what the adapter is for: the app's client is in the
 * path, so its interceptors, its `Authenticator` and its event listeners see every request. The same tests run on the desktop JVM
 * (unit tests) and on the device (instrumented tests).
 */
class OkHttpHttpAdapterTest : HttpAdapterContract() {
    override fun create(maxResponseBytes: Int?, idleTimeoutMs: Int?): HttpUnderTest {
        val client = OkHttpClient.Builder().apply {
            if (idleTimeoutMs != null) readTimeout(idleTimeoutMs.toLong(), TimeUnit.MILLISECONDS)
        }.build()
        return OkHttpHttpAdapter(client, maxResponseBytes ?: OkHttpHttpAdapter.DEFAULT_MAX_RESPONSE_BYTES).under()
    }

    // OkHttp gives up on a call after 20 follow-ups: the 21st request is the one it does not make.
    override val maxRedirects: Int get() = OkHttpRules.MAX_FOLLOW_UPS

    // OkHttp sends PATCH, closes the socket of a cancelled call and keeps the server's order of repeated headers on every runtime.
    override val canSendPatch: Boolean get() = true
    override val cancelClosesTheSocketMidBody: Boolean get() = true
    override val keepsServerOrderOfRepeatedHeaders: Boolean get() = true

    private fun OkHttpHttpAdapter.under(): HttpUnderTest = object : HttpUnderTest {
        override suspend fun request(request: HttpRequest): HttpResponse = this@under.request(request)

        override fun portImpl(): PortImpl = this@under.portImpl()
    }

    private fun get(adapter: OkHttpHttpAdapter, path: String, headers: List<Header> = emptyList(), timeoutMs: UInt? = null): HttpResponse =
        runBlocking { adapter.request(HttpRequest(HttpMethod.GET, server.base + path, headers, null, timeoutMs)) }

    private fun failure(block: () -> Any?): HttpError {
        try {
            block()
        } catch (e: HttpError) {
            return e
        }
        throw AssertionError("expected an HttpError")
    }

    // ---- the app's stack is in the path ---------------------------------------------------------------------

    @Test
    fun an_interceptor_of_the_client_sees_every_request_and_can_change_a_header() {
        val seen = CopyOnWriteArrayList<String>()
        val hops = CopyOnWriteArrayList<String>()
        val client = OkHttpClient.Builder()
            .addInterceptor(
                Interceptor { chain ->
                    val request = chain.request()
                    seen += "${request.method} ${request.url.encodedPath}"
                    chain.proceed(request.newBuilder().header("X-Traced", "yes").header("Accept", "text/plain").build())
                },
            )
            .addNetworkInterceptor(Interceptor { chain -> hops += chain.request().url.encodedPath; chain.proceed(chain.request()) })
            .build()
        val adapter = OkHttpHttpAdapter(client)
        server.fixed("/a", 200, "a")
        server.fixed("/b", 201, "b")
        server.route("/old") { it.respond(302, listOf("Location" to "/new")) }
        server.fixed("/new", 200, "arrived")

        assertEquals("a", String(get(adapter, "/a").body))
        val post = runBlocking { adapter.request(HttpRequest(HttpMethod.POST, server.base + "/b", listOf(Header("Content-Type", "text/plain")), "hello".toByteArray(), null)) }
        assertEquals(201.toUShort(), post.status)
        // Through the port method, the way the core calls it.
        val request = HttpRequest(HttpMethod.GET, server.base + "/a", emptyList(), null, 5_000u)
        val reply = HttpResponse.decodeAll(call(adapter.portImpl(), StandardPorts.Http.REQUEST, HttpRequest.encodeToByteArray(request)))
        assertEquals("a", String(reply.body))
        assertEquals("arrived", String(get(adapter, "/old").body))

        // The application interceptor sees each call once, a redirect chain included; the network interceptor sees every hop.
        assertEquals(listOf("GET /a", "POST /b", "GET /a", "GET /old"), seen.toList())
        assertEquals(listOf("/a", "/b", "/a", "/old", "/new"), hops.toList())
        // and what it added reached the server on every request, the redirect's second hop too.
        assertEquals(5, server.requests.size)
        assertTrue(server.requests.joinToString { it.path }, server.requests.all { it.header("X-Traced") == "yes" })
        assertTrue(server.requests.all { it.header("Accept") == "text/plain" })
        assertEquals("hello", String(server.requests[1].body))
    }

    @Test
    fun an_interceptor_can_fail_the_request_and_the_core_sees_a_typed_network_error() {
        val client = OkHttpClient.Builder().addInterceptor(Interceptor { throw IOException("the app's policy says no") }).build()
        val e = failure { get(OkHttpHttpAdapter(client), "/never") }
        assertTrue(e.toString(), e is HttpError.Network && e.reason.contains("the app's policy says no"))
        assertEquals(0, server.requests.size)
    }

    @Test
    fun the_clients_authenticator_refreshes_the_token_and_the_request_goes_again() {
        server.route("/secure") { ex ->
            val token = ex.request.header("Authorization")
            if (token == "Bearer fresh") ex.respond(200, body = "welcome ${ex.request.body.size}".toByteArray()) else ex.respond(401, listOf("WWW-Authenticate" to "Bearer realm=\"api\""))
        }
        val refreshed = AtomicInteger(0)
        val client = OkHttpClient.Builder()
            .authenticator(
                Authenticator { _, response ->
                    if (response.request.header("Authorization") == "Bearer fresh") {
                        null // already tried: give up
                    } else {
                        refreshed.incrementAndGet()
                        response.request.newBuilder().header("Authorization", "Bearer fresh").build()
                    }
                },
            )
            .build()
        val adapter = OkHttpHttpAdapter(client)

        val answer = get(adapter, "/secure", listOf(Header("Authorization", "Bearer stale")))
        assertEquals(200.toUShort(), answer.status)
        assertEquals("welcome 0", String(answer.body))
        assertEquals(1, refreshed.get())
        assertEquals(listOf("Bearer stale", "Bearer fresh"), server.requests.map { it.header("Authorization") })

        // A request with a body is sent again whole.
        val posted = runBlocking {
            adapter.request(HttpRequest(HttpMethod.POST, server.base + "/secure", listOf(Header("Authorization", "Bearer stale")), "12345".toByteArray(), null))
        }
        assertEquals("welcome 5", String(posted.body))
        assertEquals(2, refreshed.get())
        assertEquals(listOf("5", "5"), server.requests.takeLast(2).map { it.header("Content-Length") })

        // An authenticator that has nothing to offer leaves the 401 as the answer, which is a response, not a failure.
        val refusing = OkHttpHttpAdapter(OkHttpClient.Builder().authenticator(Authenticator { _, _ -> null }).build())
        assertEquals(401.toUShort(), get(refusing, "/secure").status)
    }

    @Test
    fun the_clients_event_listener_sees_each_call_from_start_to_end() {
        val events = CopyOnWriteArrayList<String>()
        val listener = object : EventListener() {
            override fun callStart(call: Call) {
                events += "start ${call.request().url.encodedPath}"
            }

            override fun callEnd(call: Call) {
                events += "end"
            }

            override fun callFailed(call: Call, ioe: IOException) {
                events += "failed"
            }
        }
        val adapter = OkHttpHttpAdapter(OkHttpClient.Builder().eventListener(listener).build())
        server.fixed("/traced", 200, "ok")

        get(adapter, "/traced")
        assertEquals(listOf("start /traced", "end"), events.toList())

        events.clear()
        val closed = dev.undra.android.TestHttpServer().use { it.port }
        failure { runBlocking { adapter.request(HttpRequest(HttpMethod.GET, "http://127.0.0.1:$closed/x", emptyList(), null, 5_000u)) } }
        assertEquals(listOf("start /x", "failed"), events.toList())
    }

    // ---- the client's settings are respected -----------------------------------------------------------------

    @Test
    fun a_client_provider_is_asked_for_every_request_so_a_replaced_client_is_followed() {
        server.fixed("/x", 200, "ok")
        val tagged = AtomicInteger(0)
        var current = OkHttpClient()
        val adapter = OkHttpHttpAdapter({ current })
        get(adapter, "/x")
        assertNull(server.requests.last().header("X-Generation"))

        current = OkHttpClient.Builder().addInterceptor(Interceptor { it.proceed(it.request().newBuilder().header("X-Generation", "${tagged.incrementAndGet()}").build()) }).build()
        get(adapter, "/x")
        assertEquals("1", server.requests.last().header("X-Generation"))
    }

    @Test
    fun a_client_that_cannot_be_provided_is_a_network_error() {
        val e = failure { get(OkHttpHttpAdapter({ throw IllegalStateException("the graph is gone") }), "/x") }
        assertTrue(e.toString(), e is HttpError.Network && e.reason.contains("the graph is gone"))
    }

    @Test
    fun a_client_with_a_call_timeout_gives_a_timeout_and_the_requests_own_timeout_still_applies() {
        server.route("/never") { it.awaitClientClose(10_000) }
        val adapter = OkHttpHttpAdapter(OkHttpClient.Builder().callTimeout(300, TimeUnit.MILLISECONDS).build())
        assertEquals(HttpError.Timeout, failure { get(adapter, "/never") })
        // The request's own timeout is the shorter here, and is a Timeout too.
        val patient = OkHttpHttpAdapter(OkHttpClient.Builder().callTimeout(60, TimeUnit.SECONDS).build())
        assertEquals(HttpError.Timeout, failure { get(patient, "/never", timeoutMs = 300u) })
    }

    @Test
    fun the_clients_redirect_policy_is_the_policy() {
        server.route("/old") { it.respond(302, listOf("Location" to "/new")) }
        server.fixed("/new", 200, "arrived")
        val staying = OkHttpHttpAdapter(OkHttpClient.Builder().followRedirects(false).followSslRedirects(false).build())
        val answer = get(staying, "/old")
        assertEquals(302.toUShort(), answer.status)
        assertTrue(answer.headers.contains(Header("Location", "/new")))
        assertEquals(1, server.requests.size)
    }

    @Test
    fun a_gzip_answer_arrives_decoded_and_the_encoding_header_is_not_passed_on() {
        val plain = "compress me ".repeat(500).toByteArray()
        val zipped = ByteArrayOutputStream().also { out -> GZIPOutputStream(out).use { it.write(plain) } }.toByteArray()
        server.route("/gz") { ex ->
            val accepts = ex.request.header("Accept-Encoding").orEmpty().contains("gzip")
            if (accepts) ex.respond(200, listOf("Content-Encoding" to "gzip"), zipped) else ex.respond(200, body = plain)
        }
        val answer = get(OkHttpHttpAdapter(OkHttpClient()), "/gz")
        assertEquals(String(plain), String(answer.body))
        assertFalse("the Content-Encoding header describes bytes the core never sees", answer.headers.contains(Header("Content-Encoding", "gzip")))
    }

    @Test
    fun a_header_value_that_is_not_printable_ascii_is_refused_naming_the_header() {
        val e = failure { get(OkHttpHttpAdapter(OkHttpClient()), "/x", listOf(Header("X-Name", "José"))) }
        assertTrue(e.toString(), e is HttpError.InvalidUrl && e.reason.contains("X-Name"))
        assertEquals(0, server.requests.size)
    }

    @Test
    fun calls_cancelled_or_timed_out_leave_no_call_running_and_no_connection_in_use() = runBlocking {
        // The server holds every call open for longer than the test, and the client has no read timeout: a call the adapter
        // abandoned instead of cancelling would stay running.
        server.route("/hang") { it.awaitClientClose(60_000) }
        server.route("/midbody") { ex ->
            ex.startChunked()
            ex.chunk(ByteArray(1000))
            ex.awaitClientClose(60_000)
        }
        val client = OkHttpClient.Builder().readTimeout(0, TimeUnit.MILLISECONDS).build()
        val adapter = OkHttpHttpAdapter(client)
        var sent = 0
        repeat(3) {
            for (path in listOf("/hang", "/midbody")) {
                val call = async(Dispatchers.Default) { adapter.request(HttpRequest(HttpMethod.GET, server.base + path, emptyList(), null, null)) }
                assertTrue(server.awaitRequests(++sent))
                call.cancelAndJoin()
            }
        }
        assertEquals(HttpError.Timeout, failure { runBlocking { adapter.request(HttpRequest(HttpMethod.GET, server.base + "/hang", emptyList(), null, 200u)) } })
        // Each call was cancelled on OkHttp (Call.cancel), not abandoned: its dispatcher thread is back and its socket closed.
        eventually("the dispatcher has no call left and the pool no connection in use") {
            client.dispatcher.runningCallsCount() == 0 && client.dispatcher.queuedCallsCount() == 0 &&
                client.connectionPool.connectionCount() == client.connectionPool.idleConnectionCount()
        }
        assertTrue(server.awaitIdle())
    }

    @Test
    fun the_default_limit_is_the_documented_one() {
        assertEquals(64 * 1024 * 1024, OkHttpHttpAdapter.DEFAULT_MAX_RESPONSE_BYTES)
    }
}
