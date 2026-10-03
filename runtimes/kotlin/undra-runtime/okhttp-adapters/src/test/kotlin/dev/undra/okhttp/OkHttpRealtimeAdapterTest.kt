package dev.undra.okhttp

import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.SseError
import dev.undra.runtime.adapters.SsePortAdapter
import dev.undra.runtime.adapters.WebSocketPortAdapter
import dev.undra.runtime.adapters.WsError
import dev.undra.runtime.adapters.WsMessage
import dev.undra.runtime.contracts.RealtimeAdapterContract
import dev.undra.runtime.contracts.SseSubject
import dev.undra.runtime.contracts.WebSocketSubject
import dev.undra.runtime.support.eventually
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.fail
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.Call
import okhttp3.Credentials
import okhttp3.EventListener
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import okhttp3.Protocol
import org.junit.Test

private fun <T> within(millis: Long = 20_000, block: suspend CoroutineScope.() -> T): T = runBlocking { withTimeout(millis) { block() } }

private suspend inline fun <reified E : Throwable> failsWith(crossinline block: suspend () -> Unit): E {
    try {
        block()
    } catch (e: Throwable) {
        if (e is E) return e
        fail("expected ${E::class.java.simpleName} but got $e")
    }
    fail("expected ${E::class.java.simpleName} but nothing was thrown")
}

/** A client as an app has one: its defaults (10 s read timeout, HTTP/2 allowed), a short connect timeout for the cases that connect nowhere. */
private fun appClient(): OkHttpClient = OkHttpClient.Builder().connectTimeout(5, TimeUnit.SECONDS).build()

/** An app's client with an interceptor that tags every request, as an app's tracing or auth interceptor does. */
private fun tracingClient(base: OkHttpClient = appClient()): OkHttpClient =
    base.newBuilder().addInterceptor(Interceptor { it.proceed(it.request().newBuilder().header("X-Traced", "yes").build()) }).build()

/**
 * OkHttp's WebSocket and Sse adapters against the suite every such adapter of the Kotlin runtime meets (`RealtimeAdapterContract`,
 * shared with `:runtime`'s `RealtimeAdapterTests`: ADR-060, ADR-047), which uses `contract-tests/servers/realtime-server.mjs` run
 * with Node (skipped, saying why, without it; failed with `UNDRA_REQUIRE_TOOLCHAINS=1`), then what is about OkHttp: the app's
 * client is in the path, and where OkHttp's reader decodes text itself.
 */
