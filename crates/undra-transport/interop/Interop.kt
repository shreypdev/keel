import dev.undra.runtime.*
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import kotlinx.coroutines.runBlocking

// The shipped Kotlin RemoteTransport, unmodified, against the real Rust server. Not part of
// `cargo test`; see run.sh, which compiles this with the runtime's own sources.
fun main(args: Array<String>) {
    val url = args[0]
    val hash = args[1].removePrefix("0x").toULong(16)
    val counter = args[2].toUInt(); val newId = args[3].toUInt(); val addId = args[4].toUInt(); val sumId = args[5].toUInt()
    val askId = args[6].toUInt(); val echoPort = args[7].toUInt(); val echoMethod = args[8].toUInt()

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
    println("all Kotlin interop checks passed")
}
