package dev.undra.runtime

import dev.undra.runtime.adapters.ClientWebSocketAdapter
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.JdkHttpSseAdapter
import dev.undra.runtime.adapters.SseAdapter
import dev.undra.runtime.adapters.SseError
import dev.undra.runtime.adapters.SseEvent
import dev.undra.runtime.adapters.SsePortAdapter
import dev.undra.runtime.adapters.UrlConnectionSseAdapter
import dev.undra.runtime.adapters.WebSocketPortAdapter
import dev.undra.runtime.adapters.WsError
import dev.undra.runtime.adapters.WsMessage
import dev.undra.runtime.support.MiniJson
import dev.undra.runtime.support.RealtimeServer
import dev.undra.runtime.support.eventually
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.fail
import java.net.ServerSocket
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.jupiter.api.Test

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

/** Receives exactly [count] messages from [conn], pulling 16 at a time as the core does. */
private suspend fun WebSocketPortAdapter.receiveExactly(conn: UInt, count: Int): List<WsMessage> {
    val out = ArrayList<WsMessage>(count)
    while (out.size < count) out.addAll(receive(conn, minOf(16, count - out.size).toUInt()))
    return out
}

/** Every event of [stream] until its end, which is returned too. */
private suspend fun SsePortAdapter.drain(stream: UInt): Pair<List<SseEvent>, SseError> {
    val out = ArrayList<SseEvent>()
    while (true) {
        try {
            out.addAll(next(stream, 16u))
        } catch (e: SseError) {
            return out to e
        }
    }
}

private val FEED = listOf(
    SseEvent("1", "message", "one", 1500u),
    SseEvent("2", "tick", "two\nlines", null),
    SseEvent("2", "message", "three", null),
    SseEvent("4", "message", "four", null),
)

/**
 * The shared failure-injection suite of the default WebSocket and Sse adapters (ADR-047, brief section 5), through their
 * bindings, against `contract-tests/servers/realtime-server.mjs` run with Node. Skipped, saying why, without Node.
 */
class RealtimeAdapterTests : Suite() {
    private var started: Result<RealtimeServer>? = null

    /** The server, started once for the suite. */
    private fun server(): RealtimeServer {
        val outcome = started ?: RealtimeServer.start().also { started = it }
        return outcome.getOrElse {
            if (System.getenv("UNDRA_REQUIRE_TOOLCHAINS") == "1") fail("UNDRA_REQUIRE_TOOLCHAINS=1: the realtime server cannot run here: ${it.message}")
            skip("the realtime server cannot run here: ${it.message}")
        }
    }

    private fun ws(): WebSocketPortAdapter = WebSocketPortAdapter(ClientWebSocketAdapter(connectTimeoutMillis = 5_000))

