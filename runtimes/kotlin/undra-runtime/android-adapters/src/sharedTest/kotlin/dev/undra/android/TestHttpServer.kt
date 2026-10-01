package dev.undra.android

import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * A small HTTP/1.1 server on the loopback interface for the adapter tests: it parses requests, lets each test script the
 * answer (status, headers, a body sent whole or in timed chunks, silence, a dropped connection) and records what
 * arrived. One thread per connection; every response closes its connection.
 */
class TestHttpServer : AutoCloseable {
    /** One request as the server read it. */
    class Request(val method: String, val target: String, val headers: List<Pair<String, String>>, val body: ByteArray) {
        /** The first value of the header [name] (any case), or `null`. */
        fun header(name: String): String? = headers.firstOrNull { it.first.equals(name, ignoreCase = true) }?.second

        /** Every value of the header [name], in the order sent. */
        fun headers(name: String): List<String> = headers.filter { it.first.equals(name, ignoreCase = true) }.map { it.second }

        /** The path of [target], without the query. */
        val path: String get() = target.substringBefore('?')
    }

    /** One connection's request, and the means to answer it. */
    inner class Exchange(val request: Request, private val socket: Socket) {
        private val out: OutputStream = socket.getOutputStream()

        /** Answers with [body] in one piece. */
        fun respond(status: Int, headers: List<Pair<String, String>> = emptyList(), body: ByteArray = ByteArray(0), contentLength: Long = body.size.toLong()) {
            val head = StringBuilder("HTTP/1.1 $status ${reason(status)}\r\n")
            for ((n, v) in headers) head.append(n).append(": ").append(v).append("\r\n")
            head.append("Content-Length: ").append(contentLength).append("\r\nConnection: close\r\n\r\n")
            out.write(head.toString().toByteArray(Charsets.ISO_8859_1))
            out.write(body)
            out.flush()
        }

        /** Starts a chunked answer; follow with [chunk] and [endChunked]. */
        fun startChunked(status: Int = 200, headers: List<Pair<String, String>> = emptyList()) {
            val head = StringBuilder("HTTP/1.1 $status ${reason(status)}\r\n")
            for ((n, v) in headers) head.append(n).append(": ").append(v).append("\r\n")
            head.append("Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
            out.write(head.toString().toByteArray(Charsets.ISO_8859_1))
            out.flush()
        }

        /** Sends one chunk of a chunked answer. */
        fun chunk(bytes: ByteArray) {
            out.write("${Integer.toHexString(bytes.size)}\r\n".toByteArray(Charsets.ISO_8859_1))
            out.write(bytes)
            out.write("\r\n".toByteArray(Charsets.ISO_8859_1))
            out.flush()
        }

        /** Ends a chunked answer. */
        fun endChunked() {
            out.write("0\r\n\r\n".toByteArray(Charsets.ISO_8859_1))
            out.flush()
        }

        /** Sends the head of an answer that declares [contentLength] bytes and no body yet. */
        fun respondHeadOnly(status: Int, contentLength: Long, headers: List<Pair<String, String>> = emptyList()) {
            respond(status, headers, ByteArray(0), contentLength)
        }

        /** Blocks until the client has closed the connection (or [timeoutMs] passes); `true` if it did. */
        fun awaitClientClose(timeoutMs: Long): Boolean {
            val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs)
            socket.soTimeout = 100
            val input = socket.getInputStream()
            while (System.nanoTime() < deadline) {
                try {
                    if (input.read() < 0) return true
                } catch (e: java.net.SocketTimeoutException) {
                    continue
                } catch (e: IOException) {
                    return true
                }
            }
            return false
        }

        /** Drops the connection without answering. */
        fun drop() {
            try {
                socket.setSoLinger(true, 0) // reset instead of a clean close
            } catch (e: SocketException) {
                // already closed
            }
            socket.close()
        }

        /** Writes raw bytes to the connection, for answers no well-behaved server gives. */
        fun raw(text: String) {
            out.write(text.toByteArray(Charsets.ISO_8859_1))
            out.flush()
        }
    }

    /** What a test says about one path. */
    fun interface Handler {
        /** Answers [exchange]. */
        fun handle(exchange: Exchange)
    }

    private val server = ServerSocket(0, 64, InetAddress.getByName("127.0.0.1"))
    private val routes = ConcurrentHashMap<String, Handler>()
    private val seen = CopyOnWriteArrayList<Request>()
    private val open = CopyOnWriteArrayList<Socket>()
    private val activeHandlers = AtomicInteger(0)

    @Volatile
    private var closed = false

    /** The port the server listens on. */
    val port: Int get() = server.localPort

    /** Where the server is, with a trailing slash omitted: `http://127.0.0.1:<port>`. */
    val base: String get() = "http://127.0.0.1:$port"

    /** Every request received so far, in arrival order. */
    val requests: List<Request> get() = seen.toList()

