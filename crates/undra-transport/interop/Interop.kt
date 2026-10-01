import dev.undra.runtime.*
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import kotlinx.coroutines.runBlocking
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.time.Duration.Companion.milliseconds

// The shipped Kotlin RemoteTransport, unmodified, against the real Rust server. Not part of
// `cargo test`; see run.sh, which compiles this with the runtime's own sources.
fun main(args: Array<String>) {
    val url = args[0]
    val hash = args[1].removePrefix("0x").toULong(16)
    val counter = args[2].toUInt(); val newId = args[3].toUInt(); val addId = args[4].toUInt(); val sumId = args[5].toUInt()
    val askId = args[6].toUInt(); val echoPort = args[7].toUInt(); val echoMethod = args[8].toUInt()
    val serveBinary = args[9]

    val echoed = mutableListOf<Int>()
    val ports = mapOf(echoPort to PortImpl(false, mapOf(echoMethod to { a: ByteArray ->
        val x = Codecs.i32.decodeAll(a); echoed.add(x); Codecs.i32.encodeToByteArray(x + 1)
    })))
    val core = UndraCore.load(LoadOptions(mode = Mode.REMOTE, remoteUrl = url, expectedSchemaHash = hash, adapters = ports, defaultAdapters = false))
    println("ok  handshake (${core.mode})")
    val two = Codecs.i32.encodeToByteArray(2); val forty = Codecs.i32.encodeToByteArray(40)
    val s = Codecs.i32.decodeAll(runBlocking { core.call(CallTarget.FreeFunction(sumId), sumId, two + forty) })
    check(s == 42) { "sum was $s" }
    println("ok  free function call")
    val handle = core.construct(counter, newId, Codecs.i32.encodeToByteArray(5))
    val added = Codecs.i32.decodeAll(runBlocking { core.call(CallTarget.ObjectMethod(Handle(handle), addId), addId, Codecs.i32.encodeToByteArray(3)) })
    check(added == 8) { "add was $added" }
    println("ok  constructor and method (callSync/construct block for the reply)")
    val asked = Codecs.i32.decodeAll(runBlocking { core.call(CallTarget.ObjectMethod(Handle(handle), askId), askId, Codecs.i32.encodeToByteArray(41)) })
    check(asked == 42 && echoed == listOf(41)) { "ask was $asked, echoed $echoed" }
    println("ok  port call answered by the JVM adapter")
    try {
        UndraCore.load(LoadOptions(mode = Mode.REMOTE, remoteUrl = url, expectedSchemaHash = hash xor 0xffffuL, defaultAdapters = false))
        error("a mismatched schema was accepted")
    } catch (e: UndraSchemaMismatchException) {
        check(e.got == hash && e.expected == (hash xor 0xffffuL))
        println("ok  schema mismatch rejected as UndraSchemaMismatchException")
    }
    core.close()
    reconnecting(serveBinary, hash, counter, newId, addId)
    println("all Kotlin interop checks passed")
}

/** A running `serve` example. */
private class Server(binary: String, address: String?) {
    val process: Process = ProcessBuilder(listOfNotNull(binary, address)).redirectError(ProcessBuilder.Redirect.DISCARD).start()
    val url: String
    init {
        val line = process.inputStream.bufferedReader().readLine()
        url = Regex("\"url\":\"([^\"]+)\"").find(line)!!.groupValues[1]
    }
    fun stop() {
        process.outputStream.close() // the server runs until its stdin closes
        process.waitFor()
    }
}

/** A TCP proxy that can cut every connection it carries: a Wi-Fi blip between the app and the dev server. */
private class Proxy(private val port: Int) : AutoCloseable {
    private val listener = ServerSocket(0)
    private val open = CopyOnWriteArrayList<Socket>()
    val url: String get() = "ws://127.0.0.1:${listener.localPort}"
    init {
        Thread {
            while (true) {
                val client = try { listener.accept() } catch (e: Exception) { break }
                val upstream = try { Socket("127.0.0.1", port) } catch (e: Exception) { client.close(); continue }
                open += client; open += upstream
                for ((from, to) in listOf(client to upstream, upstream to client)) {
                    Thread { try { from.getInputStream().copyTo(to.getOutputStream()) } catch (e: Exception) { } finally { runCatching { from.close() }; runCatching { to.close() } } }.also { it.isDaemon = true }.start()
                }
            }
        }.also { it.isDaemon = true }.start()
    }
    fun dropAll() { open.forEach { runCatching { it.close() } }; open.clear() }
    override fun close() { runCatching { listener.close() }; dropAll() }
}

/** ADR-034 against the real server: a dropped connection resumes the same objects; a restarted server is a lost session. */
private fun reconnecting(binary: String, hash: ULong, counter: UInt, newId: UInt, addId: UInt) {
    var server = Server(binary, null)
    val port = server.url.substringAfterLast(':').toInt()
    Proxy(port).use { proxy ->
        val states = CopyOnWriteArrayList<ConnectionState>()
        val pc = UndraCore.load(LoadOptions(
            mode = Mode.REMOTE, remoteUrl = proxy.url, expectedSchemaHash = hash, defaultAdapters = false,
            reconnect = ReconnectPolicy(initialDelay = 50.milliseconds, maxDelay = 200.milliseconds),
            onConnectionChange = { states += it },
        ))
        val handle = pc.construct(counter, newId, Codecs.i32.encodeToByteArray(5))
        val target = CallTarget.ObjectMethod(Handle(handle), addId)
        val seen = CopyOnWriteArrayList<Int>()
        pc.mirror.register(handle) { _, _, r -> seen += Codecs.i32.decode(r) }
        pc.observe(handle, UInt.MAX_VALUE, true)
        check(Codecs.i32.decodeAll(runBlocking { pc.call(target, addId, Codecs.i32.encodeToByteArray(3)) }) == 8)
        proxy.dropAll()
        val deadline = System.currentTimeMillis() + 10_000
        while ((states.lastOrNull() != ConnectionState.Connected || states.size < 3) && System.currentTimeMillis() < deadline) Thread.sleep(20)
        check(states.last() == ConnectionState.Connected && states.any { it is ConnectionState.Reconnecting }) { "states: $states" }
        println("ok  a dropped connection was reconnected: ${states.map { it::class.simpleName }}")
        // The server kept the object: same handle, same state; the mirror was observed again.
        check(Codecs.i32.decodeAll(runBlocking { pc.call(target, addId, Codecs.i32.encodeToByteArray(1)) }) == 9)
        val settle = System.currentTimeMillis() + 5_000
        while (seen.lastOrNull() != 9 && System.currentTimeMillis() < settle) Thread.sleep(20)
        check(seen.last() == 9) { "mirror saw $seen" }
        println("ok  the session resumed: same object, same state (9), mirror converged")
        // The core is rebuilt: the server restarts on the same address.
        server.stop()
        server = Server(binary, "127.0.0.1:$port")
        val lost = System.currentTimeMillis() + 15_000
        while (states.last() !is ConnectionState.Closed && System.currentTimeMillis() < lost) Thread.sleep(20)
        val closed = states.last() as? ConnectionState.Closed
        check(closed != null && closed.reason == ClosedReason.SESSION_LOST && closed.cause is UndraSessionLostException) { "states: $states" }
        check(states.count { it is ConnectionState.Closed } == 1)
        println("ok  a restarted server: the client reports a lost session, once")
    }
    server.stop()
}
