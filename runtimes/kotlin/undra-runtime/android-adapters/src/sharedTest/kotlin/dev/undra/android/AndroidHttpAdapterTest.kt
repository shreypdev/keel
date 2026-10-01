package dev.undra.android

import dev.undra.runtime.UndraPortException
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test

/**
 * The Http adapter against a real HTTP server on the loopback interface. The same tests run on the desktop JVM (unit
 * tests) and on the device (instrumented tests, where `HttpURLConnection` is Android's own).
 */
class AndroidHttpAdapterTest {
    private lateinit var server: TestHttpServer
    private val http = AndroidHttpAdapter()

    @Before
    fun startServer() {
        server = TestHttpServer()
    }

    @After
    fun stopServer() {
        server.close()
    }

    private fun get(path: String, headers: List<Header> = emptyList(), timeoutMs: UInt? = null, adapter: AndroidHttpAdapter = http): HttpResponse =
        runBlocking { adapter.request(HttpRequest(HttpMethod.GET, server.base + path, headers, null, timeoutMs)) }

    private fun send(method: HttpMethod, path: String, body: ByteArray?, headers: List<Header> = emptyList()): HttpResponse =
        runBlocking { http.request(HttpRequest(method, server.base + path, headers, body, null)) }

    private fun failure(block: () -> Any?): HttpError {
        try {
            block()
        } catch (e: HttpError) {
            return e
        }
        throw AssertionError("expected an HttpError")
    }

    private fun HttpResponse.text(): String = String(body, Charsets.UTF_8)

    // ---- responses -------------------------------------------------------------------------------------------

    @Test
    fun a_get_returns_the_status_the_headers_and_the_body() {
        server.fixed("/hello", 200, "hello, world", listOf("Content-Type" to "text/plain", "X-Thing" to "1"))
        val response = get("/hello")
        assertEquals(200.toUShort(), response.status)
        assertEquals("hello, world", response.text())
        assertTrue(response.headers.contains(Header("Content-Type", "text/plain")))
        assertTrue(response.headers.contains(Header("X-Thing", "1")))
        assertEquals("GET", server.requests.single().method)
    }

    @Test
    fun an_error_status_is_a_response_not_a_failure() {
        server.fixed("/missing", 404, "no such thing")
        server.fixed("/boom", 500, "{\"error\":\"boom\"}")
        server.fixed("/denied", 401, "")
        assertEquals(404.toUShort(), get("/missing").status)
        assertEquals("no such thing", get("/missing").text())
        assertEquals(500.toUShort(), get("/boom").status)
        assertEquals("{\"error\":\"boom\"}", get("/boom").text())
        assertEquals(401.toUShort(), get("/denied").status)
    }

    @Test
    fun a_no_content_answer_and_a_head_request_have_an_empty_body() {
        server.route("/empty") { it.respond(204) }
        server.route("/head") { it.respond(200, listOf("X-Size" to "123"), ByteArray(0), contentLength = 123) }
        val none = get("/empty")
        assertEquals(204.toUShort(), none.status)
        assertEquals(0, none.body.size)
        val head = runBlocking { http.request(HttpRequest(HttpMethod.HEAD, server.base + "/head", emptyList(), null, null)) }
        assertEquals(200.toUShort(), head.status)
        assertEquals(0, head.body.size)
        assertTrue(head.headers.contains(Header("X-Size", "123")))
    }

    @Test
    fun repeated_response_headers_are_all_reported_sorted_by_name() {
        server.route("/cookies") {
            it.respond(200, listOf("Set-Cookie" to "a=1", "X-Z" to "z", "Set-Cookie" to "b=2", "Content-Type" to "text/plain"), "x".toByteArray())
        }
        val headers = get("/cookies").headers
        val names = headers.map { it.name }
        assertEquals(names.sortedBy { it.lowercase() }, names)
        val cookies = headers.filter { it.name == "Set-Cookie" }.map { it.value }
        // Android lists a repeated header in the order the server sent it; the desktop JVM's HttpURLConnection reverses it.
        if (isAndroidRuntime) assertEquals(listOf("a=1", "b=2"), cookies) else assertEquals(setOf("a=1", "b=2"), cookies.toSet())
    }