    private val acceptor = Thread({
        while (!closed) {
            val socket = try {
                server.accept()
            } catch (e: IOException) {
                break
            }
            open.add(socket)
            Thread({ serve(socket) }, "test-http-conn").apply { isDaemon = true }.start()
        }
    }, "test-http-accept").apply {
        isDaemon = true
        start()
    }

    /** Answers requests whose path is [path] with [handler]. */
    fun route(path: String, handler: Handler) {
        routes[path] = handler
    }

    /** Answers requests whose path is [path] with [status] and [body]. */
    fun fixed(path: String, status: Int = 200, body: String = "", headers: List<Pair<String, String>> = emptyList()) =
        route(path) { it.respond(status, headers, body.toByteArray(Charsets.UTF_8)) }

    /** Waits until [count] requests have been received. */
    fun awaitRequests(count: Int, timeoutMs: Long = 5_000): Boolean {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs)
        while (System.nanoTime() < deadline) {
            if (seen.size >= count) return true
            Thread.sleep(10)
        }
        return seen.size >= count
    }

    private fun serve(socket: Socket) {
        activeHandlers.incrementAndGet()
        try {
            val input = BufferedInputStream(socket.getInputStream())
            val request = readRequest(input) ?: return
            seen.add(request)
            val handler = routes[request.path] ?: Handler { it.respond(404, body = "no route for ${request.path}".toByteArray()) }
            handler.handle(Exchange(request, socket))
        } catch (e: IOException) {
            // the client went away mid-answer: part of what the tests do
        } finally {
            activeHandlers.decrementAndGet()
            try {
                socket.close()
            } catch (e: IOException) {
                // closed
            }
            open.remove(socket)
        }
    }

    private fun readRequest(input: InputStream): Request? {
        val requestLine = readLine(input) ?: return null
        val parts = requestLine.split(' ')
        if (parts.size < 2) return null
        val headers = ArrayList<Pair<String, String>>()
        while (true) {
            val line = readLine(input) ?: return null
            if (line.isEmpty()) break
            val colon = line.indexOf(':')
            if (colon > 0) headers.add(line.substring(0, colon).trim() to line.substring(colon + 1).trim())
        }
        val length = headers.firstOrNull { it.first.equals("Content-Length", true) }?.second?.toLongOrNull()
        val chunked = headers.any { it.first.equals("Transfer-Encoding", true) && it.second.contains("chunked", true) }
        val body = when {
            chunked -> readChunked(input)
            length != null -> input.readNBytesCompat(length.toInt())
            else -> ByteArray(0)
        }
        return Request(parts[0], parts[1], headers, body)
    }

    private fun readChunked(input: InputStream): ByteArray {
        val out = ByteArrayOutputStream()
        while (true) {
            val size = (readLine(input) ?: break).trim().substringBefore(';').toInt(16)
            if (size == 0) {
                readLine(input)
                break
            }
            out.write(input.readNBytesCompat(size))
            readLine(input)
        }
        return out.toByteArray()
    }

    private fun readLine(input: InputStream): String? {
        val line = ByteArrayOutputStream()
        while (true) {
            val b = input.read()
            if (b < 0) return if (line.size() == 0) null else line.toString(Charsets.ISO_8859_1.name())
            if (b == '\n'.code) break
            if (b != '\r'.code) line.write(b)
        }
        return line.toString(Charsets.ISO_8859_1.name())
    }

    private fun InputStream.readNBytesCompat(n: Int): ByteArray {
        val buffer = ByteArray(n)
        var read = 0
        while (read < n) {
            val r = read(buffer, read, n - read)
            if (r < 0) throw IOException("the client closed the connection mid-body")
            read += r
        }
        return buffer
    }

    /** Blocks until every request being served has finished (or [timeoutMs] passes); `true` if none is left. */
    fun awaitIdle(timeoutMs: Long = 5_000): Boolean {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs)
        while (System.nanoTime() < deadline) {
            if (activeHandlers.get() == 0) return true
            Thread.sleep(10)
        }
        return activeHandlers.get() == 0
    }

    override fun close() {
        closed = true
        try {
            server.close()
        } catch (e: IOException) {
            // closed
        }
        for (socket in open) {
            try {
                socket.close()
            } catch (e: IOException) {
                // closed
            }
        }
    }

    private fun reason(status: Int): String =
        when (status) {
            200 -> "OK"
            201 -> "Created"
            204 -> "No Content"
            301 -> "Moved Permanently"
            302 -> "Found"
            303 -> "See Other"
            307 -> "Temporary Redirect"
            308 -> "Permanent Redirect"
            400 -> "Bad Request"
            401 -> "Unauthorized"
            404 -> "Not Found"
            500 -> "Internal Server Error"
            503 -> "Service Unavailable"
            else -> "Status"
        }
}
