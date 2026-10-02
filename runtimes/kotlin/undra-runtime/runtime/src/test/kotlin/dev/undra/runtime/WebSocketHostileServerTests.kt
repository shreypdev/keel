package dev.undra.runtime

import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.support.eventually
import java.io.DataInputStream
import java.io.File
import java.io.FileInputStream
import java.io.IOException
import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.URI
import java.nio.file.Files
import java.security.KeyStore
import java.security.MessageDigest
import java.util.Base64
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLServerSocket
import javax.net.ssl.TrustManagerFactory
import org.junit.jupiter.api.Test

private const val HOSTILE_WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

private fun hostileAcceptFor(key: String): String =
    Base64.getEncoder().encodeToString(MessageDigest.getInstance("SHA-1").digest((key + HOSTILE_WS_GUID).toByteArray()))

/** A correct upgrade response for [key]. */
private fun upgradeResponse(key: String, extra: String = ""): String =
    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${hostileAcceptFor(key)}\r\n$extra\r\n"

/** One frame as the server read it, unmasked. */
private class SeenFrame(val fin: Boolean, val opcode: Int, val mask: ByteArray?, val payload: ByteArray) {
    val closeCode: Int get() = if (payload.size >= 2) ((payload[0].toInt() and 0xFF) shl 8) or (payload[1].toInt() and 0xFF) else 1005
}

/** The server end of one connection: raw bytes out, frames in. */
private class HostilePeer(val socket: Socket, val head: String = "") {
    val input = DataInputStream(socket.getInputStream())
    private val out: OutputStream = socket.getOutputStream()

    fun write(vararg bytes: Int) = write(ByteArray(bytes.size) { bytes[it].toByte() })

    fun write(bytes: ByteArray) {
        out.write(bytes)
        out.flush()
    }

    /** Reads one client frame, or `null` at the end of the stream. */
    fun readFrame(): SeenFrame? {
        val b0 = input.read()
        if (b0 < 0) return null
        val b1 = input.readUnsignedByte()
        var length = (b1 and 0x7F).toLong()
        if (length == 126L) length = input.readUnsignedShort().toLong() else if (length == 127L) length = input.readLong()
        val mask = if (b1 and 0x80 != 0) ByteArray(4).also { input.readFully(it) } else null
        val payload = ByteArray(length.toInt()).also { input.readFully(it) }
        if (mask != null) for (i in payload.indices) payload[i] = (payload[i].toInt() xor mask[i and 3].toInt()).toByte()
        return SeenFrame(b0 and 0x80 != 0, b0 and 0x0F, mask, payload)
    }

    /** Reads frames until a close arrives (or the stream ends). */
    fun awaitClose(): SeenFrame? {
        socket.soTimeout = 5_000
        while (true) {
            val frame = try {
                readFrame()
            } catch (e: IOException) {
                return null
            } ?: return null
            if (frame.opcode == 8) return frame
        }
    }
}

/**
 * A server that misbehaves on request: it reads the upgrade request, answers it with [respond] (given the client's
 * `Sec-WebSocket-Key`), and then hands the raw socket to the test.
 */
private class HostileServer(
    private val respond: (String) -> String = { upgradeResponse(it) },
    private val server: ServerSocket = ServerSocket(0, 10, InetAddress.getLoopbackAddress()),
) : AutoCloseable {
    val port: Int get() = server.localPort
    private val peers = LinkedBlockingQueue<HostilePeer>()

    init {
        Thread {
            while (true) {
                val socket = try {
                    server.accept()
                } catch (e: IOException) {
                    break
                }
                Thread {
                    try {
                        val input = socket.getInputStream()
                        val head = StringBuilder()
                        while (!head.endsWith("\r\n\r\n")) {
                            val b = input.read()
                            if (b < 0) return@Thread
                            head.append(b.toChar())
                        }
                        val key = head.lines().first { it.startsWith("Sec-WebSocket-Key:", ignoreCase = true) }.substringAfter(':').trim()
                        val peer = HostilePeer(socket, head.toString())
                        peers.add(peer)
                        peer.write(respond(key).toByteArray())
                    } catch (e: IOException) {
                        // the client went away
                    }
                }.also { it.isDaemon = true }.start()
            }
        }.also { it.isDaemon = true }.start()
    }

    fun url(host: String = "127.0.0.1", scheme: String = "ws"): URI = URI("$scheme://$host:$port/")

    fun awaitPeer(): HostilePeer = peers.poll(5, TimeUnit.SECONDS) ?: throw AssertionError("no client connected")

    override fun close() = server.close()
}