    @Test
    fun a_binary_body_survives_untouched() {
        val everyByte = ByteArray(256) { it.toByte() }
        server.route("/bin") { it.respond(200, listOf("Content-Type" to "application/octet-stream"), everyByte) }
        assertArrayEquals(everyByte, get("/bin").body)
    }

    @Test
    fun a_large_chunked_body_is_assembled_in_order() {
        val payload = ByteArray(5 * 1024 * 1024) { (it * 31).toByte() }
        server.route("/big") { ex ->
            ex.startChunked()
            var offset = 0
            while (offset < payload.size) {
                val n = minOf(100_000, payload.size - offset)
                ex.chunk(payload.copyOfRange(offset, offset + n))
                offset += n
            }
            ex.endChunked()
        }
        assertArrayEquals(payload, get("/big").body)
    }

    @Test
    fun a_body_that_arrives_in_slow_chunks_is_one_response() {
        server.route("/slow") { ex ->
            ex.startChunked()
            for (part in listOf("one ", "two ", "three")) {
                ex.chunk(part.toByteArray())
                Thread.sleep(80)
            }
            ex.endChunked()
        }
        assertEquals("one two three", get("/slow").text())
    }

    @Test
    fun a_body_larger_than_the_limit_fails_with_a_network_error_declared_or_not() {
        val small = AndroidHttpAdapter(maxResponseBytes = 1_000)
        server.route("/declared") { it.respond(200, body = ByteArray(5_000)) }
        server.route("/streamed") { ex ->
            ex.startChunked()
            repeat(10) { ex.chunk(ByteArray(500)) }
            ex.endChunked()
        }
        for (path in listOf("/declared", "/streamed")) {
            val e = failure { get(path, adapter = small) }
            assertTrue("$path: $e", e is HttpError.Network && e.reason.contains("limit of 1000 bytes"))
        }
        server.route("/fits") { it.respond(200, body = ByteArray(1_000)) }
        assertEquals(1_000, get("/fits", adapter = small).body.size)
    }

    @Test
    fun nothing_is_cached_or_remembered_between_requests() {
        server.route("/cacheable") { it.respond(200, listOf("Cache-Control" to "max-age=3600", "Set-Cookie" to "s=1"), "x".toByteArray()) }
        get("/cacheable")
        get("/cacheable")
        assertEquals(2, server.requests.size)
        assertNull(server.requests[1].header("Cookie"))
    }

    // ---- requests -------------------------------------------------------------------------------------------

    @Test
    fun request_headers_arrive_with_repeats_and_the_ones_the_connection_manages_are_dropped() {
        server.fixed("/h", 200, "ok")
        get("/h", listOf(Header("X-Tag", "1"), Header("X-Tag", "2"), Header("Accept", "application/json"), Header("Host", "evil.example"), Header("Content-Length", "99")))
        val request = server.requests.single()
        assertEquals(listOf("1", "2"), request.headers("X-Tag").flatMap { it.split(",").map(String::trim) })
        assertEquals("application/json", request.header("Accept"))
        assertEquals("127.0.0.1:${server.port}", request.header("Host"))
        assertNull(request.header("Content-Length"))
    }

    @Test
    fun a_post_sends_its_body_and_content_type() {
        server.route("/echo") { ex -> ex.respond(200, listOf("Content-Type" to "application/json"), ex.request.body) }
        val body = "{\"title\":\"café ☕\"}".toByteArray(Charsets.UTF_8)
        val response = send(HttpMethod.POST, "/echo", body, listOf(Header("Content-Type", "application/json")))
        assertArrayEquals(body, response.body)
        val request = server.requests.single()
        assertEquals("POST", request.method)
        assertEquals("application/json", request.header("Content-Type"))
        assertEquals(body.size.toString(), request.header("Content-Length"))
    }

