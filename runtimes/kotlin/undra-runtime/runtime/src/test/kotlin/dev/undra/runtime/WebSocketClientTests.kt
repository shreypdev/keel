package dev.undra.runtime

import dev.undra.runtime.support.WsTestServer
import dev.undra.runtime.support.eventually
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import java.io.IOException
import java.net.URI
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.Test

/** Everything the client told its listener, in order. */
private class Heard : WebSocketClient.Listener {
    val messages = LinkedBlockingQueue<ByteArray>()
    val texts = CopyOnWriteArrayList<Unit>()
    val closes = CopyOnWriteArrayList<Pair<Int, String>>()
    val errors = CopyOnWriteArrayList<Throwable>()

    override fun onBinary(message: ByteArray) {
        messages.add(message)
    }

    override fun onText() {
        texts.add(Unit)
    }

    override fun onClose(code: Int, reason: String) {
        closes.add(code to reason)
    }

    override fun onError(cause: Throwable) {
        errors.add(cause)
    }
}

private fun connect(server: WsTestServer, heard: Heard, path: String = "", pingAfterMillis: Long = 10_000, maxMessageBytes: Int = 1 shl 20): WebSocketClient =
    WebSocketClient.connect(URI(server.url + path), heard, 3_000, maxMessageBytes, pingAfterMillis)

/** The RFC 6455 client that Android runs: tested over a real socket against a server that follows the RFC strictly. */
class WebSocketClientTests : Suite() {
    init {
        case("the upgrade request carries the path and query, and messages go both ways, masked") {
            WsTestServer().use { server ->
                val heard = Heard()
                val ws = connect(server, heard, "/dev?undra_session=abc&undra_resume=1")
                val conn = server.awaitConnection()
                assertEq("GET /dev?undra_session=abc&undra_resume=1 HTTP/1.1", conn.requestLine)
                assertEq(mapOf("undra_session" to "abc", "undra_resume" to "1"), conn.query)
                ws.sendBinary(byteArrayOf(1, 2, 3))
                eventually("the server got it") { conn.messages.isNotEmpty() }
                assertEq(listOf<Byte>(1, 2, 3), conn.messages.poll().toList())
                conn.sendBinary(byteArrayOf(9, 8))
                assertEq(listOf<Byte>(9, 8), heard.messages.poll(5, TimeUnit.SECONDS)!!.toList())
                ws.abort()
            }
        }

        case("a request without a path asks for /") {
            WsTestServer().use { server ->
                val ws = connect(server, Heard())
                assertEq("GET / HTTP/1.1", server.awaitConnection().requestLine)
                ws.abort()
            }
        }

        case("a ping from the server is answered with a pong carrying the same bytes") {
            WsTestServer().use { server ->
                val ws = connect(server, Heard())
                val conn = server.awaitConnection()
                conn.sendPing(byteArrayOf(4, 5, 6))
                assertEq(listOf<Byte>(4, 5, 6), conn.pongs.poll(5, TimeUnit.SECONDS)!!.toList())
                ws.abort()
            }
        }

        case("a close from the server is reported once, with its code and reason, and echoed") {
            WsTestServer().use { server ->
                val heard = Heard()
                val ws = connect(server, heard)
                val conn = server.awaitConnection()
                conn.sendClose(4001, "session lost")
                eventually("the close is reported") { heard.closes.isNotEmpty() }
                assertEq(listOf(4001 to "session lost"), heard.closes.toList())
                eventually("the server sees the echo") { conn.closeCodes.isNotEmpty() }
                assertEq(4001, conn.closeCodes.first())
                eventually("the client is closed") { !ws.isOpen }
                assertTrue(heard.errors.isEmpty(), "a close is not an error: ${heard.errors}")
            }
        }

        case("closing from the client sends 1000, waits for the echo and reports nothing") {
            WsTestServer().use { server ->
                val heard = Heard()
                val ws = connect(server, heard)
                val conn = server.awaitConnection()
                ws.close()
                eventually("the server sees the close") { conn.closeCodes.isNotEmpty() }
                assertEq(1000, conn.closeCodes.first())
                assertTrue(!ws.isOpen)
                Thread.sleep(150)
                assertTrue(heard.closes.isEmpty() && heard.errors.isEmpty(), "closed by us: nothing to report (${heard.closes}, ${heard.errors})")
                assertThrows<IOException> { ws.sendBinary(byteArrayOf(1)) }
            }
        }

        case("a connection the server drops is an error, once") {
            WsTestServer().use { server ->
                val heard = Heard()
                connect(server, heard)
                server.awaitConnection().drop()
                eventually("the error is reported") { heard.errors.isNotEmpty() }
                Thread.sleep(100)
                assertEq(1, heard.errors.size)
            }
        }

        case("a text message is reported as one, not as a message") {
            WsTestServer().use { server ->
                val heard = Heard()
                val ws = connect(server, heard)
                server.awaitConnection().sendText("hello")
                eventually("the text is reported") { heard.texts.isNotEmpty() }
                assertTrue(heard.messages.isEmpty())
                ws.abort()
            }
        }

        case("a message over the limit is a protocol error") {
            WsTestServer().use { server ->
                val heard = Heard()
                connect(server, heard, maxMessageBytes = 1_000)
                server.awaitConnection().sendBinary(ByteArray(2_000))
                eventually("the error is reported") { heard.errors.isNotEmpty() }
                assertTrue(heard.errors.single().message!!.contains("protocol error"), heard.errors.single().message!!)
            }
        }

        case("the upgrade is checked: a status other than 101, or a wrong Sec-WebSocket-Accept, is refused") {
            WsTestServer().use { server ->
                server.rejectWith = "HTTP/1.1 403 Forbidden"
                val e = assertThrows<IOException> { connect(server, Heard()) }
                assertTrue(e.message!!.contains("403"), e.message!!)
            }
            WsTestServer().use { server ->
                server.corruptAccept = true
                val e = assertThrows<IOException> { connect(server, Heard()) }
                assertTrue(e.message!!.contains("Sec-WebSocket-Accept"), e.message!!)
            }
        }

        case("connecting to a port nobody listens on fails fast with an IOException") {
            val port = java.net.ServerSocket(0).use { it.localPort }
            assertThrows<IOException> { WebSocketClient.connect(URI("ws://127.0.0.1:$port"), Heard(), 2_000) }
        }

        case("a quiet server is pinged, and one that answers stays connected") {
            WsTestServer().use { server ->
                val heard = Heard()
                val ws = connect(server, heard, pingAfterMillis = 200)
                val conn = server.awaitConnection()
                eventually("the client pings a quiet server") { conn.pingsReceived.get() >= 1 }
                Thread.sleep(500)
                assertTrue(heard.errors.isEmpty(), "pongs keep it alive: ${heard.errors}")
                assertTrue(ws.isOpen)
                ws.abort()
            }
        }

        case("a server that stops answering is given up on after about two ping intervals") {
            WsTestServer().use { server ->
                val heard = Heard()
                connect(server, heard, pingAfterMillis = 200)
                server.awaitConnection().answerPings = false // the peer is gone, its socket is not
                val started = System.nanoTime()
                eventually("the silence is reported", timeoutMs = 5_000) { heard.errors.isNotEmpty() }
                val tookMillis = (System.nanoTime() - started) / 1_000_000
                assertTrue(heard.errors.single().message!!.contains("stopped answering"), heard.errors.single().message!!)
                assertTrue(tookMillis in 300..3_000, "reported after $tookMillis ms")
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