private class WsRecorder(private val throwOnBinary: Boolean = false) : WebSocketClient.Listener {
    val messages = LinkedBlockingQueue<ByteArray>()
    val texts = LinkedBlockingQueue<String>()
    val closes = CopyOnWriteArrayList<Pair<Int, String>>()
    val errors = CopyOnWriteArrayList<Throwable>()

    override fun onBinary(message: ByteArray) {
        if (throwOnBinary) throw IllegalStateException("the owner broke")
        messages.add(message)
    }

    override fun onText() = Unit

    override fun onText(message: String) {
        texts.add(message)
    }

    override fun onClose(code: Int, reason: String) {
        closes.add(code to reason)
    }

    override fun onError(cause: Throwable) {
        errors.add(cause)
    }

    fun awaitError(): Throwable {
        eventually("the client reports an error") { errors.isNotEmpty() }
        return errors.single()
    }
}

private fun openHostile(server: HostileServer, recorder: WsRecorder, maxMessageBytes: Int = 1 shl 20, timeoutMillis: Int = 3_000): WebSocketClient =
    WebSocketClient.connect(server.url(), recorder, timeoutMillis, maxMessageBytes)

/** A read gate the test opens: the reader waits at it until [open]. */
private class LatchGate : WebSocketClient.ReadGate {
    private val latch = java.util.concurrent.CountDownLatch(1)

    override fun awaitOpen() = latch.await()

    fun open() = latch.countDown()
}

/** An unmasked server frame. */
private fun serverFrame(opcode: Int, payload: ByteArray, fin: Boolean = true): ByteArray {
    val head = if (payload.size < 126) {
        byteArrayOf(((if (fin) 0x80 else 0) or opcode).toByte(), payload.size.toByte())
    } else {
        byteArrayOf(((if (fin) 0x80 else 0) or opcode).toByte(), 126, (payload.size shr 8).toByte(), payload.size.toByte())
    }
    return head + payload
}

/** A self-signed certificate for `localhost` (no IP address in it), made with the JDK's keytool; `null` without one. */
private object HostileTlsCertificate {
    const val PASSWORD = "undra-test"

    val keyStore: KeyStore? by lazy {
        val keytool = File(System.getProperty("java.home"), "bin/keytool")
        if (!keytool.canExecute()) return@lazy null
        val dir = Files.createTempDirectory("undra-wss").toFile().also { it.deleteOnExit() }
        val file = File(dir, "localhost.p12")
        val process = ProcessBuilder(
            keytool.path, "-genkeypair", "-alias", "localhost", "-keyalg", "EC", "-groupname", "secp256r1",
            "-dname", "CN=localhost", "-ext", "san=dns:localhost", "-validity", "2",
            "-storetype", "PKCS12", "-keystore", file.path, "-storepass", PASSWORD, "-keypass", PASSWORD,
        ).redirectErrorStream(true).start()
        process.inputStream.readBytes()
        if (!process.waitFor(60, TimeUnit.SECONDS) || process.exitValue() != 0) return@lazy null
        KeyStore.getInstance("PKCS12").also { ks -> FileInputStream(file).use { ks.load(it, PASSWORD.toCharArray()) } }
    }

    /** A TLS context that serves the certificate and trusts it (and nothing else). */
    fun context(): SSLContext? {
        val ks = keyStore ?: return null
        val keys = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm()).also { it.init(ks, PASSWORD.toCharArray()) }
        val trust = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).also { it.init(ks) }
        return SSLContext.getInstance("TLS").also { it.init(keys.keyManagers, trust.trustManagers, null) }
    }
}

