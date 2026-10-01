package dev.undra.android

import androidx.test.platform.app.InstrumentationRegistry
import dev.undra.runtime.adapters.ClientWebSocketAdapter
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.SseError
import dev.undra.runtime.adapters.SseEvent
import dev.undra.runtime.adapters.SsePortAdapter
import dev.undra.runtime.adapters.UrlConnectionSseAdapter
import dev.undra.runtime.adapters.WebSocketPortAdapter
import dev.undra.runtime.adapters.WsError
import dev.undra.runtime.adapters.WsMessage
import java.net.HttpURLConnection
import java.net.URL
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test

/**
 * The default WebSocket and Sse adapters on the device, against the realtime server of the contract tests
 * (`contract-tests/servers/realtime-server.mjs`) running on the host, which the emulator reaches as 10.0.2.2. Runs when the
 * instrumentation argument `undra.realtimePort` names the server's port:
 *
 * ```
 * node contract-tests/servers/realtime-server.mjs --port 0     # prints READY <port>
 * ./gradlew :android-adapters:connectedAndroidTest -Pandroid.testInstrumentationRunnerArguments.undra.realtimePort=<port>
 * ```
 *
 * The case that matters most on Android: closing a stream whose server sends nothing releases the connection
 * (`HttpURLConnection.disconnect()` aborts the blocked read here, which the JDK's does not).
 */
class RealtimeOnDeviceTest {
    private var port = 0
    private val host = "10.0.2.2"

    @Before
    fun setUp() {
        val given = InstrumentationRegistry.getArguments().getString("undra.realtimePort")
        assumeTrue("no undra.realtimePort instrumentation argument: the host's realtime server is not running", given != null)
        port = given!!.toInt()
    }

    private fun run(block: suspend CoroutineScope.() -> Unit) {
        runBlocking { withTimeout(30_000) { block() } }
    }

    /** The newest connection to [path] in the server's `/stats`, as JSON text. */
    private fun lastConnection(path: String): String {
        val connection = URL("http://$host:$port/stats").openConnection() as HttpURLConnection
        try {
            val stats = connection.inputStream.use { String(it.readBytes()) }
            val entries = stats.split("{\"id\":").drop(1)
            return entries.lastOrNull { it.contains("\"path\":\"$path\"") } ?: fail("no connection to $path in $stats").let { "" }
        } finally {
            connection.disconnect()
        }
    }

    @Test
    fun websocket_echo_headers_subprotocol_and_typed_ends() = run {
        val ws = WebSocketPortAdapter(ClientWebSocketAdapter(connectTimeoutMillis = 5_000))
        val opened = ws.connect("ws://$host:$port/ws/headers", listOf("v2", "v1"), listOf(Header("X-Token", "t")))
        assertEquals("v2", opened.protocol)
        val headers = ws.receive(opened.conn, 1u).single() as WsMessage.Text
        assertTrue(headers.value, headers.value.contains("\"x-token\":\"t\""))
        ws.send(opened.conn, WsMessage.Text("é"))
        ws.send(opened.conn, WsMessage.Binary(byteArrayOf(1, 2, 3)))
        val back = ArrayList<WsMessage>()
        while (back.size < 2) back.addAll(ws.receive(opened.conn, 16u))
        assertEquals(listOf(WsMessage.Text("é"), WsMessage.Binary(byteArrayOf(1, 2, 3))), back)
        ws.close(opened.conn, 4000u, "bye")
        assertTrue(lastConnection("/ws/headers").contains("\"closeCode\":4000"))

        val closing = ws.connect("ws://$host:$port/ws/close?code=4001&reason=kicked", emptyList(), emptyList()).conn
        assertEquals(listOf<WsMessage>(WsMessage.Text("hello")), ws.receive(closing, 16u))
        try {
            ws.receive(closing, 16u)
            fail("expected the peer's close")
        } catch (e: WsError.Closed) {
            assertEquals(WsError.Closed(4001u, "kicked"), e)
        }
        try {
            ws.connect("ws://$host:$port/ws/deny?status=401", emptyList(), emptyList())
            fail("expected a refusal")
        } catch (e: WsError.Refused) {
            assertEquals(401.toUShort(), e.status)
        }
    }

    @Test
    fun sse_feed_resume_refusals_and_closing_a_silent_stream() = run {
        val sse = SsePortAdapter(UrlConnectionSseAdapter())
        val stream = sse.open("http://$host:$port/sse/feed", emptyList(), "2")
        val events = ArrayList<SseEvent>()
        try {
            while (true) events.addAll(sse.next(stream, 16u))
        } catch (e: SseError.Ended) {
            // the body ended
        }
        assertEquals(listOf(SseEvent("2", "message", "three", null), SseEvent("4", "message", "four", null)), events)
        try {
            sse.open("http://$host:$port/sse/status?code=204", emptyList(), null)
            fail("expected a refusal")
        } catch (e: SseError.Refused) {
            assertEquals(204.toUShort(), e.status)
        }
        try {
            sse.open("http://$host:$port/sse/html", emptyList(), null)
            fail("expected a protocol error")
        } catch (e: SseError.Protocol) {
            assertTrue(e.reason, e.reason.contains("text/html"))
        }
        val hang = sse.open("http://$host:$port/sse/hang", emptyList(), null)
        delay(200)
        assertTrue(lastConnection("/sse/hang").contains("\"clientClosed\":false"))
        sse.close(hang)
        val deadline = System.nanoTime() + 2_000_000_000L
        while (!lastConnection("/sse/hang").contains("\"clientClosed\":true")) {
            if (System.nanoTime() > deadline) fail("the server did not see the client leave /sse/hang")
            delay(20)
        }
    }
}
