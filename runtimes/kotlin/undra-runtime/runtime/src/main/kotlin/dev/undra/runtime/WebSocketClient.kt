package dev.undra.runtime

import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.ConnectException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketTimeoutException
import java.net.URI
import java.nio.charset.StandardCharsets
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.atomic.AtomicBoolean
import javax.net.ssl.HttpsURLConnection
import javax.net.ssl.SSLPeerUnverifiedException
import javax.net.ssl.SSLSocket
import javax.net.ssl.SSLSocketFactory

/**
 * The server broke RFC 6455 (a masked frame, a reserved bit, a malformed control frame, a message over the size
 * cap, a malformed close): retrying would meet the same server. [code] is the close code the client sent it
 * (1002, or 1009 for a message that is too big) before dropping the connection.
 */
internal class WebSocketProtocolException(val code: Int, message: String) : IOException(message)

/**
 * A small WebSocket client (RFC 6455) over `java.net.Socket`: the handshake, masked client frames,
 * fragmented messages, ping and pong, close. It exists because `java.net.http.WebSocket`, which the remote
 * transport used before, is not on Android; this one needs nothing but the JDK's (and Android's) core
 * classes, so the code that runs in the JVM tests is the code that runs on a phone.
 *
 * Threads: one reader and one writer, both daemons. **No call to a method of this class touches the
 * network on the calling thread**, which is what lets an Android app call into it from its main thread
 * (`NetworkOnMainThreadException`): [sendBinary] only enqueues, and [connect] is meant to be called from a
 * thread of the transport's own (it blocks for the connect and the handshake).
 *
 * The connection is watched: when the server has been silent for [pingAfterMillis] the client sends a
 * ping, and when it stays silent for as long again the connection is declared dead through
 * [Listener.onError] (a server that vanished without a FIN would otherwise be noticed by the operating
 * system after minutes).
 */
