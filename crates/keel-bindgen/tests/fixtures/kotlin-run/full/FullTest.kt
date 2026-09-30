// Runs the generated Kotlin of the `full` golden case against the real wire layer and a fake core.
// Compiled and run by `tests/typecheck_kotlin.rs`; exits non-zero on the first failed expectation.

package golden.full

import dev.keel.runtime.KeelCore
import dev.keel.runtime.KeelPortException
import dev.keel.runtime.KeelReplyException
import dev.keel.runtime.Mirror
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.Handle
import dev.keel.runtime.wire.KeelReader
import dev.keel.runtime.wire.KeelWriter
import dev.keel.runtime.wire.KeyedPatch
import dev.keel.runtime.wire.PatchOp
import dev.keel.runtime.wire.Payloads.CallTarget
import dev.keel.runtime.wire.Payloads.ChangeOp
import dev.keel.runtime.wire.Payloads.ReplyStatus
import dev.keel.runtime.wire.Timestamp
import dev.keel.runtime.wire.WireException
import dev.keel.runtime.wire.decodeAll
import dev.keel.runtime.wire.encodeToByteArray
import java.util.UUID
import kotlin.time.Duration.Companion.milliseconds
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

/** What the fake core saw of one call. */
private data class Call(val target: CallTarget, val methodId: UInt, val args: String)

private class FakeMirror : Mirror() {
    val callbacks = mutableMapOf<Long, (UInt, ChangeOp, KeelReader) -> Unit>()

    override fun register(handle: Long, apply: (UInt, ChangeOp, KeelReader) -> Unit) {
        callbacks[handle] = apply
    }

    override fun unregister(handle: Long) {
        callbacks.remove(handle)
    }
}

private class FakeCore : KeelCore() {
    val calls = mutableListOf<Call>()
    val constructed = mutableListOf<Triple<UInt, UInt, String>>()
    val observed = mutableListOf<Triple<Long, UInt, Boolean>>()
    val events = mutableListOf<Triple<UInt, UInt, String>>()
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
        calls += Call(target, methodId, args.hex())
        return reply()
    }

    override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        calls += Call(target, methodId, args.hex())
        return reply()
    }

    override fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray> {
        calls += Call(target, methodId, args.hex())
        val items = streams.removeFirstOrNull() ?: emptyList()
        return flow {
            for (item in items) {
                if (item is Throwable) throw item
                emit(item as ByteArray)
            }
        }
    }

    override fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long {
        constructed += Triple(typeId, methodId, args.hex())
        reply()
        return nextHandle++
    }

    override fun observe(handle: Long, signalId: UInt, on: Boolean) {
        observed += Triple(handle, signalId, on)
    }

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) {
        events += Triple(portId, methodId, payload.hex())
    }

    override fun release(handle: Long) = Unit

    fun deliver(handle: Long, signalId: Int, op: ChangeOp, value: ByteArray) {
        fakeMirror.callbacks.getValue(handle)(signalId.toUInt(), op, KeelReader(value))
    }
}

private fun replyError(value: ByteArray) = KeelReplyException(ReplyStatus.ERROR, value)

fun main() {
    records()
    errors()
    objects()
    stores()
    ports()
    queries()
    println("ok")
}

private val todo = Todo(
    id = UUID.fromString("00112233-4455-6677-8899-aabbccddeeff"),
    title = "Milk",
    done = true,
    tags = listOf("a", "bc"),
    due = Timestamp(1700000000000L),
    priority = Priority.HIGH,
)
private const val TODO_BYTES =
    "00112233445566778899aabbccddeeff" + "04000000" + "4d696c6b" + "01" +
        "02000000" + "0100000061" + "020000006263" + "01" + "0068e5cf8b010000" + "0200"