/**
 * The Kotlin WebSocket client against a server that breaks RFC 6455 on purpose: the upgrade, the framing, the
 * close, the size cap and TLS. Each case states what the client must do; none may hang, allocate what the server
 * claims, or end without telling its owner.
 */
class WebSocketHostileServerTests : Suite() {
    init {
        case("an upgrade response with a wrong or missing Sec-WebSocket-Accept is refused") {
            HostileServer({ upgradeResponse("not the client's key") }).use { server ->
                val e = assertThrows<IOException> { openHostile(server, WsRecorder()) }
                assertTrue(e.message!!.contains("Sec-WebSocket-Accept"), e.message!!)
            }
            HostileServer({ "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n" }).use { server ->
                val e = assertThrows<IOException> { openHostile(server, WsRecorder()) }
                assertTrue(e.message!!.contains("Sec-WebSocket-Accept"), e.message!!)
            }
        }

        case("an upgrade response without Upgrade or Connection, or with an extension nobody asked for, is refused") {
            HostileServer({ "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${hostileAcceptFor(it)}\r\n\r\n" }).use { server ->
                assertTrue(assertThrows<IOException> { openHostile(server, WsRecorder()) }.message!!.contains("Upgrade"))
            }
            HostileServer({ "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: ${hostileAcceptFor(it)}\r\n\r\n" }).use { server ->
                assertTrue(assertThrows<IOException> { openHostile(server, WsRecorder()) }.message!!.contains("Connection"))
            }
            HostileServer({ upgradeResponse(it, "Sec-WebSocket-Extensions: permessage-deflate\r\n") }).use { server ->
                assertTrue(assertThrows<IOException> { openHostile(server, WsRecorder()) }.message!!.contains("extension"))
            }
        }

        case("a server that trickles its upgrade response is given up on at the deadline") {
            HostileServer({ "" }).use { server ->
                Thread {
                    val peer = server.awaitPeer()
                    try {
                        for (c in "HTTP/1.1 101 Switching Protocols\r\n") {
                            peer.write(c.code)
                            Thread.sleep(150)
                        }
                    } catch (e: IOException) {
                        // the client gave up, as it should
                    }
                }.also { it.isDaemon = true }.start()
                val started = System.nanoTime()
                assertThrows<IOException> { openHostile(server, WsRecorder(), timeoutMillis = 500) }
                val took = (System.nanoTime() - started) / 1_000_000
                assertTrue(took < 2_000, "gave up after $took ms (the response takes ~5 s to trickle)")
            }
        }

        case("every client frame is masked, each with a fresh mask") {
            HostileServer().use { server ->
                val ws = openHostile(server, WsRecorder())
                val peer = server.awaitPeer()
                repeat(8) { ws.sendBinary(byteArrayOf(it.toByte(), 1, 2, 3)) }
                val frames = (0 until 8).map { peer.readFrame()!! }
                assertTrue(frames.all { it.mask != null && it.fin && it.opcode == 2 }, "masked, whole, binary")
                assertEq((0 until 8).map { listOf(it.toByte(), 1, 2, 3) }, frames.map { it.payload.toList() })
                assertEq(8, frames.map { it.mask!!.toList() }.toSet().size, "eight frames, eight masks")
                ws.close()
                val close = peer.awaitClose()!!
                assertTrue(close.mask != null, "the close frame is masked too")
                assertEq(1000, close.closeCode)
            }
        }

        case("a masked frame from the server is a protocol error, answered with close 1002") {
            HostileServer().use { server ->
                val recorder = WsRecorder()
                openHostile(server, recorder)
                val peer = server.awaitPeer()
                peer.write(0x82, 0x83, 1, 2, 3, 4, 9, 9, 9)
                val error = recorder.awaitError()
                assertTrue(error is WebSocketProtocolException && error.code == 1002, "$error")
                assertEq(1002, peer.awaitClose()?.closeCode)
                assertTrue(recorder.messages.isEmpty())
            }
        }

        case("a frame longer than the cap is refused before it is allocated, with close 1009") {
            // 2^62 bytes: allocating it would be an OutOfMemoryError, not a protocol error.
            HostileServer().use { server ->
                val recorder = WsRecorder()
                openHostile(server, recorder, maxMessageBytes = 1_000)
                val peer = server.awaitPeer()
                peer.write(0x82, 127, 0x40, 0, 0, 0, 0, 0, 0, 0)
                val error = recorder.awaitError()
                assertTrue(error is WebSocketProtocolException && error.code == 1009, "$error")
                assertEq(1009, peer.awaitClose()?.closeCode)
            }
            // A length with the top bit set (negative as a Long) is no better.
            HostileServer().use { server ->
                val recorder = WsRecorder()
                openHostile(server, recorder)
                server.awaitPeer().write(0x82, 127, 0x80, 0, 0, 0, 0, 0, 0, 1)
                val error = recorder.awaitError()
                assertTrue(error is WebSocketProtocolException && error.code == 1002, "$error")
            }
        }

        case("fragments that add up to more than the cap are refused, with close 1009") {
            HostileServer().use { server ->
                val recorder = WsRecorder()
                openHostile(server, recorder, maxMessageBytes = 1_000)
                val peer = server.awaitPeer()
                peer.write(byteArrayOf(0x02, 126, 0x02, 0x58) + ByteArray(600)) // 600 bytes, not final
                peer.write(byteArrayOf(0x00, 126, 0x02, 0x58) + ByteArray(600)) // 600 more
                val error = recorder.awaitError()
                assertTrue(error is WebSocketProtocolException && error.code == 1009, "$error")
                assertEq(1009, peer.awaitClose()?.closeCode)
                assertTrue(recorder.messages.isEmpty())
            }
        }

        case("a fragmented message under the cap is reassembled, with a ping between its fragments") {
            HostileServer().use { server ->
                val recorder = WsRecorder()
                val ws = openHostile(server, recorder)
                val peer = server.awaitPeer()
                peer.write(0x02, 2, 1, 2)
                peer.write(0x89, 1, 7) // a ping in the middle of a message
                peer.write(0x80, 1, 3)
                assertEq(listOf<Byte>(1, 2, 3), recorder.messages.poll(5, TimeUnit.SECONDS)!!.toList())
                val pong = peer.readFrame()!!
                assertEq(10 to listOf<Byte>(7), pong.opcode to pong.payload.toList())
                ws.abort()
            }
        }

        case("a frame cut off in the middle is a dropped connection, not a protocol error") {
            HostileServer().use { server ->
                val recorder = WsRecorder()
                openHostile(server, recorder)
                val peer = server.awaitPeer()
                peer.write(byteArrayOf(0x82.toByte(), 100) + ByteArray(10))
                peer.socket.close()
                val error = recorder.awaitError()
                assertTrue(error !is WebSocketProtocolException && error.message!!.contains("middle"), "$error")
            }
            HostileServer().use { server ->
                val recorder = WsRecorder()
                openHostile(server, recorder)
                val peer = server.awaitPeer()
                peer.write(0x82, 126, 0x01) // the extended length is cut off too
                peer.socket.close()
                assertTrue(recorder.awaitError() !is WebSocketProtocolException)
            }
        }

        case("a close without a status is reported as 1005 and answered with 1000") {
            HostileServer().use { server ->
                val recorder = WsRecorder()
                val ws = openHostile(server, recorder)
                val peer = server.awaitPeer()
                peer.write(0x88, 0x00)
                eventually("the close is reported") { recorder.closes.isNotEmpty() }
                assertEq(listOf(1005 to ""), recorder.closes.toList())
                assertEq(1000, peer.awaitClose()?.closeCode)
                eventually("the client is closed") { !ws.isOpen }
                assertTrue(recorder.errors.isEmpty(), "${recorder.errors}")
            }
        }

        case("the close is echoed before the owner hears of it, so an owner that aborts at once does not swallow the echo") {
            // RemoteTransport aborts the socket from onClose; the echo must already be on the wire by then.
            HostileServer().use { server ->
                val client = java.util.concurrent.atomic.AtomicReference<WebSocketClient>()
                val heard = CopyOnWriteArrayList<Int>()
                val listener = object : WebSocketClient.Listener {
                    override fun onBinary(message: ByteArray) = Unit
                    override fun onText() = Unit
                    override fun onClose(code: Int, reason: String) {
                        heard.add(code)
                        client.get().abort()
                    }
                    override fun onError(cause: Throwable) = Unit
                }
                client.set(WebSocketClient.connect(server.url(), listener, 3_000))
                val peer = server.awaitPeer()
                peer.write(0x88, 2, 0x0F, 0xA1) // 4001
                assertEq(4001, peer.awaitClose()?.closeCode)
                // The owner hears of it right after the echo is out.
                eventually("the close is reported") { heard.isNotEmpty() }
                assertEq(listOf(4001), heard.toList())
            }
        }

        case("a close of one byte, or with a code no endpoint may send, is a protocol error") {
            for (payload in listOf(byteArrayOf(0x03), byteArrayOf(0x03, 0xED.toByte()), byteArrayOf(0x00, 0x05))) {
                HostileServer().use { server ->
                    val recorder = WsRecorder()
                    openHostile(server, recorder)
                    server.awaitPeer().write(byteArrayOf(0x88.toByte(), payload.size.toByte()) + payload)
                    val error = recorder.awaitError()
                    assertTrue(error is WebSocketProtocolException && error.code == 1002, "${payload.toList()}: $error")
                    assertTrue(recorder.closes.isEmpty())
                }
            }
        }

        case("a control frame over 125 bytes, or fragmented, is a protocol error") {
            HostileServer().use { server ->
                val recorder = WsRecorder()
                openHostile(server, recorder)
                server.awaitPeer().write(byteArrayOf(0x89.toByte(), 126, 0, 126) + ByteArray(126))
                assertTrue((recorder.awaitError() as WebSocketProtocolException).code == 1002)
            }
            HostileServer().use { server ->
                val recorder = WsRecorder()
                openHostile(server, recorder)
                server.awaitPeer().write(0x09, 0x00) // a ping without FIN
                assertTrue(recorder.awaitError() is WebSocketProtocolException)
            }
        }

        case("a reserved bit or an unknown opcode is a protocol error") {
            for (b0 in listOf(0xC2, 0x83, 0x8B)) {
                HostileServer().use { server ->
                    val recorder = WsRecorder()
                    openHostile(server, recorder)
                    server.awaitPeer().write(b0, 0x00)
                    assertTrue(recorder.awaitError() is WebSocketProtocolException, "first byte 0x${b0.toString(16)}")
                }
            }
        }

        case("an owner whose callback throws ends the connection with an error instead of losing the reader") {
            HostileServer().use { server ->
                val recorder = WsRecorder(throwOnBinary = true)
                val ws = openHostile(server, recorder)
                server.awaitPeer().write(0x82, 1, 42)
                val error = recorder.awaitError()
                assertTrue(error.message!!.contains("listener failed"), "$error")
                eventually("the client is closed") { !ws.isOpen }
            }
        }

        case("wss: the certificate must be trusted and name the host") {
            val context = HostileTlsCertificate.context() ?: skip("no keytool in this JDK")
            val tlsServer = context.serverSocketFactory.createServerSocket(0, 10, InetAddress.getLoopbackAddress()) as SSLServerSocket
            HostileServer(server = tlsServer).use { server ->
                // Trusted and named: the handshake and a message go through.
                val recorder = WsRecorder()
                val ws = WebSocketClient.connect(server.url("localhost", "wss"), recorder, 5_000, sslSocketFactory = context.socketFactory)
                val peer = server.awaitPeer()
                peer.write(0x82, 1, 5)
                assertEq(listOf<Byte>(5), recorder.messages.poll(5, TimeUnit.SECONDS)!!.toList())
                ws.abort()
                // Trusted, but the certificate names localhost, not 127.0.0.1: refused.
                val misnamed = assertThrows<IOException> {
                    WebSocketClient.connect(server.url("127.0.0.1", "wss"), WsRecorder(), 5_000, sslSocketFactory = context.socketFactory)
                }
                assertTrue(misnamed is javax.net.ssl.SSLException, "$misnamed")
                // The platform's trust store does not know the certificate: refused.
                val untrusted = assertThrows<IOException> { WebSocketClient.connect(server.url("localhost", "wss"), WsRecorder(), 5_000) }
                assertTrue(untrusted is javax.net.ssl.SSLException, "$untrusted")
            }
        }

        // ---- what the WebSocket port's adapter needs (ADR-047): text, subprotocols, headers, refusals, the read gate ----

        case("text messages arrive as text once checked to be UTF-8, fragments joined before the check") {
            HostileServer().use { server ->
                val recorder = WsRecorder()
                val ws = openHostile(server, recorder)
                val peer = server.awaitPeer()
                peer.write(serverFrame(1, "é😀".toByteArray()))
                assertEq("é😀", recorder.texts.poll(5, TimeUnit.SECONDS))
                // A two-byte character split across two fragments is whole once joined.
                peer.write(serverFrame(1, byteArrayOf(0xC3.toByte()), fin = false))
                peer.write(serverFrame(0, byteArrayOf(0xA9.toByte())))
                assertEq("é", recorder.texts.poll(5, TimeUnit.SECONDS))
                peer.write(serverFrame(1, ByteArray(0)))
                assertEq("", recorder.texts.poll(5, TimeUnit.SECONDS))
                ws.abort()
            }
        }

        case("a text message that is not UTF-8 (a stray byte, a surrogate, an overlong form) closes with 1007") {
            for (bad in listOf(byteArrayOf(0xFF.toByte()), byteArrayOf(0xED.toByte(), 0xA0.toByte(), 0x80.toByte()), byteArrayOf(0xC0.toByte(), 0xAF.toByte()), byteArrayOf(0xC3.toByte()))) {
                HostileServer().use { server ->
                    val recorder = WsRecorder()
                    openHostile(server, recorder)
                    val peer = server.awaitPeer()
                    peer.write(serverFrame(1, bad))
                    val error = recorder.awaitError()
                    assertTrue(error is WebSocketProtocolException && error.code == 1007, "$error")
                    assertEq(1007, peer.awaitClose()?.closeCode)
                    assertTrue(recorder.texts.isEmpty())
                }
            }
        }

        case("subprotocols are offered in order; the server's choice is the client's protocol; one never offered is refused") {
            HostileServer({ upgradeResponse(it, "Sec-WebSocket-Protocol: chat\r\n") }).use { server ->
                val ws = WebSocketClient.connect(server.url(), WsRecorder(), 3_000, protocols = listOf("chat", "v1"))
                assertEq("chat", ws.protocol)
                assertTrue(server.awaitPeer().head.contains("\r\nSec-WebSocket-Protocol: chat, v1\r\n"))
                ws.abort()
                val other = assertThrows<WebSocketHandshakeException> { WebSocketClient.connect(server.url(), WsRecorder(), 3_000, protocols = listOf("v1")) }
                assertTrue(other.message!!.contains("subprotocol"), other.message!!)
            }
            HostileServer().use { server ->
                val ws = WebSocketClient.connect(server.url(), WsRecorder(), 3_000, protocols = listOf("v1"))
                assertEq("", ws.protocol, "the server may choose none")
                ws.abort()
            }
        }

        case("extra headers go into the upgrade request as given; reserved and broken ones are not allowed") {
            HostileServer().use { server ->
                val ws = WebSocketClient.connect(server.url(), WsRecorder(), 3_000, headers = listOf("X-Token" to "t", "Authorization" to "Bearer é"))
                val head = server.awaitPeer().head
                assertTrue(head.contains("\r\nX-Token: t\r\n"), head)
                ws.abort()
                assertThrows<IllegalArgumentException> { WebSocketClient.connect(server.url(), WsRecorder(), 3_000, headers = listOf("X" to "a\r\nInjected: 1")) }
            }
            assertEq(null, WebSocketClient.checkHeader("X-Token", "t"))
            assertTrue(WebSocketClient.checkHeader("Host", "evil") != null)
            assertTrue(WebSocketClient.checkHeader("sec-websocket-key", "x") != null)
            assertTrue(WebSocketClient.checkHeader("bad name", "x") != null)
            assertTrue(WebSocketClient.checkHeader("X", "a\u0000") != null)
            assertTrue(WebSocketClient.checkProtocol("chat.v2") == null && WebSocketClient.checkProtocol("a b") != null && WebSocketClient.checkProtocol("") != null)
        }

        case("a refused upgrade reports its HTTP status") {
            HostileServer({ "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n" }).use { server ->
                val refused = assertThrows<WebSocketUpgradeException> { openHostile(server, WsRecorder()) }
                assertEq(403, refused.status)
                assertTrue(refused.message!!.contains("403"), refused.message!!)
            }
            HostileServer({ "HTTP/1.0 401 Unauthorized\r\n\r\n" }).use { server ->
                assertEq(401, assertThrows<WebSocketUpgradeException> { openHostile(server, WsRecorder()) }.status)
            }
            HostileServer({ "SPDY/9 nonsense\r\n\r\n" }).use { server ->
                assertEq(-1, assertThrows<WebSocketUpgradeException> { openHostile(server, WsRecorder()) }.status)
            }
        }

        case("the read gate pauses the reader: nothing is read while it is closed, the server's writes back up, and the pause is not silence") {
            HostileServer().use { server ->
                val recorder = WsRecorder()
                val gate = LatchGate()
                val ws = WebSocketClient.connect(server.url(), recorder, 3_000, pingAfterMillis = 300, readGate = gate)
                val peer = server.awaitPeer()
                val frame = serverFrame(2, ByteArray(60_000) { 7 })
                val total = 400
                val written = java.util.concurrent.atomic.AtomicInteger()
                val writer = Thread {
                    try {
                        repeat(total) {
                            peer.write(frame)
                            written.incrementAndGet()
                        }
                    } catch (e: IOException) {
                        // the test ended
                    }
                }.also { it.isDaemon = true }
                writer.start()
                Thread.sleep(700)
                assertEq(0, recorder.messages.size, "nothing is read while the gate is closed")
                assertTrue(written.get() < total, "the server's writes stall once the buffers are full: ${written.get()} of $total")
                gate.open()
                repeat(total) { assertTrue(recorder.messages.poll(5, TimeUnit.SECONDS) != null, "message $it arrives") }
                assertTrue(recorder.errors.isEmpty(), "a paused reader is not a silent server: ${recorder.errors}")
                assertTrue(ws.isOpen)
                ws.abort()
            }
        }

        case("text goes out as a text frame; the outbound buffer drains; close waits for the server's echo") {
            HostileServer().use { server ->
                val ws = openHostile(server, WsRecorder())
                val peer = server.awaitPeer()
                peer.socket.soTimeout = 5_000
                ws.sendText("héllo")
                val frame = peer.readFrame()!!
                assertEq(1, frame.opcode)
                assertEq("héllo", String(frame.payload, Charsets.UTF_8))
                eventually("the outbound buffer is empty") { ws.bufferedAmount == 0L }
                ws.close(4000, "bye")
                val close = peer.awaitClose()!!
                assertEq(4000, close.closeCode)
                assertTrue(!ws.awaitFinished(100), "not finished before the server answers")
                peer.write(0x88, 2, 0x0F, 0xA0)
                assertTrue(ws.awaitFinished(2_000), "finished once the server echoed the close")
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