internal class WebSocketClient private constructor(
    private val socket: Socket,
    private val input: BufferedInputStream,
    private val output: OutputStream,
    private val listener: Listener,
    private val maxMessageBytes: Int,
    private val pingAfterMillis: Long,
) {
    /**
     * What the connection tells its owner. Called on the reader thread (an [onError] for a failed write on the
     * writer thread), once for the end of the connection, and never after [close] or [abort]. A callback that throws
     * ends the connection with [onError].
     */
    interface Listener {
        /** A whole binary message. */
        fun onBinary(message: ByteArray)

        /** A text message: Undra speaks binary, so the owner treats it as a protocol error. */
        fun onText()

        /** The server closed the connection (the client has echoed the close). */
        fun onClose(code: Int, reason: String)

        /**
         * The connection failed: an I/O error, a server that stopped answering, or (a [WebSocketProtocolException]) a
         * server that broke RFC 6455. It is over.
         */
        fun onError(cause: Throwable)
    }

    private sealed interface Outgoing {
        class Frame(val bytes: ByteArray) : Outgoing

        data object Stop : Outgoing
    }

    private val queue = LinkedBlockingQueue<Outgoing>()
    private val finished = AtomicBoolean(false)
    private val closedByUs = AtomicBoolean(false)
    private val closeSent = AtomicBoolean(false)

    @Volatile
    private var lastReceivedNanos = System.nanoTime()

    @Volatile
    private var closeStartedNanos = 0L

    @Volatile
    private var writer: Thread? = null

    @Volatile
    private var pinged = false

    /** `false` once the connection is over, whoever ended it. */
    val isOpen: Boolean get() = !finished.get() && !closedByUs.get()

    private fun start() {
        writer = Thread(::writeLoop, "undra-ws-writer").also {
            it.isDaemon = true
            it.start()
        }
        Thread(::readLoop, "undra-ws-reader").also { it.isDaemon = true }.start()
    }

    // ---- sending ---------------------------------------------------------------------------------------

    /** Queues [message] as one binary message. The write happens on the writer thread. */
    fun sendBinary(message: ByteArray) {
        if (!isOpen) throw IOException("the WebSocket is closed")
        queue.add(Outgoing.Frame(frame(OP_BINARY, message)))
    }

    /** Starts the closing handshake: a close frame is sent, then the socket is released once the server answers (or after two seconds). */
    fun close(code: Int = NORMAL_CLOSURE, reason: String = "") {
        if (!closedByUs.compareAndSet(false, true)) return
        closeStartedNanos = System.nanoTime()
        sendCloseFrame(code, reason)
        queue.add(Outgoing.Stop)
    }

    /**
     * Drops the connection without a closing handshake. The socket is released on a thread of the client's own:
     * closing a TLS socket writes (`close_notify`), and the caller may be Android's main thread.
     */
    fun abort() {
        closedByUs.set(true)
        finish(onOwnThread = true)
    }

    private fun sendCloseFrame(code: Int, reason: String) {
        if (!closeSent.compareAndSet(false, true)) return
        val text = utf8Prefix(reason, MAX_CONTROL_PAYLOAD - 2)
        val payload = ByteArray(2 + text.size)
        payload[0] = (code shr 8).toByte()
        payload[1] = code.toByte()
        System.arraycopy(text, 0, payload, 2, text.size)
        queue.add(Outgoing.Frame(frame(OP_CLOSE, payload)))
    }

    /** Waits a moment for the writer to put out what is queued (a close frame) before the socket goes. */
    private fun letWriterFinish() {
        val w = writer ?: return
        if (w === Thread.currentThread()) return
        try {
            w.join(CLOSE_ECHO_MILLIS)
        } catch (e: InterruptedException) {
            Thread.currentThread().interrupt()
        }
    }

    private fun writeLoop() {
        try {
            while (true) {
                when (val item = queue.take()) {
                    is Outgoing.Frame -> {
                        output.write(item.bytes)
                        // Several queued frames go out in one flush.
                        if (queue.isEmpty()) output.flush()
                    }
                    Outgoing.Stop -> {
                        output.flush()
                        // Half-close: the server sees our close frame and EOF; the reader releases the socket.
                        try {
                            socket.shutdownOutput()
                        } catch (e: IOException) {
                            // already closed, or TLS (no half-close): the reader's timeout releases it
                        } catch (e: UnsupportedOperationException) {
                            // SSL sockets
                        }
                        return
                    }
                }
            }
        } catch (e: IOException) {
            fail(e)
        } catch (e: InterruptedException) {
            Thread.currentThread().interrupt()
        }
    }

    // ---- receiving -------------------------------------------------------------------------------------

    private fun readLoop() {
        // The fragments of a message in progress; a message in one frame (the usual case) never goes through it.
        val message = ByteArrayOutputStream()
        var messageOpcode = -1
        try {
            while (true) {
                val b0 = readByte()
                val fin = b0 and 0x80 != 0
                if (b0 and 0x70 != 0) throw ProtocolViolation("the server set a reserved bit")
                val opcode = b0 and 0x0F
                if (opcode !in KNOWN_OPCODES) throw ProtocolViolation("the server sent a frame with the unknown opcode $opcode")
                val b1 = readByte()
                if (b1 and 0x80 != 0) throw ProtocolViolation("the server sent a masked frame")
                var length = (b1 and 0x7F).toLong()
                if (length == 126L) {
                    length = ((readByte() shl 8) or readByte()).toLong()
                } else if (length == 127L) {
                    length = 0
                    for (i in 0 until 8) length = (length shl 8) or readByte().toLong()
                    if (length < 0) throw ProtocolViolation("the server sent a frame of an impossible length")
                }
                val control = opcode >= 8
                if (control && (!fin || length > MAX_CONTROL_PAYLOAD)) throw ProtocolViolation("the server sent a malformed control frame")
                // Checked before anything is allocated: a hostile length costs nothing.
                if (!control && (length > maxMessageBytes || message.size() + length > maxMessageBytes)) {
                    throw ProtocolViolation("the server sent a message larger than $maxMessageBytes bytes", MESSAGE_TOO_BIG)
                }
                val payload = try {
                    ByteArray(length.toInt())
                } catch (e: OutOfMemoryError) {
                    // A message under the cap that this heap cannot hold (a small Android heap): a typed failure, not a dead app.
                    throw ProtocolViolation("the server sent a message of $length bytes, more than this process can hold", MESSAGE_TOO_BIG)
                }
                readExactly(payload)
                when (opcode) {
                    OP_CONTINUATION, OP_BINARY, OP_TEXT -> {
                        if (opcode == OP_CONTINUATION) {
                            if (messageOpcode < 0) throw ProtocolViolation("the server sent a continuation frame with nothing to continue")
                        } else {
                            if (messageOpcode >= 0) throw ProtocolViolation("the server started a message inside another")
                            messageOpcode = opcode
                        }
                        val whole = if (fin && message.size() == 0) {
                            payload
                        } else {
                            message.write(payload, 0, payload.size)
                            if (fin) message.toByteArray().also { message.reset() } else null
                        }
                        if (whole != null) {
                            val kind = messageOpcode
                            messageOpcode = -1
                            if (closedByUs.get()) continue
                            tell { if (kind == OP_BINARY) listener.onBinary(whole) else listener.onText() }
                        }
                    }
                    OP_CLOSE -> {
                        if (payload.size == 1) throw ProtocolViolation("the server sent a close frame with a one-byte payload")
                        val code = if (payload.size >= 2) ((payload[0].toInt() and 0xFF) shl 8) or (payload[1].toInt() and 0xFF) else NO_STATUS
                        if (payload.size >= 2 && !isValidCloseCode(code)) throw ProtocolViolation("the server sent the close code $code, which may not be sent")
                        val reason = if (payload.size > 2) String(payload, 2, payload.size - 2, StandardCharsets.UTF_8) else ""
                        val byUs = closedByUs.getAndSet(true)
                        sendCloseFrame(if (payload.size >= 2) code else NORMAL_CLOSURE, "")
                        queue.add(Outgoing.Stop)
                        // The echo reaches the server before the owner hears of the close (and may abort the socket).
                        letWriterFinish()
                        if (!byUs) tell { listener.onClose(code, reason) }
                        finish()
                        return
                    }
                    OP_PING -> if (!closedByUs.get()) queue.add(Outgoing.Frame(frame(OP_PONG, payload)))
                    else -> Unit // OP_PONG
                }
            }
        } catch (e: ListenerFailed) {
            fail(IOException("the WebSocket's listener failed: ${e.cause?.message}", e.cause))
        } catch (e: IOException) {
            fail(e)
        } catch (e: ProtocolViolation) {
            // RFC 6455 section 7.1.7: say why before dropping the connection (best effort, briefly).
            sendCloseFrame(e.code, e.message ?: "")
            queue.add(Outgoing.Stop)
            letWriterFinish()
            fail(WebSocketProtocolException(e.code, "protocol error: ${e.message}"))
        }
    }

    /** Runs a listener callback; one that throws ends the connection (the reader must not die silently). */
    private inline fun tell(callback: () -> Unit) {
        try {
            callback()
        } catch (e: RuntimeException) {
            throw ListenerFailed(e)
        }
    }

    private class ListenerFailed(cause: RuntimeException) : Exception(cause)

    /** Reads one byte, polling: a quiet server is pinged, and one that stays quiet is given up on. */
    private fun readByte(): Int {
        while (true) {
            try {
                val b = input.read()
                if (b < 0) throw IOException("the server closed the connection without a closing handshake")
                noteReceived()
                return b
            } catch (e: SocketTimeoutException) {
                idle()
            }
        }
    }

    private fun readExactly(into: ByteArray) {
        var offset = 0
        while (offset < into.size) {
            try {
                val n = input.read(into, offset, into.size - offset)
                if (n < 0) throw IOException("the server closed the connection in the middle of a message")
                offset += n
                noteReceived()
            } catch (e: SocketTimeoutException) {
                idle()
            }
        }
    }

    private fun noteReceived() {
        lastReceivedNanos = System.nanoTime()
        pinged = false
    }

    /** The socket read timed out: nothing arrived for a poll interval. */
    private fun idle() {
        if (closedByUs.get()) {
            // Our close frame is out and the server did not answer in time.
            if (closeStartedNanos != 0L && (System.nanoTime() - closeStartedNanos) / NANOS_PER_MILLI >= CLOSE_GRACE_MILLIS) {
                throw IOException("the server did not answer the close")
            }
            return
        }
        val quietMillis = (System.nanoTime() - lastReceivedNanos) / NANOS_PER_MILLI
        if (quietMillis < pingAfterMillis) return
        if (pinged) {
            if (quietMillis >= 2 * pingAfterMillis) throw IOException("the server stopped answering (nothing received for $quietMillis ms, not even a pong)")
            return
        }
        pinged = true
        queue.add(Outgoing.Frame(frame(OP_PING, ByteArray(0))))
    }

    private class ProtocolViolation(message: String, val code: Int = PROTOCOL_ERROR) : Exception(message)

    // ---- ending ----------------------------------------------------------------------------------------

    private fun fail(cause: Throwable) {
        val expected = closedByUs.get()
        val first = finish()
        if (first && !expected) {
            try {
                listener.onError(cause)
            } catch (e: RuntimeException) {
                UndraLog.warn("the WebSocket's listener failed while hearing of an error", e)
            }
        }
    }

    /** Releases the socket and the threads. `true` for the call that did it. */
    private fun finish(onOwnThread: Boolean = false): Boolean {
        if (!finished.compareAndSet(false, true)) return false
        queue.add(Outgoing.Stop)
        if (onOwnThread) {
            Thread(::closeSocket, "undra-ws-close").also { it.isDaemon = true }.start()
        } else {
            closeSocket()
        }
        return true
    }

    private fun closeSocket() {
        try {
            socket.close()
        } catch (e: IOException) {
            // nothing left to release
        } catch (e: RuntimeException) {
            // Android refuses network I/O (a TLS close) on its main thread; the reader's own failure releases it
            UndraLog.debug("closing the WebSocket failed: $e")
        }
    }

    // ---- framing ---------------------------------------------------------------------------------------

    /** One client frame: masked, as RFC 6455 requires of a client. */
    private fun frame(opcode: Int, payload: ByteArray): ByteArray {
        val headerSize = 2 + when {
            payload.size < 126 -> 0
            payload.size < 65_536 -> 2
            else -> 8
        } + 4
        val out = ByteArray(headerSize + payload.size)
        out[0] = (0x80 or opcode).toByte()
        var at = 1
        when {
            payload.size < 126 -> out[at++] = (0x80 or payload.size).toByte()
            payload.size < 65_536 -> {
                out[at++] = (0x80 or 126).toByte()
                out[at++] = (payload.size shr 8).toByte()
                out[at++] = payload.size.toByte()
            }
            else -> {
                out[at++] = (0x80 or 127).toByte()
                for (shift in 56 downTo 0 step 8) out[at++] = (payload.size.toLong() shr shift).toByte()
            }
        }
        val mask = ByteArray(4).also { RANDOM.nextBytes(it) }
        System.arraycopy(mask, 0, out, at, 4)
        at += 4
        for (i in payload.indices) out[at + i] = (payload[i].toInt() xor mask[i and 3].toInt()).toByte()
        return out
    }

    companion object {
        private const val OP_CONTINUATION = 0
        private const val OP_TEXT = 1
        private const val OP_BINARY = 2
        private const val OP_CLOSE = 8
        private const val OP_PING = 9
        private const val OP_PONG = 10
        private const val MAX_CONTROL_PAYLOAD = 125
        private val KNOWN_OPCODES = setOf(OP_CONTINUATION, OP_TEXT, OP_BINARY, OP_CLOSE, OP_PING, OP_PONG)
        private const val NORMAL_CLOSURE = 1000
        private const val PROTOCOL_ERROR = 1002
        private const val NO_STATUS = 1005
        private const val MESSAGE_TOO_BIG = 1009
        private const val NANOS_PER_MILLI = 1_000_000L
        private const val CLOSE_GRACE_MILLIS = 2_000L
        private const val CLOSE_ECHO_MILLIS = 500L
        private const val MAX_HEAD_BYTES = 16 * 1024
        private const val GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
        private val RANDOM = SecureRandom()

        /**
         * Connects to [uri] (`ws://` or `wss://`), performs the opening handshake and starts the reader and
         * writer threads. Blocks for at most about twice [timeoutMillis] (the connect, then the handshake, each
         * bounded by it) and does network I/O on the calling thread: call it from a thread of your own.
         *
         * `wss` uses [sslSocketFactory] (the platform's default when `null`) with host name verification on: a
         * certificate that does not name the host is refused like an untrusted one.
         *
         * @throws IOException if the server cannot be reached or does not complete the handshake.
         */
        fun connect(
            uri: URI,
            listener: Listener,
            timeoutMillis: Int,
            maxMessageBytes: Int = 64 * 1024 * 1024,
            pingAfterMillis: Long = 10_000L,
            sslSocketFactory: SSLSocketFactory? = null,
        ): WebSocketClient {
            val secure = uri.scheme.equals("wss", ignoreCase = true)
            val host = uri.host ?: throw IOException("the URL has no host")
            val port = if (uri.port >= 0) uri.port else if (secure) 443 else 80
            val socket = openSocket(host, port, secure, timeoutMillis, sslSocketFactory)
            try {
                val pollMillis = maxOf(10L, minOf(1_000L, pingAfterMillis / 4)).toInt()
                socket.soTimeout = timeoutMillis.coerceAtLeast(1)
                val handshakeDeadline = System.nanoTime() + timeoutMillis.coerceAtLeast(1) * NANOS_PER_MILLI
                val key = Base64.getEncoder().encodeToString(ByteArray(16).also { RANDOM.nextBytes(it) })
                val output = socket.getOutputStream()
                output.write(request(uri, host, port, secure, key).toByteArray(StandardCharsets.US_ASCII))
                output.flush()
                val input = BufferedInputStream(socket.getInputStream(), 16 * 1024)
                val head = readHead(input, handshakeDeadline)
                checkResponse(head, key)
                // The handshake is done; from now on reads poll, so a quiet server can be pinged.
                socket.soTimeout = pollMillis
                val client = WebSocketClient(socket, input, output, listener, maxMessageBytes, pingAfterMillis)
                client.start()
                return client
            } catch (e: Throwable) {
                try {
                    socket.close()
                } catch (closing: IOException) {
                    // the original failure is the one to report
                }
                throw e
            }
        }

        private fun openSocket(host: String, port: Int, secure: Boolean, timeoutMillis: Int, sslSocketFactory: SSLSocketFactory?): Socket {
            val addresses = try {
                InetAddress.getAllByName(host)
            } catch (e: IOException) {
                throw IOException("cannot resolve $host: ${e.message}", e)
            }
            var failure: IOException? = null
            val deadline = System.nanoTime() + timeoutMillis * NANOS_PER_MILLI
            for (address in addresses) {
                val left = ((deadline - System.nanoTime()) / NANOS_PER_MILLI).toInt()
                if (left <= 0) break
                val plain = Socket()
                try {
                    plain.tcpNoDelay = true
                    plain.keepAlive = true
                    plain.connect(InetSocketAddress(address, port), left)
                    if (!secure) return plain
                    val factory = sslSocketFactory ?: SSLSocketFactory.getDefault() as SSLSocketFactory
                    val tls = factory.createSocket(plain, host, port, true) as SSLSocket
                    // Host name verification during the handshake (the JDK and Conscrypt honour it) ...
                    tls.sslParameters = tls.sslParameters.also { it.endpointIdentificationAlgorithm = "HTTPS" }
                    tls.soTimeout = left
                    try {
                        tls.startHandshake()
                        // ... and, on Android, once more with the platform's verifier: an `SSLSocket` there is not
                        // documented to check the name by itself on every release. (The JDK's default verifier
                        // refuses everything, so it is not asked.)
                        if (Platform.isAndroid && !HttpsURLConnection.getDefaultHostnameVerifier().verify(host, tls.session)) {
                            throw SSLPeerUnverifiedException("the certificate of $host does not name it")
                        }
                    } catch (e: IOException) {
                        try {
                            tls.close()
                        } catch (closing: IOException) {
                            // the handshake failure is the one to report
                        }
                        throw e
                    }
                    return tls
                } catch (e: IOException) {
                    // A server that was reached and refused (TLS) says more than an address nobody listens on.
                    if (failure == null || e !is ConnectException) failure = e
                    try {
                        plain.close()
                    } catch (closing: IOException) {
                        // the connect failure is the one to report
                    }
                }
            }
            throw failure ?: IOException("timed out connecting to $host:$port")
        }

        private fun request(uri: URI, host: String, port: Int, secure: Boolean, key: String): String {
            val path = (uri.rawPath?.takeIf { it.isNotEmpty() } ?: "/") + (uri.rawQuery?.let { "?$it" } ?: "")
            val shownHost = if (host.contains(':')) "[$host]" else host
            val defaultPort = if (secure) 443 else 80
            val authority = if (port == defaultPort) shownHost else "$shownHost:$port"
            return "GET $path HTTP/1.1\r\n" +
                "Host: $authority\r\n" +
                "Upgrade: websocket\r\n" +
                "Connection: Upgrade\r\n" +
                "Sec-WebSocket-Key: $key\r\n" +
                "Sec-WebSocket-Version: 13\r\n\r\n"
        }

        /**
         * The response head, up to and including the blank line; nothing after it is consumed. A server that
         * trickles it is given up on at [deadlineNanos] (each read is also bounded by the socket's timeout).
         */
        private fun readHead(input: InputStream, deadlineNanos: Long): String {
            val head = StringBuilder()
            while (!head.endsWith("\r\n\r\n")) {
                if (System.nanoTime() - deadlineNanos > 0) throw IOException("the server did not complete the handshake in time")
                val b = input.read()
                if (b < 0) throw IOException("the server closed the connection during the handshake")
                head.append(b.toChar())
                if (head.length > MAX_HEAD_BYTES) throw IOException("the server's handshake response is too long")
            }
            return head.toString()
        }

        /** RFC 6455 section 4.1: the checks a client must make of the server's answer to its upgrade request. */
        private fun checkResponse(head: String, key: String) {
            val lines = head.split("\r\n")
            val status = lines.first()
            if (!status.startsWith("HTTP/1.1 101")) throw IOException("the server did not upgrade the connection: ${status.take(80)}")
            fun header(name: String): String? =
                lines.drop(1).firstOrNull { it.startsWith("$name:", ignoreCase = true) }?.substringAfter(':')?.trim()
            if (!header("Upgrade").equals("websocket", ignoreCase = true)) throw IOException("the server's upgrade response has no Upgrade: websocket")
            val connection = header("Connection")?.split(',')?.map { it.trim() }.orEmpty()
            if (connection.none { it.equals("upgrade", ignoreCase = true) }) throw IOException("the server's upgrade response has no Connection: Upgrade")
            val expected = Base64.getEncoder().encodeToString(MessageDigest.getInstance("SHA-1").digest((key + GUID).toByteArray(StandardCharsets.US_ASCII)))
            if (header("Sec-WebSocket-Accept") != expected) throw IOException("the server's Sec-WebSocket-Accept is wrong")
            // This client asks for no extension and no subprotocol, so the server may not pick one.
            if (!header("Sec-WebSocket-Extensions").isNullOrEmpty()) throw IOException("the server chose a WebSocket extension this client did not offer")
            if (!header("Sec-WebSocket-Protocol").isNullOrEmpty()) throw IOException("the server chose a WebSocket subprotocol this client did not offer")
        }

        /** Whether a close frame may carry [code] (RFC 6455 section 7.4 and the IANA registry). */
        private fun isValidCloseCode(code: Int): Boolean =
            code in 1000..1003 || code in 1007..1014 || code in 3000..4999

        /** The UTF-8 bytes of [text], cut to at most [max] bytes without splitting a character. */
        private fun utf8Prefix(text: String, max: Int): ByteArray {
            val bytes = text.toByteArray(StandardCharsets.UTF_8)
            if (bytes.size <= max) return bytes
            var end = max
            // Back off continuation bytes (10xxxxxx) so the cut falls on a character boundary.
            while (end > 0 && (bytes[end].toInt() and 0xC0) == 0x80) end--
            return bytes.copyOf(end)
        }
    }
}
