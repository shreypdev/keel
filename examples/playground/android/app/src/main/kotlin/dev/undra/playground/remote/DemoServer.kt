package dev.undra.playground.remote

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.util.Log
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketException
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/**
 * The server behind the Remote tab: a small HTTP server on the loopback interface of the device, so that the core's
 * requests go through the real `Http` adapter (`HttpURLConnection` over a real socket) and need no backend. It answers
 * the routes of `remote.rs`, in memory:
 *
 * | Request | Answer |
 * |---|---|
 * | `GET /lists/{list}/todos` | 200, a JSON array of `{"id", "title", "done"}` |
 * | `POST /lists/{list}/todos` with `{"title"}` | 201, the created item; a repeat with the same `Idempotency-Key` returns the same item |
 * | `PATCH /lists/{list}/todos/{id}` with `{"done"}` | 200, the item; 404 if there is none |
 *
 * Every answer takes [LATENCY_MS] milliseconds, so optimistic updates are visible. The server stands in for a host on
 * the internet, so it is reachable only while the device has a network: with airplane mode on, or while [offline] is
 * set (the Offline switch of the screen), it drops the connection without an answer, which the core sees as
 * `HttpError.Network`, as on a device that is really cut off. The `inbox` list starts with three items. Each request is
 * logged under the tag [TAG] (`adb logcat UndraDemoServer:I *:S`).
 *
 * @param context any context of the app; used to ask whether the device has a network.
 */
class DemoServer(context: Context) {
    private val connectivity = context.applicationContext.getSystemService(ConnectivityManager::class.java)

    /** Whether the Offline switch is on: the server then drops every connection. */
    @Volatile
    var offline: Boolean = false

    private class Todo(val id: Int, val title: String, var done: Boolean) {
        fun toJson(): JSONObject = JSONObject().put("id", id).put("title", title).put("done", done)
    }

    private class Request(val method: String, val target: String, val headers: Map<String, String>, val body: String)

    private val lock = Any()
    private val lists = HashMap<String, MutableList<Todo>>()
    private val created = HashMap<String, Todo>() // by Idempotency-Key
    private var nextId = 1
    private var server: ServerSocket? = null

    init {
        for (title in listOf("Buy milk", "Walk the dog", "Write Undra")) add("inbox", title)
    }

    /** Starts listening on a free loopback port and returns the base URL to pass to `configureRemote`. */
    fun start(): String {
        val socket = ServerSocket(0, BACKLOG, InetAddress.getByName("127.0.0.1"))
        server = socket
        Thread({
            while (!socket.isClosed) {
                val client = try {
                    socket.accept()
                } catch (e: SocketException) {
                    break
                }
                Thread({ serve(client) }, "demo-server-connection").apply { isDaemon = true }.start()
            }
        }, "demo-server").apply { isDaemon = true }.start()
        return "http://127.0.0.1:${socket.localPort}"
    }

    private fun serve(client: Socket) {
        client.use {
            try {
                val request = read(client.getInputStream()) ?: return
                Thread.sleep(LATENCY_MS)
                val key = request.headers["idempotency-key"]
                if (offline || !deviceHasNetwork()) {
                    Log.i(TAG, "${request.method} ${request.target} dropped: the network is down${key?.let { " (Idempotency-Key $it)" }.orEmpty()}")
                    client.setSoLinger(true, 0) // reset the connection instead of closing it politely
                    return
                }
                val (status, body) = answer(request)
                Log.i(TAG, "${request.method} ${request.target} -> $status${key?.let { " (Idempotency-Key $it)" }.orEmpty()}")
                write(client, status, body)
            } catch (e: IOException) {
                // the client went away mid-request
            }
        }
    }

    private fun deviceHasNetwork(): Boolean {
        val network = connectivity?.activeNetwork ?: return false
        return connectivity.getNetworkCapabilities(network)?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
    }