    private fun sseCases(name: String, adapter: () -> SseAdapter, canAbortBlockedRead: Boolean) {
        case("SSE ($name): the feed parses as the HTML standard says, then the body's end is Ended; no Last-Event-ID was sent") {
            val server = server()
            val port = SsePortAdapter(adapter())
            within {
                val stream = port.open("${server.http}/sse/feed", listOf(Header("X-Token", "t")), null)
                val (events, end) = port.drain(stream)
                assertEq(FEED, events)
                assertEq(SseError.Ended, end)
                port.close(stream)
            }
            val seen = server.last("/sse/feed")
            assertEq(null, seen.headers["last-event-id"])
            assertEq("text/event-stream", seen.headers["accept"])
            assertEq("no-cache", seen.headers["cache-control"])
            assertEq("t", seen.headers["x-token"])
        }

        case("SSE ($name): resuming sends Last-Event-ID and gets what came after") {
            val server = server()
            val port = SsePortAdapter(adapter())
            within {
                val stream = port.open("${server.http}/sse/feed", emptyList(), "2")
                val (events, end) = port.drain(stream)
                assertEq(FEED.drop(2), events)
                assertEq(SseError.Ended, end)
            }
            assertEq("2", server.last("/sse/feed").headers["last-event-id"])
        }

        case("SSE ($name): a 204 or a 500 is Refused with its status; text/html is Protocol") {
            val server = server()
            val port = SsePortAdapter(adapter())
            within {
                for (code in listOf(204, 500, 404)) {
                    val refused = failsWith<SseError.Refused> { port.open("${server.http}/sse/status?code=$code", emptyList(), null) }
                    assertEq(code.toUShort(), refused.status, "status $code")
                }
                val html = failsWith<SseError.Protocol> { port.open("${server.http}/sse/html", emptyList(), null) }
                assertEq("expected text/event-stream, got text/html", html.reason)
            }
        }

        case("SSE ($name): a server that cannot be reached is Network; a bad URL is Refused without a status") {
            val closedPort = ServerSocket(0).use { it.localPort }
            val port = SsePortAdapter(adapter())
            within {
                failsWith<SseError.Network> { port.open("http://127.0.0.1:$closedPort/sse/feed", emptyList(), null) }
                val bad = failsWith<SseError.Refused> { port.open("http://exa mple.test/", emptyList(), null) }
                assertEq(null, bad.status)
                val header = failsWith<SseError.Refused> { port.open("http://127.0.0.1:$closedPort/", listOf(Header("X", "a\r\nb")), null) }
                assertEq(null, header.status)
            }
        }

        if (canAbortBlockedRead) {
            case("SSE ($name): closing a stream whose server sends nothing releases the connection: the server sees the client leave") {
                val server = server()
                val port = SsePortAdapter(adapter())
                within {
                    val stream = port.open("${server.http}/sse/hang", emptyList(), null)
                    kotlinx.coroutines.delay(100)
                    assertTrue(!server.last("/sse/hang").clientClosed)
                    port.close(stream)
                }
                eventually("the server saw the client leave", timeoutMs = 1_000) { server.last("/sse/hang").clientClosed }
            }

            case("SSE ($name): a stalled reader stalls the server; then everything arrives in order") {
                val server = server()
                val port = SsePortAdapter(adapter())
                val n = 2000
                within(60_000) {
                    val stream = port.open("${server.http}/sse/flood?n=$n&size=65536", emptyList(), null)
                    kotlinx.coroutines.delay(1_000)
                    val written = server.last("/sse/flood").written
                    assertTrue(written < n / 4, "the server could write only what fits in the buffers while nobody read: $written of $n")
                    val (events, end) = port.drain(stream)
                    assertEq(n, events.size)
                    assertEq((0 until n).map { "$it" }, events.map { it.id })
                    assertTrue(events.all { it.data.length == 65536 })
                    assertEq(SseError.Ended, end)
                }
            }
        }
    }

