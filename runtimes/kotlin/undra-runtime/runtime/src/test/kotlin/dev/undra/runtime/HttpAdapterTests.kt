package dev.undra.runtime

import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpAdapter
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.eventually
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.net.InetAddress
import java.net.InetSocketAddress
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.jupiter.api.Test

/** What the test server saw of a request. */
private class Seen(val method: String, val path: String, val headers: Map<String, List<String>>, val body: ByteArray)

private class TestHttpServer : AutoCloseable {
    private val server = HttpServer.create(InetSocketAddress(InetAddress.getLoopbackAddress(), 0), 0)
    val seen = CopyOnWriteArrayList<Seen>()
    val base: String get() = "http://127.0.0.1:${server.address.port}"

    init {
        server.executor = Executors.newCachedThreadPool { r -> Thread(r).also { it.isDaemon = true } }
        server.createContext("/") { ex ->
            record(ex)
            respond(ex, 200, "hello".toByteArray(), mapOf("Content-Type" to listOf("text/plain"), "X-Multi" to listOf("a", "b")))
        }
        server.createContext("/echo") { ex ->
            val body = record(ex)
            respond(ex, 200, body, mapOf("X-Echo-Method" to listOf(ex.requestMethod)))
        }
        server.createContext("/status/404") { ex -> record(ex); respond(ex, 404, "no such thing".toByteArray(), emptyMap()) }
        server.createContext("/status/500") { ex -> record(ex); respond(ex, 500, NO_BYTES, emptyMap()) }
        server.createContext("/redirect") { ex -> record(ex); respond(ex, 302, NO_BYTES, mapOf("Location" to listOf("/echo"))) }
        server.createContext("/slow") { ex ->
            record(ex)
            Thread.sleep(3_000)
            respond(ex, 200, "late".toByteArray(), emptyMap())
        }
        server.createContext("/trickle") { ex ->
            // Headers at once, then a byte every 50 ms for three seconds: a slow body, not a slow response.
            record(ex)
            ex.sendResponseHeaders(200, 0)
            try {
                val out = ex.responseBody
                repeat(60) {
                    Thread.sleep(50)
                    out.write(1)
                    out.flush()
                }
                ex.close()
            } catch (e: Exception) {
                // the client hung up
            }
        }
        server.createContext("/head") { ex -> record(ex); respond(ex, 200, NO_BYTES, mapOf("X-Head" to listOf("yes"))) }
        server.start()
    }

    private fun record(ex: HttpExchange): ByteArray {
        val body = ex.requestBody.readBytes()
        seen.add(Seen(ex.requestMethod, ex.requestURI.toString(), ex.requestHeaders.entries.associate { it.key to it.value.toList() }, body))
        return body
    }

    private fun respond(ex: HttpExchange, status: Int, body: ByteArray, headers: Map<String, List<String>>) {
        for ((name, values) in headers) for (v in values) ex.responseHeaders.add(name, v)
        if (ex.requestMethod == "HEAD") {
            ex.sendResponseHeaders(status, -1)
        } else {
            ex.sendResponseHeaders(status, if (body.isEmpty()) -1 else body.size.toLong())
            if (body.isNotEmpty()) ex.responseBody.use { it.write(body) }
        }
        ex.close()
    }

    override fun close() = server.stop(0)
}

private fun request(http: HttpAdapter, req: HttpRequest): HttpResponse = runBlocking { http.request(req) }