    @Test
    fun put_and_delete_and_options_carry_their_methods_and_bodies() {
        server.route("/m") { ex -> ex.respond(200, listOf("X-Method" to ex.request.method), ex.request.body) }
        assertEquals("PUT", send(HttpMethod.PUT, "/m", "p".toByteArray()).headers.first { it.name == "X-Method" }.value)
        assertEquals("p", String(send(HttpMethod.PUT, "/m", "p".toByteArray()).body))
        assertEquals("DELETE", send(HttpMethod.DELETE, "/m", null).headers.first { it.name == "X-Method" }.value)
        assertEquals("OPTIONS", send(HttpMethod.OPTIONS, "/m", null).headers.first { it.name == "X-Method" }.value)
    }

    @Test
    fun a_patch_request_is_sent_as_a_patch_on_android() {
        // The desktop JVM's HttpURLConnection cannot send PATCH (the Playground's server uses it); Android's can.
        assumeTrue(isAndroidRuntime)
        server.route("/m") { ex -> ex.respond(200, listOf("X-Method" to ex.request.method), ex.request.body) }
        val response = send(HttpMethod.PATCH, "/m", "{\"done\":true}".toByteArray(), listOf(Header("Content-Type", "application/json")))
        assertEquals("PATCH", response.headers.first { it.name == "X-Method" }.value)
        assertEquals("{\"done\":true}", response.text())
    }

    @Test
    fun a_post_without_a_body_says_so_with_a_zero_content_length() {
        server.fixed("/empty", 200, "ok")
        send(HttpMethod.POST, "/empty", null)
        val request = server.requests.single()
        assertEquals("POST", request.method)
        assertEquals("0", request.header("Content-Length"))
    }

    @Test
    fun a_megabyte_request_body_arrives_intact() {
        val payload = ByteArray(1024 * 1024) { (it * 7).toByte() }
        server.route("/up") { ex -> ex.respond(200, body = "got ${ex.request.body.size} ${ex.request.body.contentHashCode()}".toByteArray()) }
        val response = send(HttpMethod.POST, "/up", payload, listOf(Header("Content-Type", "application/octet-stream")))
        assertEquals("got ${payload.size} ${payload.contentHashCode()}", response.text())
    }

    @Test
    fun a_get_with_a_body_is_refused_before_anything_is_sent() {
        val e = failure { send(HttpMethod.GET, "/x", byteArrayOf(1)) }
        assertTrue(e.toString(), e is HttpError.InvalidUrl && e.reason.contains("cannot carry a body"))
        assertEquals(0, server.requests.size)
    }

    @Test
    fun an_unusable_url_or_header_is_invalid() {
        for (url in listOf("ftp://example.com/", "example.com", "http://", "http://exa mple.com/", "file:///etc/passwd")) {
            val e = failure { runBlocking { http.request(HttpRequest(HttpMethod.GET, url, emptyList(), null, null)) } }
            assertTrue("$url: $e", e is HttpError.InvalidUrl)
        }
        val bad = failure { get("/x", listOf(Header("X-Bad", "line\nbreak"))) }
        assertTrue(bad.toString(), bad is HttpError.InvalidUrl && bad.reason.contains("X-Bad"))
        assertEquals(0, server.requests.size)
    }

    // ---- redirects -------------------------------------------------------------------------------------------

    @Test
    fun a_redirect_is_followed_to_its_target() {
        server.route("/old") { it.respond(302, listOf("Location" to "/new")) }
        server.fixed("/new", 200, "arrived")
        assertEquals("arrived", get("/old").text())
        assertEquals(listOf("/old", "/new"), server.requests.map { it.path })
    }

