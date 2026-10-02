package dev.undra.testkit

import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraSchemaMismatchException
import dev.undra.runtime.wire.Fnv
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.testkit.testing.Suite
import dev.undra.testkit.testing.assertEq
import dev.undra.testkit.testing.assertThrows
import dev.undra.testkit.testing.assertTrue
import dev.undra.testkit.testing.fixture
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private val SESSION = fixture("fixtures/session-todos.json")
private val HASH = Recording.fromJson(SESSION).schemaHash
private val TODOS_TYPE = Fnv.fnv1a32("Todos")
private val TODOS_NEW = Fnv.fnv1a32("Todos.new")
private val TODOS_ADD = Fnv.fnv1a32("Todos.add")

/** Opens the Todos store of the recording the way a generated store does: construct, register with the mirror, observe every signal. */
private class RawTodos(val core: UndraCore) {
    val handle: Long = core.construct(TODOS_TYPE, TODOS_NEW, ByteArray(0))
    val remaining = CopyOnWriteArrayList<UInt>()
    val todosSignalCount = CopyOnWriteArrayList<UInt>()

    init {
        core.mirror.register(handle) { signal, op, reader: UndraReader ->
            if (signal == 3u && op == dev.undra.runtime.wire.Payloads.ChangeOp.FULL) remaining += reader.readU32()
            if (signal == 0u && op == dev.undra.runtime.wire.Payloads.ChangeOp.FULL) todosSignalCount += reader.readU32()
        }
        core.observe(handle, UInt.MAX_VALUE, true)
    }

    val lastRemaining: UInt get() = remaining.last()
}

private fun load(options: ReplayOptions = ReplayOptions(), hash: ULong = HASH): RecordedCore =
    RecordedCore.load(Recording.fromJson(SESSION), hash, options, makeShared = false)

class RecordedCoreTests : Suite() {
    init {
        case("shows the state the recording first saw, then follows the playhead") {
            val recorded = load()
            try {
                assertEq(100L, recorded.playhead)
                assertEq(500L, recorded.durationMs)
                val todos = RawTodos(recorded.core)
                recorded.advance(0) // let the first drain land
                assertEq(0u, todos.lastRemaining)
                recorded.advance(100)
                assertEq(1u, todos.lastRemaining)
                recorded.advance(200)
                assertEq(3u, todos.lastRemaining)
                recorded.playAll()
                assertEq(2u, todos.lastRemaining)
                assertEq(500L, recorded.playhead)
            } finally {
                recorded.close()
            }
        }
        case("starts anywhere: a store observed late is brought up to the playhead") {
            val recorded = load(ReplayOptions(startAtMs = 400))
            try {
                val todos = RawTodos(recorded.core)
                recorded.advance(0)
                assertEq(3u, todos.lastRemaining)
            } finally {
                recorded.close()
            }
        }
        case("a call it has no recording for is refused with a failure that names it") {
            val recorded = load()
            try {
                RawTodos(recorded.core)
                // The recording holds one constructor reply; a second one has none left.
                val failure = assertThrows<Exception> { recorded.core.construct(TODOS_TYPE, TODOS_NEW, ByteArray(0)) }
                assertTrue(failure.message!!.contains("no reply for constructor"), failure.message ?: "")
            } finally {
                recorded.close()
            }
        }
        case("a method call is answered from the recorded reply, whatever the arguments, in order") {
            val recorded = load()
            try {
                val todos = RawTodos(recorded.core)
                val target = CallTarget.ObjectMethod(Handle(todos.handle), TODOS_ADD)
                val args = UndraWriter().also { it.writeStr("anything at all") }.toByteArray()
                val first = runBlocking { recorded.core.call(target, TODOS_ADD, args) }
                assertTrue(first.toHex().endsWith("427579206d696c6b00"), first.toHex()) // "Buy milk", not done
                val second = runBlocking { recorded.core.call(target, TODOS_ADD, args) }
                assertTrue(second.toHex().contains("57616c6b2074686520646f67"), second.toHex()) // "Walk the dog"
            } finally {
                recorded.close()
            }
        }
        case("can repeat the last reply instead of refusing") {
            val recorded = load(ReplayOptions(exhausted = Exhausted.REPEAT_LAST))
            try {
                val todos = RawTodos(recorded.core)
                val target = CallTarget.ObjectMethod(Handle(todos.handle), TODOS_ADD)
                val bodies = (1..5).map { runBlocking { recorded.core.call(target, TODOS_ADD, ByteArray(0)) }.toHex() }
                assertEq(bodies[2], bodies[3])
                assertEq(bodies[2], bodies[4])
            } finally {
                recorded.close()
            }
        }
        case("a recording of another schema is refused") {
            val error = assertThrows<UndraSchemaMismatchException> { load(hash = 1uL) }
            assertTrue(error.message!!.contains("0x1 ") && error.message!!.contains("0x"), "the message carries both hashes: ${error.message}")
        }
        case("replays a stream's items with the call id of the live call") {
            val recording = Recording(
                HASH, "hand", null,
                listOf(
                    RecordedEvent(0, RecordedKind.Call(RecordedTarget.Function(5u), 9u, ByteArray(0))),
                    RecordedEvent(0, RecordedKind.Reply(9u, ReplyStatusName.STREAM_OPENED, ByteArray(0))),
                    RecordedEvent(1, RecordedKind.StreamItem(9u, StreamFlagName.ITEM, byteArrayOf(1, 0, 0, 0))),
                    RecordedEvent(2, RecordedKind.StreamItem(9u, StreamFlagName.ITEM, byteArrayOf(2, 0, 0, 0))),
                    RecordedEvent(3, RecordedKind.StreamItem(9u, StreamFlagName.END, ByteArray(0))),
                ),
            )
            val recorded = RecordedCore.load(recording, HASH, makeShared = false)
            try {
                val seen = runBlocking { recorded.core.stream(CallTarget.FreeFunction(5u), 5u, ByteArray(0)).toList_() }
                assertEq(listOf(1, 2), seen.map { java.nio.ByteBuffer.wrap(it).order(java.nio.ByteOrder.LITTLE_ENDIAN).int })
            } finally {
                recorded.close()
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}

private suspend fun kotlinx.coroutines.flow.Flow<ByteArray>.toList_(): List<ByteArray> {
    val out = ArrayList<ByteArray>()
    collect { out += it }
    return out
}
