// Runs the generated Kotlin of the `stdlib` golden case: the standard library is left out of the
// generated package, and everything that refers to it runs on the runtime's own types and codecs
// (ADR-024). Compiled and run by `tests/typecheck_kotlin.rs`; exits non-zero on the first failed
// expectation.

package golden.stdlib

import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.Mirror
import dev.undra.runtime.adapters.AppState
import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.NetKind
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking

private fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }

private fun bytes(text: String): ByteArray =
    ByteArray(text.length / 2) { text.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

private fun expect(condition: Boolean, message: String) {
    if (!condition) throw AssertionError(message)
}

private fun <T> expectEq(actual: T, expected: T, message: String) {
    if (actual != expected) throw AssertionError("$message: expected <$expected>, got <$actual>")
}

private inline fun <reified E : Throwable> expectThrows(message: String, block: () -> Unit): E {
    try {
        block()
    } catch (e: Throwable) {
        if (e is E) return e
        throw AssertionError("$message: expected ${E::class.simpleName}, got $e")
    }
    throw AssertionError("$message: nothing was thrown")
}

private class FakeMirror : Mirror() {
    val callbacks = mutableMapOf<Long, (UInt, ChangeOp, UndraReader) -> Unit>()

    override fun register(handle: Long, apply: (UInt, ChangeOp, UndraReader) -> Unit) {
        callbacks[handle] = apply
    }

    override fun unregister(handle: Long) {
        callbacks.remove(handle)
    }
}

private class FakeCore : UndraCore() {
    val calls = mutableListOf<Pair<UInt, String>>()
    val replies = ArrayDeque<Any>()
    val streams = ArrayDeque<List<Any>>()
    var nextHandle = 7L
    private val fakeMirror = FakeMirror()
    override val mirror: Mirror get() = fakeMirror

    private fun reply(): ByteArray = when (val next = replies.removeFirstOrNull()) {
        null -> ByteArray(0)
        is Throwable -> throw next
        else -> next as ByteArray
    }

    override fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        calls += methodId to args.hex()
        return reply()
    }

    override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        calls += methodId to args.hex()
        return reply()
    }

    override fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray> {
        calls += methodId to args.hex()
        val items = streams.removeFirstOrNull() ?: emptyList()
        return flow {
            for (item in items) {
                if (item is Throwable) throw item
                emit(item as ByteArray)
            }
        }
    }

    override fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long {
        reply()
        return nextHandle++
    }

    override fun observe(handle: Long, signalId: UInt, on: Boolean) = Unit

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) = Unit

    override fun release(handle: Long) = Unit

    fun deliver(handle: Long, signalId: Int, op: ChangeOp, value: ByteArray) {
        fakeMirror.callbacks.getValue(handle)(signalId.toUInt(), op, UndraReader(value))
    }
}

private fun replyError(value: ByteArray) = UndraReplyException(ReplyStatus.ERROR, value)

private val request = HttpRequest(
    method = HttpMethod.POST,
    url = "/u",
    headers = listOf(Header("a", "b")),
    body = byteArrayOf(1),
    timeoutMs = 5000u,
)
private val response = HttpResponse(204u.toUShort(), emptyList(), ByteArray(0))
private val endpoint = Endpoint("x", request, response, listOf(HttpMethod.GET, HttpMethod.POST), listOf(Header("c", "d")))

fun main() {
    nothingIsGenerated()
    records()
    errors()
    calls()
    store()
    port()
    println("ok")
}

private fun nothingIsGenerated() {
    val names = listOf(
        "Header", "HttpMethod", "HttpRequest", "HttpResponse", "HttpError", "FsError", "NetKind", "AppState",
        "Clock", "Rng", "Log", "Http", "Kv", "SecureStore", "Fs", "Timer", "Lifecycle", "LifecycleEvents",
        "ConnectivityEvents", "ClockKt", "HttpKt",
    )
    for (name in names) {
        expect(runCatching { Class.forName("golden.stdlib.$name") }.isFailure, "golden.stdlib.$name is generated")
    }
    // The app's own type may share the name of a standard port.
    Class.forName("golden.stdlib.Connectivity")
}

private fun records() {
    val expected = "01000000" + "78" +
        request.let { HttpRequest.encodeToByteArray(it).hex() } +
        "01" + HttpResponse.encodeToByteArray(response).hex() +
        "02000000" + "0000" + "0100" +
        "01000000" + "01000000" + "63" + "01000000" + "64"
    expectEq(Endpoint.encodeToByteArray(endpoint).hex(), expected, "Endpoint encoding")
    expectEq(Endpoint.decodeAll(bytes(expected)), endpoint, "Endpoint decoding")
    expectThrows<WireException.InvalidTag>("unknown method") {
        Endpoint.decodeAll(bytes(expected.replace("02000000" + "0000" + "0100", "02000000" + "0000" + "0900")))
    }
    val connectivity = Connectivity(online = true, kind = NetKind.WIRED, app = AppState.BACKGROUND)
    expectEq(Connectivity.encodeToByteArray(connectivity).hex(), "01" + "0200" + "01" + "0200", "Connectivity")
    expectEq(Connectivity.decodeAll(bytes("01" + "0200" + "01" + "0200")), connectivity, "Connectivity decoding")
}