    @Test
    fun a_post_redirected_with_303_or_302_becomes_a_get_and_with_307_stays_a_post() {
        server.route("/see-other") { it.respond(303, listOf("Location" to "/target")) }
        server.route("/found") { it.respond(302, listOf("Location" to "/target")) }
        server.route("/temporary") { it.respond(307, listOf("Location" to "/target")) }
        server.route("/target") { ex -> ex.respond(200, listOf("X-Method" to ex.request.method), ex.request.body) }
        val payload = "payload".toByteArray()
        for (path in listOf("/see-other", "/found")) {
            val r = send(HttpMethod.POST, path, payload, listOf(Header("Content-Type", "text/plain")))
            assertEquals("GET", r.headers.first { it.name == "X-Method" }.value)
            assertEquals(0, r.body.size)
        }
        val kept = send(HttpMethod.POST, "/temporary", payload, listOf(Header("Content-Type", "text/plain")))
        assertEquals("POST", kept.headers.first { it.name == "X-Method" }.value)
        assertEquals("payload", kept.text())
    }

    @Test
    fun a_redirect_to_another_host_does_not_carry_credentials() {
        server.route("/start") { it.respond(307, listOf("Location" to "http://localhost:${server.port}/there")) }
        server.fixed("/there", 200, "ok")
        get("/start", listOf(Header("Authorization", "Bearer secret"), Header("X-Keep", "yes")))
        val first = server.requests[0]
        val second = server.requests[1]
        assertEquals("Bearer secret", first.header("Authorization"))
        assertNull(second.header("Authorization"))
        assertEquals("yes", second.header("X-Keep"))
        assertEquals("localhost:${server.port}", second.header("Host"))
    }

    @Test
    fun a_redirect_loop_ends_in_a_network_error() {
        server.route("/loop") { it.respond(302, listOf("Location" to "/loop")) }
        val e = failure { get("/loop") }
        assertTrue(e.toString(), e is HttpError.Network && e.reason.contains("too many redirects"))
        assertEquals(Redirects.MAX_HOPS + 1, server.requests.size)
    }

    @Test
    fun a_redirect_without_a_location_is_the_answer() {
        server.route("/odd") { it.respond(302, body = "no location".toByteArray()) }
        val r = get("/odd")
        assertEquals(302.toUShort(), r.status)
        assertEquals("no location", r.text())
    }

    // ---- failures -----------------------------------------------------------------------------------------

    @Test
    fun a_closed_port_is_a_network_error() {
        val port = TestHttpServer().use { it.port } // closed again by use
        val e = failure { runBlocking { http.request(HttpRequest(HttpMethod.GET, "http://127.0.0.1:$port/", emptyList(), null, 5_000u)) } }
        assertTrue(e.toString(), e is HttpError.Network)
    }

    @Test
    fun a_name_that_does_not_resolve_is_a_network_error() {
        val e = failure { runBlocking { http.request(HttpRequest(HttpMethod.GET, "http://no-such-host.invalid/", emptyList(), null, 10_000u)) } }
        assertTrue(e.toString(), e is HttpError.Network)
    }

    @Test
    fun a_connection_dropped_without_an_answer_is_a_network_error() {
        server.route("/drop") { it.drop() }
        val e = failure { get("/drop") }
        assertTrue(e.toString(), e is HttpError.Network)
    }

    @Test
    fun a_garbage_answer_is_a_network_error() {
        server.route("/garbage") { it.raw("this is not http\r\n\r\n") }
        val e = failure { get("/garbage") }
        assertTrue(e.toString(), e is HttpError.Network)
    }

    // ---- timeouts -----------------------------------------------------------------------------------------

