// A core that records what generated code asks of it, for the execution tests of the cases that
// cannot reach a real one (`tests/typecheck_kotlin.rs` compiles this file with them).

package golden.support

import dev.undra.runtime.Mirror
import dev.undra.runtime.UndraCore
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.UndraReader
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow

fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }

fun expect(condition: Boolean, message: String) {
    if (!condition) throw AssertionError(message)
}

fun <T> expectEq(actual: T, expected: T, message: String) {
    if (actual != expected) throw AssertionError("$message: expected <$expected>, got <$actual>")
}

/** What the fake core saw of one call. */
data class Call(val target: CallTarget, val methodId: UInt, val args: String)

private class FakeMirror : Mirror() {
    val callbacks = mutableMapOf<Long, (UInt, ChangeOp, UndraReader) -> Unit>()

    override fun register(handle: Long, apply: (UInt, ChangeOp, UndraReader) -> Unit) {
        callbacks[handle] = apply
    }

    override fun unregister(handle: Long) {
        callbacks.remove(handle)
    }
}

class FakeCore : UndraCore() {
    val calls = mutableListOf<Call>()
    val constructed = mutableListOf<Triple<UInt, UInt, String>>()
    val observed = mutableListOf<Triple<Long, UInt, Boolean>>()
    /** What generated commands and `apply` reported through `report` (operation, failure). */
    val reports = mutableListOf<Pair<String, Throwable>>()
    var nextHandle = 7L
    private val fakeMirror = FakeMirror()
    override val mirror: Mirror get() = fakeMirror

    override fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        calls += Call(target, methodId, args.hex())
        return ByteArray(0)
    }

    override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        calls += Call(target, methodId, args.hex())
        return ByteArray(0)
    }

    override fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray> {
        calls += Call(target, methodId, args.hex())
        return flow { }
    }

    override fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long {
        constructed += Triple(typeId, methodId, args.hex())
        return nextHandle++
    }

    override fun observe(handle: Long, signalId: UInt, on: Boolean) {
        observed += Triple(handle, signalId, on)
    }

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) = Unit

    override fun release(handle: Long) = Unit

    override fun report(error: Throwable, operation: String) {
        reports += operation to error
    }

    /** Delivers a change-set entry the way the mirror would. */
    fun deliver(handle: Long, signalId: Int, op: ChangeOp, value: ByteArray) {
        fakeMirror.callbacks.getValue(handle)(signalId.toUInt(), op, UndraReader(value))
    }
}
