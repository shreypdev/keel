package dev.undra.runtime

import dev.undra.runtime.wire.Payloads.CallTarget
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow

/**
 * What [UndraCore.shared] returns while no core is loaded, and what [CoreEntry.core] (the generated
 * `Undra<Namespace>.core`) returns while its core is not (ADR-032, amendment A, decision 7): a core that was closed
 * from the start. Every call fails with [UndraTransportException] (reason `CLOSED`), which a generated call
 * reports as [UndraCallError.Unavailable]; a command's failure is only logged (there is no
 * [LoadOptions.onError] to call); [release], [timerFired] and [close] do nothing. It never becomes the shared core
 * and never reaches a native library.
 *
 * @param standsInFor the namespace of the core it stands in for (named in its messages), or `null` for [UndraCore.shared]'s.
 */
internal class UnloadedCore(private val standsInFor: String?) : UndraCore() {
    override val mode: Mode get() = Mode.INPROC

    override val namespace: String get() = standsInFor ?: UNNAMED_NAMESPACE

    private val what: String get() = if (standsInFor == null) "no Undra core is loaded" else "the Undra core `$standsInFor` is not loaded"

    private fun gone(): UndraTransportException =
        UndraTransportException(
            UndraTransportException.Reason.CLOSED,
            "$what: load it at app startup with the generated entry of its bindings (Undra<Namespace>.load()), " +
                "before creating any Undra object",
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
        UndraLog.warn("registerPort($portId) while $what is ignored; register ports on the core its load returned")
    }

    override fun snapshot(): ByteArray = throw gone()

    override fun restore(snapshot: ByteArray): Unit = throw gone()
}