class OkHttpRealtimeAdapterTest : RealtimeAdapterContract(
    webSocket = WebSocketSubject("OkHttpWebSocketAdapter", { OkHttpWebSocketAdapter(appClient()) }, rejectsMalformedText = false),
    sse = listOf(SseSubject("OkHttpSseAdapter", { OkHttpSseAdapter(appClient()) }, canAbortBlockedRead = true)),
) {
    init {
        case("WebSocket: a text message that is not UTF-8 arrives with a replacement character for each bad byte, and the connection stays open") {
            val server = server()
            val port = WebSocketPortAdapter(OkHttpWebSocketAdapter(appClient()))
            within {
                val conn = port.connect("${server.ws}/ws/bad-utf8", emptyList(), emptyList()).conn
                val text = port.receive(conn, 16u).single() as WsMessage.Text
                assertTrue(text.value.isNotEmpty() && text.value.all { it == '�' }, "got ${text.value.map { it.code }}")
                port.send(conn, WsMessage.Text("still here"))
                port.close(conn, 1000u, "")
            }
        }

        case("WebSocket: an interceptor of the app's client sees the upgrade request and what it adds reaches the server") {
            val server = server()
            val port = WebSocketPortAdapter(OkHttpWebSocketAdapter(tracingClient()))
            within {
                val conn = port.connect("${server.ws}/ws/headers", emptyList(), listOf(Header("X-Token", "t"))).conn
                port.close(conn, 1000u, "")
            }
            val seen = server.last("/ws/headers")
            assertEq("yes", seen.headers["x-traced"])
            assertEq("t", seen.headers["x-token"])
        }

        case("WebSocket: OkHttp's rules for an upgrade: application interceptors see it; network interceptors and the event listener do not") {
            val server = server()
            val events = CopyOnWriteArrayList<String>()
            val client = appClient().newBuilder()
                .addInterceptor(Interceptor { it.proceed(it.request().newBuilder().header("X-Application", "yes").build()) })
                .addNetworkInterceptor(Interceptor { it.proceed(it.request().newBuilder().header("X-Network", "yes").build()) })
                .eventListener(
                    object : EventListener() {
                        override fun callStart(call: Call) {
                            events += "callStart ${call.request().url.encodedPath}"
                        }
                    },
                )
                .build()
            val port = WebSocketPortAdapter(OkHttpWebSocketAdapter(client))
            within {
                val conn = port.connect("${server.ws}/ws/headers", emptyList(), emptyList()).conn
                port.close(conn, 1000u, "")
            }
            val seen = server.last("/ws/headers")
            assertEq("yes", seen.headers["x-application"])
            // OkHttp runs a WebSocket's upgrade without the client's network interceptors and on a client whose listener is
            // EventListener.NONE (RealWebSocket.connect): the documented limits, pinned so that an OkHttp that changes them is noticed.
            assertEq(null, seen.headers["x-network"])
            assertEq(emptyList<String>(), events.toList())
            // The same client's listener sees an ordinary call.
            within { OkHttpHttpAdapter(client).request(HttpRequest(HttpMethod.GET, "${server.http}/stats", emptyList(), null, 5_000u)) }
            assertEq(listOf("callStart /stats"), events.toList())
        }

        case("WebSocket: the app's Authenticator answers the upgrade's 401 and the connection opens; without it the upgrade is Refused(401)") {
            val server = server()
            val asked = CopyOnWriteArrayList<Int>()
            val client = appClient().newBuilder()
                .authenticator { _, response ->
                    asked += response.code
                    if (response.request.header("Authorization") != null) null else response.request.newBuilder().header("Authorization", Credentials.basic("undra", "secret")).build()
                }
                .build()
            within {
                val port = WebSocketPortAdapter(OkHttpWebSocketAdapter(client))
                port.close(port.connect("${server.ws}/ws/auth", emptyList(), emptyList()).conn, 1000u, "")
                val refused = failsWith<WsError.Refused> { WebSocketPortAdapter(OkHttpWebSocketAdapter(appClient())).connect("${server.ws}/ws/auth", emptyList(), emptyList()) }
                assertEq(401.toUShort(), refused.status)
            }
            assertEq(listOf(401), asked.toList())
            assertEq(Credentials.basic("undra", "secret"), server.connections().filter { it.path == "/ws/auth" }[1].headers["authorization"])
        }

        case("WebSocket: the app's read timeout and call timeout do not end a quiet connection") {
            val server = server()
            val impatient = OkHttpClient.Builder().readTimeout(200, TimeUnit.MILLISECONDS).callTimeout(500, TimeUnit.MILLISECONDS).build()
            val port = WebSocketPortAdapter(OkHttpWebSocketAdapter(impatient))
            within {
                val conn = port.connect("${server.ws}/ws/stall", emptyList(), emptyList()).conn
                // Several times both timeouts, and nothing happened: the connection is still open both ways.
                delay(1_500)
                assertTrue(!server.last("/ws/stall").clientClosed, "the server saw the client leave")
                port.send(conn, WsMessage.Text("still here"))
                port.close(conn, 1000u, "")
            }
            eventually("the server saw the client's close", timeoutMs = 5_000) { server.last("/ws/stall").closeCode == 1000 }
        }

        case("WebSocket: a closed connection leaves no reader running on the app's dispatcher, whether it was held or idle") {
            val server = server()
            val client = appClient()
            val port = WebSocketPortAdapter(OkHttpWebSocketAdapter(client))
            within {
                // Nobody reads: OkHttp's reader thread is held in the listener until there is room.
                val held = port.connect("${server.ws}/ws/flood?n=2000&size=65536", emptyList(), emptyList()).conn
                eventually("the server is writing") { server.last("/ws/flood").written > 0 }
                port.close(held, 1000u, "")
                port.close(port.connect("${server.ws}/ws/stall", emptyList(), emptyList()).conn, 1000u, "")
            }
            // Each connection's reader runs as a call of the client's dispatcher for as long as the connection lives.
            eventually("the readers are gone") { client.dispatcher.runningCallsCount() == 0 }
            eventually("no connection is in use") { client.connectionPool.connectionCount() == client.connectionPool.idleConnectionCount() }
        }

        case("WebSocket: a client provider is asked at every connect") {
            val server = server()
            var current = appClient()
            val port = WebSocketPortAdapter(OkHttpWebSocketAdapter({ current }))
            within {
                port.close(port.connect("${server.ws}/ws/headers", emptyList(), emptyList()).conn, 1000u, "")
                current = tracingClient()
                port.close(port.connect("${server.ws}/ws/headers", emptyList(), emptyList()).conn, 1000u, "")
            }
            val seen = server.connections().filter { it.path == "/ws/headers" }.takeLast(2)
            assertEq(listOf<String?>(null, "yes"), seen.map { it.headers["x-traced"] })
        }

        case("WebSocket: the ping interval is the adapter's, else the client's, else 30 s; zero is never") {
            val bare = OkHttpClient()
            assertEq(30_000, OkHttpWebSocketAdapter(bare).withPing(bare).pingIntervalMillis)
            val own = OkHttpClient.Builder().pingInterval(7, TimeUnit.SECONDS).build()
            val kept = OkHttpWebSocketAdapter(own).withPing(own)
            assertEq(7_000, kept.pingIntervalMillis)
            assertTrue(kept === own, "a client that already has it is used as it is")
            assertEq(2_000, OkHttpWebSocketAdapter(own, pingIntervalMillis = 2_000).withPing(own).pingIntervalMillis)
            assertEq(0, OkHttpWebSocketAdapter(own, pingIntervalMillis = 0).withPing(own).pingIntervalMillis)
            // What is derived shares the app's connection pool, dispatcher and interceptors.
            val tracing = tracingClient()
            val derived = OkHttpWebSocketAdapter(tracing).withPing(tracing)
            assertTrue(derived.connectionPool === tracing.connectionPool && derived.dispatcher === tracing.dispatcher, "pool and dispatcher are the app's")
            assertEq(tracing.interceptors.size, derived.interceptors.size)
        }

        case("SSE: an interceptor of the app's client sees the request and what it adds reaches the server, with Last-Event-ID") {
            val server = server()
            val port = SsePortAdapter(OkHttpSseAdapter(tracingClient()))
            within {
                val stream = port.open("${server.http}/sse/feed", listOf(Header("X-Token", "t")), "2")
                while (true) {
                    try {
                        port.next(stream, 16u)
                    } catch (e: SseError.Ended) {
                        break
                    }
                }
            }
            val seen = server.last("/sse/feed")
            assertEq("yes", seen.headers["x-traced"])
            assertEq("t", seen.headers["x-token"])
            assertEq("2", seen.headers["last-event-id"])
        }

        case("SSE: a closed stream leaves no connection in use in the app's pool, whether its reader waited for room or for data") {
            val server = server()
            val client = appClient()
            val port = SsePortAdapter(OkHttpSseAdapter(client))
            within {
                // Nobody pulls: the reader fills the binding's buffer and waits for room, holding the body.
                val flood = port.open("${server.http}/sse/flood?n=2000&size=65536", emptyList(), null)
                eventually("the server is writing") { server.last("/sse/flood").written > 0 }
                port.close(flood)
                // The server sends nothing: the reader waits in a read.
                port.close(port.open("${server.http}/sse/hang", emptyList(), null))
            }
            eventually("no connection is in use") { client.connectionPool.connectionCount() == client.connectionPool.idleConnectionCount() }
            eventually("the server saw both leave") { server.last("/sse/flood").clientClosed && server.last("/sse/hang").clientClosed }
        }

        case("SSE: the app's read timeout and call timeout do not end a quiet stream") {
            val server = server()
            val impatient = OkHttpClient.Builder().readTimeout(200, TimeUnit.MILLISECONDS).callTimeout(500, TimeUnit.MILLISECONDS).build()
            val port = SsePortAdapter(OkHttpSseAdapter(impatient))
            within {
                val stream = port.open("${server.http}/sse/hang", emptyList(), null)
                // Several times both timeouts, and nothing happened: no timeout applied to the stream.
                delay(1_500)
                assertTrue(!server.last("/sse/hang").clientClosed, "the server saw the client leave")
                port.close(stream)
            }
            eventually("the server saw the client leave", timeoutMs = 2_000) { server.last("/sse/hang").clientClosed }
        }

        case("SSE: the stream is opened on a client that shares the app's pool, dispatcher and interceptors and differs in what a stream needs") {
            val app = tracingClient(OkHttpClient.Builder().readTimeout(3, TimeUnit.SECONDS).callTimeout(4, TimeUnit.SECONDS).build())
            val streaming = OkHttpSseAdapter(app).streaming(app)
            assertEq(0, streaming.readTimeoutMillis)
            assertEq(0, streaming.callTimeoutMillis)
            assertEq(listOf(Protocol.HTTP_1_1), streaming.protocols)
            assertTrue(streaming.connectionPool === app.connectionPool && streaming.dispatcher === app.dispatcher, "pool and dispatcher are the app's")
            assertEq(app.interceptors.size, streaming.interceptors.size)
            assertEq(app.connectTimeoutMillis, streaming.connectTimeoutMillis)
        }
    }

    @Test
    fun allCases() {
        try {
            assertPassed()
        } finally {
            stopServer()
        }
    }
}
