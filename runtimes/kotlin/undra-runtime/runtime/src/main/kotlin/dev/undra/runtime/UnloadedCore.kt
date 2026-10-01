package dev.undra.runtime

import dev.undra.runtime.wire.Payloads.CallTarget
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow

/**
 * What [UndraCore.shared] returns while no core is loaded (ADR-032, amendment A, decision 7): a core that was closed
 * from the start. Every call fails with [UndraTransportException] (reason `CLOSED`), which a generated call
 * reports as [UndraCallError.Unavailable]; a command's failure is only logged (there is no
 * [LoadOptions.onError] to call); [release], [timerFired] and [close] do nothing. It never becomes the shared core
 * and never reaches the native library.
 */
internal class UnloadedCore : UndraCore() {
    override val mode: Mode get() = Mode.INPROC

    private fun gone(): UndraTransportException =
        UndraTransportException(
            UndraTransportException.Reason.CLOSED,
            "no UndraCore is loaded: call UndraCore.load(LoadOptions(...)) at app startup, before creating any Undra object",
        )

    override fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray = throw gone()

    override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray = throw gone()

    override fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray> = flow { throw gone() }

    override fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long = throw gone()

    override fun observe(handle: Long, signalId: UInt, on: Boolean): Unit = throw gone()

    override fun release(handle: Long) = Unit

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray): Unit = throw gone()

    override fun timerFired(timerId: UInt) = Unit

    override fun registerPort(portId: UInt, impl: PortImpl) {
        UndraLog.warn("registerPort($portId) on UndraCore.shared while no core is loaded is ignored; register ports on the core UndraCore.load(...) returned")
    }

    override fun snapshot(): ByteArray = throw gone()

    override fun restore(snapshot: ByteArray): Unit = throw gone()
}