    init {
        // ---- WebSocket: ClientWebSocketAdapter -------------------------------------------------------------------------

        case("WebSocket: text and binary echo in order; the core's close reaches the server with its code and reason") {
            val server = server()
            val port = ws()
            within {
                val opened = port.connect("${server.ws}/ws/echo", emptyList(), emptyList())
                assertEq("", opened.protocol)
                val sent = listOf(WsMessage.Text("a"), WsMessage.Binary(byteArrayOf(1, 2, 3)), WsMessage.Text("é"), WsMessage.Text(""), WsMessage.Binary(ByteArray(200_000) { it.toByte() }))
                for (m in sent) port.send(opened.conn, m)
                assertEq(sent, port.receiveExactly(opened.conn, sent.size))
                port.close(opened.conn, 1000u, "done")
            }
            val seen = server.last("/ws/echo")
            assertEq(1000, seen.closeCode)
            assertEq("done", seen.closeReason)
        }

        case("WebSocket: subprotocols are offered and the server's choice answered; headers go with the upgrade") {
            val server = server()
            val port = ws()
            within {
                val opened = port.connect("${server.ws}/ws/headers", listOf("v2", "v1"), listOf(Header("X-Token", "t"), Header("Authorization", "Bearer x")))
                assertEq("v2", opened.protocol)
                val first = port.receiveExactly(opened.conn, 1).single() as WsMessage.Text
                @Suppress("UNCHECKED_CAST")
                val headers = MiniJson.parse(first.value) as Map<String, String>
                assertEq("t", headers["x-token"])
                assertEq("Bearer x", headers["authorization"])
                assertEq("v2, v1", headers["sec-websocket-protocol"])
                port.send(opened.conn, WsMessage.Text("ping"))
                assertEq(listOf<WsMessage>(WsMessage.Text("ping")), port.receiveExactly(opened.conn, 1))
                port.close(opened.conn, 4000u, "bye")
            }
            val seen = server.last("/ws/headers")
            assertEq(listOf("v2", "v1"), seen.protocols)
            assertEq(4000, seen.closeCode)
            assertEq("bye", seen.closeReason)
        }

        case("WebSocket: a refused upgrade is Refused with its HTTP status") {
            val server = server()
            val port = ws()
            within {
                for (status in listOf(401, 403, 404)) {
                    val refused = failsWith<WsError.Refused> { port.connect("${server.ws}/ws/deny?status=$status", emptyList(), emptyList()) }
                    assertEq(status.toUShort(), refused.status)
                    assertTrue(refused.reason.contains("$status"), refused.reason)
                }
            }
        }

        case("WebSocket: the peer's close frame is Closed with its code and reason, after the messages before it") {
            val server = server()
            val port = ws()
            within {
                val conn = port.connect("${server.ws}/ws/close?code=4001&reason=kicked", emptyList(), emptyList()).conn
                assertEq(listOf<WsMessage>(WsMessage.Text("hello")), port.receiveExactly(conn, 1))
                assertEq(WsError.Closed(4001u, "kicked"), failsWith<WsError.Closed> { port.receive(conn, 16u) })
                assertEq(WsError.Closed(4001u, "kicked"), failsWith<WsError.Closed> { port.send(conn, WsMessage.Text("late")) })
                port.close(conn, 1000u, "")
            }
        }

        case("WebSocket: a connection dropped without a close frame is Network") {
            val server = server()
            val port = ws()
            within {
                val conn = port.connect("${server.ws}/ws/drop", emptyList(), emptyList()).conn
                assertEq(listOf<WsMessage>(WsMessage.Text("hello")), port.receiveExactly(conn, 1))
                failsWith<WsError.Network> { port.receive(conn, 16u) }
                port.close(conn, 1000u, "")
            }
        }

        case("WebSocket: a text message that is not UTF-8 is Protocol, and the client closes with 1007") {
            val server = server()
            val port = ws()
            within {
                val conn = port.connect("${server.ws}/ws/bad-utf8", emptyList(), emptyList()).conn
                val error = failsWith<WsError.Protocol> { port.receive(conn, 16u) }
                assertTrue(error.reason.contains("UTF-8"), error.reason)
                port.close(conn, 1000u, "")
            }
            eventually("the server saw 1007") { server.last("/ws/bad-utf8").closeCode == 1007 }
        }

        case("WebSocket: a stalled reader stalls the server (TCP pushes back); then everything arrives in order") {
            val server = server()
            val port = ws()
            val n = 2000
            within(60_000) {
                val conn = port.connect("${server.ws}/ws/flood?n=$n&size=65536", emptyList(), emptyList()).conn
                kotlinx.coroutines.delay(1_000)
                val written = server.last("/ws/flood").written
                assertTrue(written < n / 4, "the server could write only what fits in the buffers while nobody read: $written of $n")
                val all = port.receiveExactly(conn, n)
                assertEq((0 until n).map { "$it" }, all.map { (it as WsMessage.Text).value.trimEnd('.') })
                assertTrue(all.all { (it as WsMessage.Text).value.length == 65536 })
                assertEq(WsError.Closed(1000u, "end"), failsWith<WsError.Closed> { port.receive(conn, 16u) })
            }
        }

        case("WebSocket: a connection the core leaves open is closed going away (1001) when the core detaches") {
            val server = server()
            val port = ws()
            within { port.connect("${server.ws}/ws/stall", emptyList(), emptyList()) }
            port.portImpl().detach!!.invoke()
            eventually("the server saw 1001", timeoutMs = 2_000) { server.last("/ws/stall").closeCode == 1001 }
        }

        case("WebSocket: an unreachable server is Network; headers and subprotocols the client cannot send are Refused") {
            val server = server()
            val closedPort = ServerSocket(0).use { it.localPort }
            val port = ws()
            within {
                failsWith<WsError.Network> { port.connect("ws://127.0.0.1:$closedPort/", emptyList(), emptyList()) }
                assertEq(null, failsWith<WsError.Refused> { port.connect("${server.ws}/ws/echo", emptyList(), listOf(Header("Host", "evil"))) }.status)
                assertEq(null, failsWith<WsError.Refused> { port.connect("${server.ws}/ws/echo", emptyList(), listOf(Header("X", "a\r\nInjected: 1"))) }.status)
                assertEq(null, failsWith<WsError.Refused> { port.connect("${server.ws}/ws/echo", listOf("bad protocol"), emptyList()) }.status)
                assertEq(null, failsWith<WsError.Refused> { port.connect("ws://", emptyList(), emptyList()) }.status)
            }
        }

        // ---- Sse --------------------------------------------------------------------------------------------------------

        sseCases("java.net.http", ::JdkHttpSseAdapter, canAbortBlockedRead = true)
        // The JDK's HttpURLConnection cannot abort a blocked read (Android's can): those two cases run on the device.
        sseCases("HttpURLConnection", { UrlConnectionSseAdapter() }, canAbortBlockedRead = false)
    }

    @Test
    fun allCases() {
        try {
            assertPassed()
        } finally {
            started?.getOrNull()?.close()
        }
    }

    /** Stops the server after a local run (TestMain). */
    fun stopServer() {
        started?.getOrNull()?.close()
        started = null
    }
}
