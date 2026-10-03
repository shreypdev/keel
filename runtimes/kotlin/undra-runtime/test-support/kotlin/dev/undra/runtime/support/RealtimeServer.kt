package dev.undra.runtime.support

import java.io.BufferedReader
import java.io.File
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URI
import java.util.concurrent.TimeUnit

/**
 * The shared local server of the real-time scenarios and failure-injection suites
 * (`contract-tests/servers/realtime-server.mjs`, ADR-047), run with Node: `node <script> --port 0 --exit-on-stdin-close`,
 * which prints `READY <port>`. Closing this object closes the server's stdin, so a test run that dies takes it along.
 */
class RealtimeServer private constructor(private val process: Process, val port: Int) : AutoCloseable {
    /** `ws://127.0.0.1:<port>`. */
    val ws: String get() = "ws://127.0.0.1:$port"

    /** `http://127.0.0.1:<port>`. */
    val http: String get() = "http://127.0.0.1:$port"

    /** One connection as `/stats` reports it. */
    class Connection(val json: Map<String, Any?>) {
        val path: String get() = json["path"] as String
        val closeCode: Int? get() = (json["closeCode"] as Double?)?.toInt()
        val closeReason: String? get() = json["closeReason"] as String?
        val written: Int get() = (json["written"] as Double).toInt()
        val clientClosed: Boolean get() = json["clientClosed"] as Boolean

        @Suppress("UNCHECKED_CAST")
        val headers: Map<String, String> get() = json["headers"] as Map<String, String>

        @Suppress("UNCHECKED_CAST")
        val protocols: List<String> get() = json["protocols"] as List<String>

        override fun toString(): String = json.toString()
    }

    /** Every connection since the last [reset], oldest first. */
    @Suppress("UNCHECKED_CAST")
    fun connections(): List<Connection> {
        val stats = MiniJson.parse(get("/stats")) as Map<String, Any?>
        return (stats["connections"] as List<Map<String, Any?>>).map(::Connection)
    }

    /** The newest connection to [path]. */
    fun last(path: String): Connection = connections().lastOrNull { it.path == path } ?: throw AssertionError("no connection to $path in ${connections()}")

    /** Forgets the connections. */
    fun reset() {
        get("/reset")
    }

    private fun get(path: String): String {
        val connection = URI(http + path).toURL().openConnection() as HttpURLConnection
        try {
            connection.connectTimeout = 5_000
            connection.readTimeout = 5_000
            return connection.inputStream.use { String(it.readBytes()) }
        } finally {
            connection.disconnect()
        }
    }

    override fun close() {
        try {
            process.outputStream.close()
        } catch (e: java.io.IOException) {
            // already gone
        }
        if (!process.waitFor(2, TimeUnit.SECONDS)) process.destroyForcibly()
    }

    companion object {
        /** Where the script is: the system property `undra.realtime.server`, else found above the working directory. */
        fun script(): File? {
            System.getProperty("undra.realtime.server")?.let { return File(it).takeIf(File::isFile) }
            var dir: File? = File(System.getProperty("user.dir") ?: ".").absoluteFile
            while (dir != null) {
                val candidate = File(dir, "contract-tests/servers/realtime-server.mjs")
                if (candidate.isFile) return candidate
                dir = dir.parentFile
            }
            return null
        }

        /** Starts the server, or returns why it cannot be started here (no script, no Node). */
        fun start(): Result<RealtimeServer> {
            val script = script() ?: return Result.failure(IllegalStateException("contract-tests/servers/realtime-server.mjs not found above ${System.getProperty("user.dir")}"))
            val process = try {
                ProcessBuilder("node", script.path, "--port", "0", "--exit-on-stdin-close")
                    .redirectError(ProcessBuilder.Redirect.INHERIT)
                    .start()
            } catch (e: java.io.IOException) {
                return Result.failure(IllegalStateException("node is not on PATH (${e.message})"))
            }
            val line = BufferedReader(InputStreamReader(process.inputStream)).readLine()
            if (line == null || !line.startsWith("READY ")) {
                process.destroyForcibly()
                return Result.failure(IllegalStateException("the realtime server did not start: $line"))
            }
            return Result.success(RealtimeServer(process, line.removePrefix("READY ").trim().toInt()))
        }
    }
}

/** A JSON reader for test fixtures: objects are maps, arrays lists, numbers doubles. */
object MiniJson {
    fun parse(text: String): Any? {
        val reader = Reader(text)
        val value = reader.value()
        reader.space()
        check(reader.at == text.length) { "trailing JSON at ${reader.at}" }
        return value
    }

    private class Reader(val s: String) {
        var at = 0

        fun space() {
            while (at < s.length && s[at].isWhitespace()) at++
        }

        fun value(): Any? {
            space()
            return when (s[at]) {
                '{' -> obj()
                '[' -> arr()
                '"' -> str()
                't' -> literal("true", true)
                'f' -> literal("false", false)
                'n' -> literal("null", null)
                else -> num()
            }
        }

        fun literal(word: String, value: Any?): Any? {
            check(s.startsWith(word, at)) { "bad literal at $at" }
            at += word.length
            return value
        }

        fun num(): Double {
            val start = at
            while (at < s.length && (s[at].isDigit() || s[at] in "+-.eE")) at++
            return s.substring(start, at).toDouble()
        }

        fun str(): String {
            at++
            val out = StringBuilder()
            while (s[at] != '"') {
                if (s[at] == '\\') {
                    at++
                    when (val c = s[at]) {
                        'n' -> out.append('\n')
                        't' -> out.append('\t')
                        'r' -> out.append('\r')
                        'b' -> out.append('\b')
                        'f' -> out.append('\u000c')
                        'u' -> {
                            out.append(s.substring(at + 1, at + 5).toInt(16).toChar())
                            at += 4
                        }
                        else -> out.append(c)
                    }
                    at++
                } else {
                    out.append(s[at++])
                }
            }
            at++
            return out.toString()
        }

        fun arr(): List<Any?> {
            at++
            val out = ArrayList<Any?>()
            space()
            if (s[at] == ']') {
                at++
                return out
            }
            while (true) {
                out.add(value())
                space()
                if (s[at++] == ']') return out
            }
        }

        fun obj(): Map<String, Any?> {
            at++
            val out = LinkedHashMap<String, Any?>()
            space()
            if (s[at] == '}') {
                at++
                return out
            }
            while (true) {
                space()
                val key = str()
                space()
                at++ // ':'
                out[key] = value()
                space()
                if (s[at++] == '}') return out
            }
        }
    }
}