    private fun answer(request: Request): Pair<Int, String> {
        val path = request.target.substringBefore('?')
        val route = ROUTE.matchEntire(path) ?: return 404 to "not found"
        val list = route.groupValues[1]
        val id = route.groupValues[2].toIntOrNull()
        return try {
            when {
                request.method == "GET" && id == null -> 200 to listJson(list)
                request.method == "POST" && id == null -> create(list, request)
                request.method == "PATCH" && id != null -> patch(list, id, request)
                else -> 405 to "method not allowed"
            }
        } catch (e: JSONException) {
            400 to "bad request: ${e.message}"
        }
    }

    private fun listJson(list: String): String = synchronized(lock) {
        JSONArray(lists[list].orEmpty().map { it.toJson() }).toString()
    }

    private fun create(list: String, request: Request): Pair<Int, String> {
        val title = JSONObject(request.body).getString("title")
        val key = request.headers["idempotency-key"]
        val todo = synchronized(lock) {
            // A replay of a request the server already handled (the core queued it while offline) is not a second item.
            key?.let { created[it] } ?: add(list, title).also { if (key != null) created[key] = it }
        }
        return 201 to todo.toJson().toString()
    }

    private fun patch(list: String, id: Int, request: Request): Pair<Int, String> {
        val done = JSONObject(request.body).getBoolean("done")
        val todo = synchronized(lock) { lists[list]?.firstOrNull { it.id == id }?.also { it.done = done } }
        return if (todo == null) 404 to "no item $id in $list" else 200 to todo.toJson().toString()
    }

    private fun add(list: String, title: String): Todo = synchronized(lock) {
        Todo(nextId++, title, done = false).also { lists.getOrPut(list) { mutableListOf() }.add(it) }
    }

    /** Reads one HTTP/1.1 request; `null` if the client closed the connection before sending one. */
    private fun read(input: InputStream): Request? {
        val stream = BufferedInputStream(input)
        val requestLine = readLine(stream) ?: return null
        val parts = requestLine.split(' ')
        if (parts.size < 2) return null
        val headers = HashMap<String, String>()
        while (true) {
            val line = readLine(stream) ?: return null
            if (line.isEmpty()) break
            val colon = line.indexOf(':')
            if (colon > 0) headers[line.substring(0, colon).trim().lowercase()] = line.substring(colon + 1).trim()
        }
        val length = headers["content-length"]?.toIntOrNull() ?: 0
        val body = ByteArray(length)
        var read = 0
        while (read < length) {
            val n = stream.read(body, read, length - read)
            if (n < 0) return null
            read += n
        }
        return Request(parts[0], parts[1], headers, String(body, Charsets.UTF_8))
    }

    private fun readLine(stream: InputStream): String? {
        val line = ByteArrayOutputStream()
        while (true) {
            val b = stream.read()
            if (b < 0) return if (line.size() == 0) null else line.toString(Charsets.ISO_8859_1.name())
            if (b == '\n'.code) break
            if (b != '\r'.code) line.write(b)
        }
        return line.toString(Charsets.ISO_8859_1.name())
    }

    private fun write(client: Socket, status: Int, body: String) {
        val bytes = body.toByteArray(Charsets.UTF_8)
        val type = if (status < 400) "application/json" else "text/plain; charset=utf-8"
        val head = "HTTP/1.1 $status ${REASONS[status] ?: "Status"}\r\nContent-Type: $type\r\nContent-Length: ${bytes.size}\r\nConnection: close\r\n\r\n"
        client.getOutputStream().apply {
            write(head.toByteArray(Charsets.ISO_8859_1))
            write(bytes)
            flush()
        }
    }

    /** How long the server takes to answer, and its log tag. */
    companion object {
        /** The log tag of the server's request log. */
        const val TAG: String = "UndraDemoServer"

        /** Every answer takes this long. */
        const val LATENCY_MS: Long = 300L

        private const val BACKLOG = 16
        private val ROUTE = Regex("^/lists/([^/]+)/todos(?:/(\\d+))?$")
        private val REASONS = mapOf(200 to "OK", 201 to "Created", 400 to "Bad Request", 404 to "Not Found", 405 to "Method Not Allowed")
    }
}