private fun records() {
    expectEq(Todo.encodeToByteArray(todo).hex(), TODO_BYTES, "Todo encoding")
    expectEq(Todo.decodeAll(bytes(TODO_BYTES)), todo, "Todo decoding")
    expect(todo.copy(title = "Bread") != todo, "data class copy")
    expectEq(Todo(todo.id, "x", tags = emptyList(), due = null, priority = Priority.LOW).done, false, "default field")

    val circle = Shape.Circle(1.5)
    expectEq(Shape.encodeToByteArray(circle).hex(), "0000" + "000000000000f83f", "Shape.Circle")
    expectEq(Shape.decodeAll(bytes("0100" + "000000000000f83f" + "000000000000f83f")), Shape.Rect(1.5, 1.5), "Shape.Rect")
    expectEq(Shape.decodeAll(bytes("0200")), Shape.Empty, "Shape.Empty")
    val tag = expectThrows<WireException.InvalidTag>("unknown variant") { Shape.decodeAll(bytes("0900")) }
    expectEq(tag.tag, 9u, "the offending tag")
    expectEq(Filter.encodeToByteArray(Filter.DONE).hex(), "0200", "Filter")
    expectThrows<WireException.InvalidTag>("unknown filter") { Filter.decodeAll(bytes("0700")) }
    expectThrows<WireException.TrailingBytes>("trailing bytes") { Todo.decodeAll(bytes(TODO_BYTES + "00")) }

    val request = HttpRequest("GET", "/", mapOf("b" to "2", "a" to "1"), byteArrayOf(1, 2, 3))
    val decoded = HttpRequest.decodeAll(HttpRequest.encodeToByteArray(request))
    // Byte arrays compare by content in generated data classes.
    expectEq(decoded, request, "HttpRequest round trip")
    expectEq(decoded.hashCode(), request.hashCode(), "hash of equal requests")
    expect(decoded != request.copy(body = byteArrayOf(9)), "different bodies differ")
    expect(request.copy(body = null) != request, "null body differs")
    expectEq(
        HttpRequest.encodeToByteArray(request.copy(headers = mapOf("a" to "1", "b" to "2"))).hex(),
        HttpRequest.encodeToByteArray(request).hex(),
        "map order does not change the bytes",
    )
    val response = HttpResponse(200u.toUShort(), emptyMap(), byteArrayOf(9), 2.milliseconds)
    expectEq(HttpResponse.decodeAll(HttpResponse.encodeToByteArray(response)), response, "HttpResponse round trip")
    val page = Page(listOf(todo), null, ULong.MAX_VALUE)
    expectEq(Page.decodeAll(Page.encodeToByteArray(page)), page, "Page round trip")
}

private fun errors() {
    val notFound = TodoError.NotFound("x1")
    expectEq(notFound.message, "todo x1 not found", "message")
    val asThrowable: Throwable = notFound
    expect(asThrowable is dev.keel.runtime.KeelException, "hierarchy")
    val wrapped = TodoError.Http(HttpError.Status(404u.toUShort()))
    expectEq(wrapped.message, "status 404", "transparent message")
    expectEq(wrapped.cause, HttpError.Status(404u.toUShort()), "cause")
    expectEq(TodoError.encodeToByteArray(wrapped).hex(), "0100" + "0100" + "9401", "nested error")
    val decoded = TodoError.decodeAll(bytes("0100" + "0100" + "9401"))
    expect(decoded is TodoError.Http && decoded.cause is HttpError.Status, "decoded nested error")
    expectEq(TodoError.EmptyTitle.message, "title cannot be empty", "unit variant")
    expectEq(TodoError.Storage("disk").message, "storage failure (disk)", "named field")

    val typed = TodoError.fromReply(replyError(TodoError.encodeToByteArray(TodoError.EmptyTitle)))
    expectEq(typed, TodoError.EmptyTitle, "typed error from a reply")
    val panic = KeelReplyException(ReplyStatus.PANIC, ByteArray(0))
    expect(TodoError.fromReply(panic) === panic, "other replies pass through")
}

private fun objects() {
    val core = FakeCore()
    val calc = Calculator(core)
    expectEq(core.constructed.single(), Triple(KeelIds.Objects.Calculator.TYPE_ID, KeelIds.Objects.Calculator.NEW, ""), "constructor call")
    expectEq(calc.handle, 7L, "handle")
    expectEq(core.observed.size, 0, "plain objects are not observed")

    core.replies.add(bytes("05000000"))
    expectEq(calc.add(2, 3), 5, "sync method")
    expectEq(
        core.calls.last(),
        Call(CallTarget.ObjectMethod(Handle(7L), KeelIds.Objects.Calculator.ADD), KeelIds.Objects.Calculator.ADD, "0200000003000000"),
        "sync call",
    )

    runBlocking {
        core.replies.add(replyError(HttpError.encodeToByteArray(HttpError.Timeout)))
        expectThrows<HttpError.Timeout>("typed suspend failure") { runBlocking { calc.fetch("https://example.com") } }
        expectEq(core.calls.last().args, "13000000" + "68747470733a2f2f6578616d706c652e636f6d", "fetch arguments")
        core.replies.add(Codecs.string.encodeToByteArray("ok"))
        expectEq(calc.fetch("u"), "ok", "suspend method")
        val panic = KeelReplyException(ReplyStatus.PANIC, ByteArray(0))
        core.replies.add(panic)
        val thrown = expectThrows<KeelReplyException>("other failures") { runBlocking { calc.fetch("u") } }
        expect(thrown === panic, "the same exception")

        core.streams.add(listOf(bytes("01000000"), bytes("02000000")))
        expectEq(calc.ticks().toList(), listOf(1u, 2u), "stream items")
        core.streams.add(listOf(Todo.encodeToByteArray(todo), replyError(TodoError.encodeToByteArray(TodoError.EmptyTitle))))
        val seen = mutableListOf<Todo>()
        expectThrows<TodoError.EmptyTitle>("stream failure") {
            runBlocking { calc.watch(Priority.HIGH).collect { seen += it } }
        }
        expectEq(seen, listOf(todo), "items before the failure")
    }
    expectEq(core.calls.last().args, "0200", "stream arguments")

    core.replies.add(Codecs.string.encodeToByteArray("hello"))
    expectEq(greet("keel", core), "hello", "free function")
    expectEq(core.calls.last().target, CallTarget.FreeFunction(KeelIds.Functions.GREET), "function target")
    core.replies.add(replyError(TodoError.encodeToByteArray(TodoError.EmptyTitle)))
    expectThrows<TodoError.EmptyTitle>("function failure") { runBlocking { ping(core) } }
}

