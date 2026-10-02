package dev.undra.contract

import java.io.BufferedReader
import java.io.File
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URI
import java.util.concurrent.TimeUnit

/**
 * The shared local server of S23 and S24 (`contract-tests/servers/realtime-server.mjs`), started once per run with
 * `node <script> --port 0 --exit-on-stdin-close` (it prints `READY <port>`), on first use. The runner's exit closes its
 * stdin, which ends it.
 */
object RealtimeServer {
    private var running: Pair<Process, Int>? = null

    /** The server's port, starting it on first use. */
    @Synchronized
    fun port(): Int {
        running?.let { return it.second }
        val script = script() ?: fail("contract-tests/servers/realtime-server.mjs not found above ${System.getProperty("user.dir")}")
        val process = ProcessBuilder("node", script.path, "--port", "0", "--exit-on-stdin-close")
            .redirectError(ProcessBuilder.Redirect.INHERIT)
            .start()
        val line = BufferedReader(InputStreamReader(process.inputStream)).readLine()
        if (line == null || !line.startsWith("READY ")) {
            process.destroyForcibly()
            fail("the realtime server did not start: $line")
        }
        val port = line.removePrefix("READY ").trim().toInt()
        running = process to port
        return port
    }

    /** `ws://127.0.0.1:<port>`: scenarios.md's `WS`. */
    val ws: String get() = "ws://127.0.0.1:${port()}"

    /** `http://127.0.0.1:<port>`: scenarios.md's `HTTP`. */
    val http: String get() = "http://127.0.0.1:${port()}"

    /** One connection of `/stats`. */
    class Connection(private val json: Map<String, Any?>) {
        val path: String get() = json["path"] as String
        val closeCode: Long? get() = json["closeCode"] as Long?
        val closeReason: String? get() = json["closeReason"] as String?
        val clientClosed: Boolean get() = json["clientClosed"] as Boolean

        @Suppress("UNCHECKED_CAST")
        val headers: Map<String, Any?> get() = json["headers"] as Map<String, Any?>

        override fun toString(): String = json.toString()
    }

    /** The connections the server saw, oldest first. */
    @Suppress("UNCHECKED_CAST")
    fun connections(): List<Connection> {
        val connection = URI("$http/stats").toURL().openConnection() as HttpURLConnection
        try {
            connection.connectTimeout = 5_000
            connection.readTimeout = 5_000
            val text = connection.inputStream.use { String(it.readBytes()) }
            return (Json.parseObject(text)["connections"] as List<Map<String, Any?>>).map(::Connection)
        } finally {
            connection.disconnect()
        }
    }

    /** The newest connection whose path is [path]. */
    fun last(path: String): Connection = connections().lastOrNull { it.path == path } ?: fail("the server saw no connection to $path")

    /** Stops the server (the runner calls it before exiting). */
    @Synchronized
    fun stop() {
        val (process, _) = running ?: return
        running = null
        try {
            process.outputStream.close()
        } catch (e: java.io.IOException) {
            // already gone
        }
        if (!process.waitFor(2, TimeUnit.SECONDS)) process.destroyForcibly()
    }

    private fun script(): File? {
        var dir: File? = File(System.getProperty("user.dir") ?: ".").absoluteFile
        while (dir != null) {
            val candidate = File(dir, "contract-tests/servers/realtime-server.mjs")
            if (candidate.isFile) return candidate
            dir = dir.parentFile
        }
        return null
    }
}