class HttpAdapterTests : Suite() {
    init {
        case("a GET returns the status, headers and body") {
            TestHttpServer().use { server ->
                val response = request(HttpAdapter(), HttpRequest(HttpMethod.GET, server.base + "/"))
                assertEq(200.toUShort(), response.status)
                assertEq("hello", String(response.body))
                val byName = response.headers.groupBy({ it.name.lowercase() }, { it.value })
                assertEq(listOf("text/plain"), byName["content-type"])
                assertEq(listOf("a", "b"), byName["x-multi"], "a repeated header comes back as one entry per value")
                assertEq(response.headers.map { it.name.lowercase() }.sorted(), response.headers.map { it.name.lowercase() }, "headers are sorted by name")
                assertEq("GET", server.seen.single().method)
            }
        }

        case("a POST sends its body and headers") {
            TestHttpServer().use { server ->
                val body = ByteArray(10_000) { (it % 200).toByte() }
                val response = request(
                    HttpAdapter(),
                    HttpRequest(HttpMethod.POST, server.base + "/echo?x=1", listOf(Header("X-Token", "abc"), Header("Content-Type", "application/octet-stream")), body),
                )
                assertTrue(response.body.contentEquals(body))
                val seen = server.seen.single()
                assertEq("POST", seen.method)
                assertEq("/echo?x=1", seen.path)
                assertEq(listOf("abc"), seen.headers["X-token"])
                assertEq(listOf("application/octet-stream"), seen.headers["Content-type"])
                assertTrue(seen.body.contentEquals(body))
            }
        }

        case("every method is sent as itself, and HEAD has no body") {
            TestHttpServer().use { server ->
                val http = HttpAdapter()
                for (m in listOf(HttpMethod.PUT, HttpMethod.DELETE, HttpMethod.PATCH, HttpMethod.OPTIONS)) {
                    val r = request(http, HttpRequest(m, server.base + "/echo", body = byteArrayOf(1)))
                    assertEq(m.name, r.headers.first { it.name.equals("X-Echo-Method", true) }.value)
                }
                val head = request(http, HttpRequest(HttpMethod.HEAD, server.base + "/head"))
                assertEq(0, head.body.size)
                assertEq("yes", head.headers.first { it.name.equals("X-Head", true) }.value)
            }
        }

        case("an empty body and no body are both allowed") {
            TestHttpServer().use { server ->
                val http = HttpAdapter()
                request(http, HttpRequest(HttpMethod.POST, server.base + "/echo", body = NO_BYTES))
                request(http, HttpRequest(HttpMethod.POST, server.base + "/echo", body = null))
                assertEq(2, server.seen.size)
                assertTrue(server.seen.all { it.body.isEmpty() })
            }
        }

        case("error statuses are successes: the caller sees the status and body") {
            TestHttpServer().use { server ->
                val http = HttpAdapter()
                val notFound = request(http, HttpRequest(HttpMethod.GET, server.base + "/status/404"))
                assertEq(404.toUShort(), notFound.status)
                assertEq("no such thing", String(notFound.body))
                assertEq(500.toUShort(), request(http, HttpRequest(HttpMethod.GET, server.base + "/status/500")).status)
            }
        }

        case("redirects are followed") {
            TestHttpServer().use { server ->
                val r = request(HttpAdapter(), HttpRequest(HttpMethod.GET, server.base + "/redirect"))
                assertEq(200.toUShort(), r.status)
                assertEq(listOf("/redirect", "/echo"), server.seen.map { it.path })
            }
        }

        case("headers the JDK manages itself are dropped instead of failing the request") {
            TestHttpServer().use { server ->
                val response = request(
                    HttpAdapter(),
                    HttpRequest(HttpMethod.POST, server.base + "/echo", listOf(Header("Host", "evil.example"), Header("Content-Length", "1"), Header("Connection", "close"), Header("X-Ok", "1")), byteArrayOf(1, 2)),
                )
                assertEq(200.toUShort(), response.status)
                val seen = server.seen.single()
                assertEq(listOf("1"), seen.headers["X-ok"])
                assertTrue(seen.headers["Host"]!!.single().startsWith("127.0.0.1"), "the real host was sent")
                assertEq(2, seen.body.size)
            }
        }

        case("a request that takes longer than its timeout fails with Timeout") {
            TestHttpServer().use { server ->
                val started = System.nanoTime()
                assertEq(HttpError.Timeout, assertThrows<HttpError.Timeout> { request(HttpAdapter(), HttpRequest(HttpMethod.GET, server.base + "/slow", timeoutMs = 200u)) })
                assertTrue(System.nanoTime() - started < 2_500_000_000L, "it did not wait for the slow server")
            }
        }

        case("the timeout also covers a body that arrives too slowly") {
            TestHttpServer().use { server ->
                val started = System.nanoTime()
                assertThrows<HttpError.Timeout> { request(HttpAdapter(), HttpRequest(HttpMethod.GET, server.base + "/trickle", timeoutMs = 300u)) }
                assertTrue(System.nanoTime() - started < 2_500_000_000L)
            }
        }

        case("a refused connection is a Network error") {
            val port = java.net.ServerSocket(0).use { it.localPort }
            val e = assertThrows<HttpError.Network> { request(HttpAdapter(), HttpRequest(HttpMethod.GET, "http://127.0.0.1:$port/", timeoutMs = 2000u)) }
            assertTrue(e.reason.isNotEmpty())
        }

        case("URLs and headers that cannot be used are InvalidUrl") {
            val http = HttpAdapter()
            for (url in listOf("not a url", "ftp://example.com/x", "http://", "/relative", "", "http:///nohost", "mailto:a@b")) {
                assertThrows<HttpError.InvalidUrl>("url '$url'") { request(http, HttpRequest(HttpMethod.GET, url)) }
            }
            assertThrows<HttpError.InvalidUrl>("bad header name") {
                request(http, HttpRequest(HttpMethod.GET, "http://127.0.0.1:1/", listOf(Header("Bad Header", "x"))))
            }
        }

        case("cancelling the coroutine returns at once and stays a cancellation") {
            TestHttpServer().use { server ->
                val http = HttpAdapter()
                val started = System.nanoTime()
                assertThrows<CancellationException> {
                    runBlocking { withTimeout(300) { http.request(HttpRequest(HttpMethod.GET, server.base + "/trickle")) } }
                }
                assertTrue(System.nanoTime() - started < 2_500_000_000L, "cancelling does not wait for the transfer")
            }
        }

        case("a large response body arrives whole") {
            TestHttpServer().use { server ->
                val big = ByteArray(5_000_000) { (it * 13).toByte() }
                val r = request(HttpAdapter(), HttpRequest(HttpMethod.POST, server.base + "/echo", body = big))
                assertTrue(r.body.contentEquals(big))
            }
        }

        case("as a port: the reply body is an encoded HttpResponse and failures are typed port errors") {
            TestHttpServer().use { server ->
                val impl = HttpAdapter().portImpl()
                assertTrue(!impl.sync)
                val method = impl.methods.getValue(StandardPorts.Http.REQUEST)
                val ok = runBlocking { method(HttpRequest.encodeToByteArray(HttpRequest(HttpMethod.GET, server.base + "/"))) }
                val decoded = HttpResponse.decodeAll(ok)
                assertEq(200.toUShort(), decoded.status)
                assertEq("hello", String(decoded.body))
                val failure = assertThrows<UndraPortException> {
                    runBlocking { method(HttpRequest.encodeToByteArray(HttpRequest(HttpMethod.GET, "nope"))) }
                }
                assertTrue(HttpError.decodeAll(failure.body) is HttpError.InvalidUrl)
                assertThrows<WireException> { runBlocking { method(byteArrayOf(1)) } }
            }
        }

        case("concurrent requests through one adapter are independent") {
            TestHttpServer().use { server ->
                val http = HttpAdapter()
                val results = CopyOnWriteArrayList<Int>()
                runBlocking {
                    val jobs = List(40) { i ->
                        launch(Dispatchers.IO) {
                            val r = http.request(HttpRequest(HttpMethod.POST, server.base + "/echo", body = byteArrayOf(i.toByte())))
                            results.add(r.body.single().toInt())
                        }
                    }
                    jobs.forEach { it.join() }
                }
                assertEq((0 until 40).toList(), results.sorted())
                eventually("all requests were seen") { server.seen.size == 40 }
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