private fun stores() {
    val core = FakeCore()
    core.nextHandle = 9L
    val store = TodoStore.create(core)
    expectEq(core.observed.single(), Triple(9L, UInt.MAX_VALUE, true), "observe all signals")
    expectEq(store.todos.value, emptyList(), "placeholder")
    expectEq(store.filter.value, Filter.ALL, "placeholder filter")

    val todoList = Codecs.vec(Todo)
    core.deliver(9L, 0, ChangeOp.FULL, todoList.encodeToByteArray(listOf(todo)))
    core.deliver(9L, 1, ChangeOp.FULL, Filter.encodeToByteArray(Filter.DONE))
    core.deliver(9L, 3, ChangeOp.FULL, Codecs.u32.encodeToByteArray(4u))
    core.deliver(9L, 4, ChangeOp.FULL, Codecs.option(Todo).encodeToByteArray(todo))
    expectEq(store.todos.value, listOf(todo), "todos")
    expectEq(store.filter.value, Filter.DONE, "filter")
    expectEq(store.remaining.value, 4u, "computed")
    expectEq(store.selected.value, todo, "option")

    val second = todo.copy(title = "Bread")
    core.deliver(9L, 0, ChangeOp.PATCH, KeyedPatch.encodePatch(listOf(PatchOp.Insert(1u, second)), Todo))
    expectEq(store.todos.value, listOf(todo, second), "insert patch")
    core.deliver(9L, 0, ChangeOp.PATCH, KeyedPatch.encodePatch(listOf(PatchOp.Move(1u, 0u), PatchOp.Remove(1u)), Todo))
    expectEq(store.todos.value, listOf(second), "move and remove patch")

    // A patch that does not fit desynchronises the signal: it is re-observed for a full value.
    core.observed.clear()
    core.deliver(9L, 0, ChangeOp.PATCH, KeyedPatch.encodePatch(listOf(PatchOp.Remove(9u)), Todo))
    expectEq(core.observed.toList(), listOf(Triple(9L, 0u, false), Triple(9L, 0u, true)), "resync")
    expectEq(store.todos.value, listOf(second), "value kept after a bad patch")

    core.deliver(9L, 42, ChangeOp.FULL, ByteArray(0))
    core.deliver(9L, 1, ChangeOp.INVALIDATED, ByteArray(0))
    expectEq(store.filter.value, Filter.DONE, "unknown signals and invalidations are ignored")

    store.setFilter(Filter.ALL)
    expectEq(core.calls.last().args, "0000", "store method")
    core.replies.add(replyError(TodoError.encodeToByteArray(TodoError.NotFound("db"))))
    expectThrows<TodoError.NotFound>("fallible async constructor") { runBlocking { TodoStore.open("/tmp/db", core) } }

    val fromConvenience = TodoStore(core)
    expect(core.observed.last() == Triple(fromConvenience.handle, UInt.MAX_VALUE, true), "convenience constructor observes")
}