    @Test
    fun a_server_that_never_answers_times_out_at_the_requests_timeout() {
        server.route("/never") { it.awaitClientClose(10_000) }
        val started = System.nanoTime()
        val e = failure { get("/never", timeoutMs = 300u) }
        val elapsedMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started)
        assertEquals(HttpError.Timeout, e)
        assertTrue("took $elapsedMs ms", elapsedMs in 250..3_000)
    }

    @Test
    fun the_timeout_covers_the_body_not_just_the_headers() {
        server.route("/stall") { ex ->
            ex.startChunked()
            ex.chunk("first".toByteArray())
            ex.awaitClientClose(10_000) // then says nothing more
        }
        val started = System.nanoTime()
        val e = failure { get("/stall", timeoutMs = 400u) }
        val elapsedMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started)
        assertEquals(HttpError.Timeout, e)
        assertTrue("took $elapsedMs ms", elapsedMs in 350..3_000)
    }

    @Test
    fun the_timeout_covers_a_body_that_trickles_in_and_never_stops() {
        server.route("/trickle") { ex ->
            ex.startChunked()
            repeat(200) {
                ex.chunk("x".toByteArray())
                Thread.sleep(20)
            }
            ex.endChunked()
        }
        val started = System.nanoTime()
        val e = failure { get("/trickle", timeoutMs = 500u) }
        val elapsedMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started)
        assertEquals(HttpError.Timeout, e) // each read is within the idle timeout; the whole is not within the request's
        assertTrue("took $elapsedMs ms", elapsedMs in 450..3_000)
    }

    @Test
    fun without_a_request_timeout_a_connection_that_goes_silent_hits_the_idle_timeout() {
        val impatient = AndroidHttpAdapter(idleTimeoutMs = 300)
        server.route("/silent") { it.awaitClientClose(10_000) }
        val e = failure { get("/silent", adapter = impatient) }
        assertEquals(HttpError.Timeout, e)
    }

    @Test
    fun a_zero_timeout_times_out_at_once() {
        server.fixed("/fast", 200, "ok")
        assertEquals(HttpError.Timeout, failure { get("/fast", timeoutMs = 0u) })
    }

    // ---- cancellation ---------------------------------------------------------------------------------------

    @Test
    fun cancelling_the_caller_aborts_the_connection_and_returns_at_once() = runBlocking {
        val arrived = CompletableDeferred<Unit>()
        val closedByClient = CompletableDeferred<Boolean>()
        server.route("/hang") { ex ->
            arrived.complete(Unit)
            closedByClient.complete(ex.awaitClientClose(5_000))
        }
        val call = async(Dispatchers.Default) { http.request(HttpRequest(HttpMethod.GET, server.base + "/hang", emptyList(), null, null)) }
        withTimeout(5_000) { arrived.await() }
        val started = System.nanoTime()
        call.cancel()
        val cancelled = try {
            call.await()
            false
        } catch (e: CancellationException) {
            true
        }
        val elapsedMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started)
        assertTrue("the caller saw a CancellationException", cancelled)
        assertTrue("returned in $elapsedMs ms", elapsedMs < 2_000)
        assertTrue("the server saw the connection close", withTimeout(5_000) { closedByClient.await() })
    }

    @Test
    fun cancelling_in_the_middle_of_a_body_aborts_the_connection() = runBlocking {
        val firstChunkSent = CompletableDeferred<Unit>()
        val closedByClient = CompletableDeferred<Boolean>()
        server.route("/midbody") { ex ->
            ex.startChunked()
            ex.chunk(ByteArray(1000))
            firstChunkSent.complete(Unit)
            closedByClient.complete(ex.awaitClientClose(5_000))
        }
        val call = async(Dispatchers.Default) { http.request(HttpRequest(HttpMethod.GET, server.base + "/midbody", emptyList(), null, null)) }
        withTimeout(5_000) { firstChunkSent.await() }
        Thread.sleep(100) // the client is now blocked reading the next chunk
        call.cancelAndJoin()
        assertTrue(call.isCancelled)
        // Android's HttpURLConnection closes the socket on disconnect(); the desktop JVM's may try to drain a chunked body first.
        if (isAndroidRuntime) assertTrue("the server saw the connection close", withTimeout(5_000) { closedByClient.await() })
    }

    @Test
    fun cancelling_before_the_request_starts_sends_nothing() = runBlocking {
        val call = async(Dispatchers.Default, start = CoroutineStart.LAZY) { http.request(HttpRequest(HttpMethod.GET, server.base + "/x", emptyList(), null, null)) }
        call.cancel()
        call.join()
        Thread.sleep(100)
        assertEquals(0, server.requests.size)
    }

    @Test
    fun an_adapter_keeps_working_after_calls_were_cancelled() = runBlocking {
        server.route("/hang") { it.awaitClientClose(5_000) }
        server.fixed("/ok", 200, "fine")
        repeat(5) {
            val call = async(Dispatchers.Default) { http.request(HttpRequest(HttpMethod.GET, server.base + "/hang", emptyList(), null, null)) }
            server.awaitRequests(it + 1)
            call.cancelAndJoin()
        }
        assertEquals("fine", http.request(HttpRequest(HttpMethod.GET, server.base + "/ok", emptyList(), null, null)).text())
    }

    @Test
    fun a_timeout_aborts_the_connection_too() = runBlocking {
        val closedByClient = CompletableDeferred<Boolean>()
        server.route("/hang") { ex -> closedByClient.complete(ex.awaitClientClose(5_000)) }
        val e = try {
            http.request(HttpRequest(HttpMethod.GET, server.base + "/hang", emptyList(), null, 300u))
            null
        } catch (e: HttpError) {
            e
        }
        assertEquals(HttpError.Timeout, e)
        assertTrue(withTimeout(5_000) { closedByClient.await() })
    }

    // ---- concurrency and the port ---------------------------------------------------------------------------

    @Test
    fun many_requests_at_once_each_get_their_own_answer() = runBlocking {
        server.route("/n") { ex ->
            Thread.sleep(20)
            ex.respond(200, body = ex.request.target.toByteArray())
        }
        val answers = (1..40).map { i -> async(Dispatchers.Default) { http.request(HttpRequest(HttpMethod.GET, server.base + "/n?i=$i", emptyList(), null, null)) } }.awaitAll()
        assertEquals((1..40).map { "/n?i=$it" }, answers.map { String(it.body) })
    }

    @Test
    fun the_port_method_decodes_the_request_and_encodes_the_response() {
        server.fixed("/p", 201, "created", listOf("X-A" to "b"))
        val impl = http.portImpl()
        assertFalse(impl.sync)
        val request = HttpRequest(HttpMethod.POST, server.base + "/p", listOf(Header("Content-Type", "text/plain")), "hi".toByteArray(), 5_000u)
        val reply = HttpResponse.decodeAll(call(impl, StandardPorts.Http.REQUEST, HttpRequest.encodeToByteArray(request)))
        assertEquals(201.toUShort(), reply.status)
        assertEquals("created", String(reply.body))
        assertTrue(reply.headers.contains(Header("X-A", "b")))
    }

    @Test
    fun the_port_method_fails_with_the_encoded_typed_error() {
        val impl = http.portImpl()
        val request = HttpRequest(HttpMethod.GET, "gopher://example.com/", emptyList(), null, null)
        val e = try {
            call(impl, StandardPorts.Http.REQUEST, HttpRequest.encodeToByteArray(request))
            null
        } catch (e: UndraPortException) {
            e
        }
        assertNotNull(e)
        val typed = HttpError.decodeAll(e!!.body)
        assertTrue(typed.toString(), typed is HttpError.InvalidUrl)
    }

    @Test
    fun the_port_method_reports_a_timeout_as_the_timeout_variant() {
        server.route("/never") { it.awaitClientClose(10_000) }
        val request = HttpRequest(HttpMethod.GET, server.base + "/never", emptyList(), null, 250u)
        val e = try {
            call(http.portImpl(), StandardPorts.Http.REQUEST, HttpRequest.encodeToByteArray(request))
            null
        } catch (e: UndraPortException) {
            e
        }
        assertEquals(HttpError.Timeout, HttpError.decodeAll(e!!.body))
    }

    @Test
    fun the_default_limits_are_the_documented_ones() {
        assertEquals(30_000, AndroidHttpAdapter.DEFAULT_CONNECT_TIMEOUT_MS)
        assertEquals(60_000, AndroidHttpAdapter.DEFAULT_IDLE_TIMEOUT_MS)
        assertEquals(64 * 1024 * 1024, AndroidHttpAdapter.DEFAULT_MAX_RESPONSE_BYTES)
    }
}
