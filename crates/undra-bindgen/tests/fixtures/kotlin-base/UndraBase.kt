// Hand-written stand-in for the `dev.undra.runtime` base API that generated Kotlin depends on.
//
// SPEC section 17.2 lists these names, but the runtime module does not implement them yet, so the
// compile test (`tests/typecheck.rs`) builds this file together with the *real* wire layer
// (`runtimes/kotlin/undra-runtime/runtime/src/main/kotlin/dev/undra/runtime/wire`). Bodies only need
// to compile. Everything marked "Addition" is API the generated code needs that section 17.2 does
// not spell out; the bindgen report lists it for the runtime authors.

package dev.undra.runtime

import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Payloads.ReplyStatus
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow

/** Marker of generated records. */
interface UndraRecord

/** Marker of generated enums. */
interface UndraEnum

open class UndraException(message: String) : RuntimeException(message)

class UndraReplyException(val status: ReplyStatus, val body: ByteArray) :
    UndraException("undra reply with status $status")

/**
 * Addition: thrown by a generated port adapter when the implementation fails with the port's typed
 * error; [body] is the encoded error and becomes a `PortReply` with status 1.
 */
class UndraPortException(val body: ByteArray) : UndraException("typed port failure")

interface UndraPort

/**
 * Addition: what generated code registers with `UndraCore.registerPort`. `methods` maps a port
 * method id to a function from encoded arguments to the encoded reply body; a port with
 * `sync = true` must reply without suspending.
 */
class PortImpl(val sync: Boolean, val methods: Map<UInt, suspend (ByteArray) -> ByteArray>)

enum class Mode { INPROC, REMOTE }

class LoadOptions(
    val mode: Mode = Mode.INPROC,
    val remoteUrl: String? = null,
    val adapters: Map<UInt, PortImpl> = emptyMap(),
    val expectedSchemaHash: ULong,
)

class UndraStats(val liveHandles: Int)

open class Mirror {
    open fun register(handle: Long, noCoalesce: Set<UInt> = emptySet(), apply: (UInt, ChangeOp, UndraReader) -> Unit) {
        throw UnsupportedOperationException("fixture: $handle $noCoalesce $apply")
    }

    open fun unregister(handle: Long) {
        throw UnsupportedOperationException("fixture: $handle")
    }
}

/** Open so that tests can subclass it as a fake core; the real one is loaded from a native library. */
open class UndraCore protected constructor() {
    companion object {
        fun load(options: LoadOptions): UndraCore {
            throw UnsupportedOperationException("fixture: ${options.mode}")
        }

        val shared: UndraCore
            get() = throw UnsupportedOperationException("fixture")
    }

    open fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        throw UnsupportedOperationException("fixture: $target $methodId ${args.size}")
    }

    open suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        throw UnsupportedOperationException("fixture: $target $methodId ${args.size}")
    }

    open fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray> {
        throw UnsupportedOperationException("fixture: $target $methodId ${args.size}")
    }

    open fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long {
        throw UnsupportedOperationException("fixture: $typeId $methodId ${args.size}")
    }

    open fun observe(handle: Long, signalId: UInt, on: Boolean) {
        throw UnsupportedOperationException("fixture: $handle $signalId $on")
    }

    open fun release(handle: Long) {
        throw UnsupportedOperationException("fixture: $handle")
    }

    /** Addition: sends a host-to-core event of an event port (`undra_event`). */
    open fun event(portId: UInt, methodId: UInt, payload: ByteArray) {
        throw UnsupportedOperationException("fixture: $portId $methodId ${payload.size}")
    }

    open val mirror: Mirror = Mirror()

    open fun registerPort(portId: UInt, impl: PortImpl) {
        throw UnsupportedOperationException("fixture: $portId ${impl.sync}")
    }

    open fun stats(): UndraStats = UndraStats(0)
}

abstract class UndraObject(val core: UndraCore, val handle: Long) : AutoCloseable {
    override fun close() {
        core.release(handle)
    }
}

abstract class UndraStore(core: UndraCore, handle: Long, noCoalesce: Set<UInt> = emptySet()) : UndraObject(core, handle) {
    protected abstract fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader)

    protected fun <T> signal(initial: T): MutableStateFlow<T> = MutableStateFlow(initial)

    init {
        core.mirror.register(handle, noCoalesce) { signalId, op, reader -> apply(signalId, op, reader) }
    }
}