private fun ports() {
    val clock = clockPortImpl(object : Clock {
        override fun nowMs(): Long = 5L
        override fun monotonicNs(): ULong = 6uL
    })
    expect(clock.sync, "clock is a sync port")
    expectEq(runBlocking { clock.methods.getValue(KeelIds.Ports.Clock.NOW_MS)(ByteArray(0)) }.hex(), "0500000000000000", "sync port reply")

    var seen: HttpRequest? = null
    val http = httpPortImpl(object : Http {
        override suspend fun request(req: HttpRequest): HttpResponse {
            seen = req
            if (req.url == "/timeout") throw HttpError.Timeout
            if (req.url == "/crash") throw IllegalStateException("boom")
            return HttpResponse(200u.toUShort(), emptyMap(), byteArrayOf(9), 2.milliseconds)
        }
    })
    expect(!http.sync, "http is an async port")
    val method = http.methods.getValue(KeelIds.Ports.Http.REQUEST)
    val request = HttpRequest("GET", "/ok", emptyMap(), null)
    val reply = runBlocking { method(HttpRequest.encodeToByteArray(request)) }
    expectEq(seen, request, "the request reaches the implementation")
    expectEq(HttpResponse.decodeAll(reply).status, 200u.toUShort(), "response status")
    val failure = expectThrows<KeelPortException>("typed port failure") {
        runBlocking { method(HttpRequest.encodeToByteArray(request.copy(url = "/timeout"))) }
    }
    expectEq(failure.body.hex(), "0000", "encoded port error")
    expectThrows<IllegalStateException>("other failures propagate") {
        runBlocking { method(HttpRequest.encodeToByteArray(request.copy(url = "/crash"))) }
    }
    expectThrows<WireException>("malformed arguments") { runBlocking { method(byteArrayOf(1)) } }

    val stored = mutableMapOf<String, ByteArray>()
    val kv = kvPortImpl(object : Kv {
        override suspend fun get(key: String): ByteArray? = stored[key]
        override suspend fun set(key: String, value: ByteArray) {
            stored[key] = value
        }
    })
    val w = KeelWriter()
    w.writeStr("k")
    w.writeBytes(byteArrayOf(1, 2))
    expectEq(runBlocking { kv.methods.getValue(KeelIds.Ports.Kv.SET)(w.toByteArray()) }.hex(), "", "unit reply")
    expectEq(stored.getValue("k").toList(), listOf<Byte>(1, 2), "stored value")
    val key = KeelWriter()
    key.writeStr("k")
    expectEq(runBlocking { kv.methods.getValue(KeelIds.Ports.Kv.GET)(key.toByteArray()) }.hex(), "01" + "02000000" + "0102", "Some(bytes)")
    val missing = KeelWriter()
    missing.writeStr("nope")
    expectEq(runBlocking { kv.methods.getValue(KeelIds.Ports.Kv.GET)(missing.toByteArray()) }.hex(), "00", "None")

    val core = FakeCore()
    ConnectivityEvents(core).changed(true, NetKind.WIFI)
    expectEq(
        core.events.single(),
        Triple(KeelIds.Ports.Connectivity.PORT_ID, KeelIds.Ports.Connectivity.CHANGED, "010000"),
        "event",
    )
}

private fun queries() {
    val core = FakeCore()
    core.nextHandle = 3L
    val handle = TodosQueryHandle.create(5u, core)
    expectEq(core.constructed.single(), Triple(KeelIds.Queries.TODOS, KeelIds.Queries.TODOS, "05000000"), "handle constructor")
    expectEq(core.observed.single(), Triple(3L, UInt.MAX_VALUE, true), "handle observes")
    expectEq(handle.status.value, QueryStatus.IDLE, "placeholder status")
    core.deliver(3L, 1, ChangeOp.FULL, QueryStatus.encodeToByteArray(QueryStatus.SUCCESS))
    core.deliver(3L, 3, ChangeOp.FULL, Codecs.bool.encodeToByteArray(true))
    core.deliver(3L, 2, ChangeOp.FULL, Codecs.option(TodoError).encodeToByteArray(TodoError.EmptyTitle))
    expectEq(handle.status.value, QueryStatus.SUCCESS, "status")
    expectEq(handle.fetching.value, true, "fetching")
    expectEq(handle.error.value, TodoError.EmptyTitle, "error")
    expectEq(handle.updatedAt.value, null, "updated at")

    handle.refetch()
    expectEq(core.calls.last().methodId, 0x21d1b9e2u, "refetch id")
    handle.invalidate()
    expectEq(core.calls.last().methodId, 0x44cec2fau, "invalidate id")

    core.replies.add(Todo.encodeToByteArray(todo))
    expectEq(runBlocking { addTodo("Milk", core) }, todo, "mutation")
    expectEq(core.calls.last().target, CallTarget.FreeFunction(KeelIds.Queries.ADD_TODO), "mutation target")
}