private fun errors() {
    val wrapped = SyncError.Http(HttpError.Timeout)
    expectEq(wrapped.message, "the request timed out", "the runtime's message")
    expectEq(wrapped.cause, HttpError.Timeout, "cause")
    expectEq(SyncError.encodeToByteArray(wrapped).hex(), "0100" + "0100", "nested standard error")
    val disk = SyncError.decodeAll(bytes("0200" + "0200" + "02000000" + "6d6d" + "01000000" + "78"))
    expect(disk is SyncError.Disk && disk.value0 == FsError.Io("mm"), "decoded standard error")
    expectEq(disk.message, "disk failure at x", "message")
}

private fun calls() {
    val core = FakeCore()
    val syncer = Syncer(core)

    core.replies.add(HttpResponse.encodeToByteArray(response))
    expectEq(runBlocking { syncer.send(request) }, response, "send")
    expectEq(core.calls.last().second, HttpRequest.encodeToByteArray(request).hex(), "send arguments")

    core.replies.add(replyError(HttpError.encodeToByteArray(HttpError.Network("down"))))
    expectEq(expectThrows<HttpError.Network>("typed error") { runBlocking { syncer.send(request) } }.reason, "down", "typed error")
    // Anything but an error reply maps onto the closed set.
    val panic = UndraReplyException(ReplyStatus.PANIC, ByteArray(0))
    core.replies.add(panic)
    expectThrows<UndraCallError.Panicked>("panic") { runBlocking { syncer.send(request) } }

    core.replies.add(replyError(FsError.encodeToByteArray(FsError.Denied)))
    expectThrows<FsError.Denied>("fs error") { runBlocking { syncer.save("/tmp/x") } }

    core.replies.add(replyError(SyncError.encodeToByteArray(SyncError.Offline)))
    expectThrows<SyncError.Offline>("generated error") { runBlocking { syncer.sync() } }

    core.streams.add(listOf(HttpResponse.encodeToByteArray(response), HttpResponse.encodeToByteArray(response)))
    expectEq(runBlocking { syncer.follow(endpoint).toList() }, listOf(response, response), "stream items")
    core.streams.add(listOf(HttpResponse.encodeToByteArray(response), replyError(HttpError.encodeToByteArray(HttpError.Cancelled))))
    expectThrows<HttpError.Cancelled>("stream failure") { runBlocking { syncer.follow(endpoint).toList() } }
}

private fun store() {
    val core = FakeCore()
    core.nextHandle = 9L
    val link = Link(core)
    expectEq(link.state.value, AppState.ACTIVE, "placeholder state")
    expectEq(link.kind.value, NetKind.WIFI, "placeholder kind")
    expectEq(link.last.value, null, "placeholder response")
    expectEq(link.failure.value, null, "placeholder failure")
    expectEq(link.pending.value, emptyList(), "placeholder requests")

    core.deliver(9L, 0, ChangeOp.FULL, AppState.encodeToByteArray(AppState.INACTIVE))
    core.deliver(9L, 1, ChangeOp.FULL, NetKind.encodeToByteArray(NetKind.NONE))
    core.deliver(9L, 2, ChangeOp.FULL, Codecs.option(HttpResponse).encodeToByteArray(response))
    core.deliver(9L, 3, ChangeOp.FULL, Codecs.option(HttpError).encodeToByteArray(HttpError.InvalidUrl("x")))
    core.deliver(9L, 4, ChangeOp.FULL, Codecs.vec(HttpRequest).encodeToByteArray(listOf(request)))
    expectEq(link.state.value, AppState.INACTIVE, "state")
    expectEq(link.kind.value, NetKind.NONE, "kind")
    expectEq(link.last.value, response, "last")
    expectEq(link.failure.value, HttpError.InvalidUrl("x"), "failure")
    expectEq(link.pending.value, listOf(request), "pending")
}

private fun port() {
    val upload = uploaderPortImpl(object : Uploader {
        override suspend fun upload(request: HttpRequest): HttpResponse {
            if (request.url == "/cancel") throw HttpError.Cancelled
            return response
        }
    }).methods.getValue(UndraIds.Ports.Uploader.UPLOAD)
    expectEq(
        runBlocking { upload(HttpRequest.encodeToByteArray(request)) }.hex(),
        HttpResponse.encodeToByteArray(response).hex(),
        "reply",
    )
    val failure = expectThrows<UndraPortException>("typed port failure") {
        runBlocking { upload(HttpRequest.encodeToByteArray(request.copy(url = "/cancel"))) }
    }
    expectEq(failure.body.hex(), "0200", "encoded error")
}
