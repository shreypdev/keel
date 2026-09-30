package dev.keel.contract

import dev.keel.runtime.KeelCore
import dev.keel.runtime.KeelDispatchers
import dev.keel.runtime.wire.Handle
import dev.keel.runtime.wire.Payloads.CallTarget
import dev.keel.runtime.wire.Payloads.ChangeOp
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import kotlin.coroutines.EmptyCoroutineContext
import kotlinx.coroutines.runBlocking

/**
 * A store driven through `KeelCore` alone, without a generated class: `core.construct`, a mirror
 * callback for its handle and `core.observe(handle, ALL_SIGNALS, on)` (scenarios.md, "Signals").
 * It records every entry the mirror delivers, in order, so a scenario can look at signal ids, ops and
 * the raw value bytes that a generated store would have decoded and thrown away.
 */
class RawStore(val core: KeelCore, typeId: UInt, constructorId: UInt, args: ByteArray = ByteArray(0)) : AutoCloseable {
    /** One entry of a change-set as the mirror delivered it. */
    class Entry(val signalId: UInt, val op: ChangeOp, val value: ByteArray) {
        override fun toString(): String = "Entry(signal=$signalId, op=$op, ${value.size} bytes)"
    }

    /** The object's handle in the core. */
    val handle: Long = core.construct(typeId, constructorId, args)

    /** Every entry delivered so far, oldest first. */
    val entries = CopyOnWriteArrayList<Entry>()

    init {
        core.mirror.register(handle) { signalId, op, reader -> entries.add(Entry(signalId, op, reader.readRemaining())) }
    }

    /** Starts (or, with `false`, stops) observing every signal; starting returns once the values were delivered. */
    fun observe(on: Boolean = true) {
        core.observe(handle, ALL_SIGNALS, on)
    }

    /**
     * A position in [entries], to ask later for what arrived since. Everything the core delivered before this
     * call has been applied first ([flushMainThread]), so a change-set an earlier call produced cannot turn up
     * after the mark.
     */
    fun mark(): Int {
        flushMainThread()
        return entries.size
    }

    /** The entries that arrived after [mark]. */
    fun since(mark: Int): List<Entry> = entries.drop(mark)

    /** Calls the synchronous method [methodId] of this object. */
    fun callSync(methodId: UInt, args: ByteArray = ByteArray(0)): ByteArray =
        core.callSync(CallTarget.ObjectMethod(Handle(handle), methodId), methodId, args)

    /** Calls the method [methodId] of this object through the asynchronous path and waits for the reply. */
    fun call(methodId: UInt, args: ByteArray = ByteArray(0)): ByteArray =
        runBlocking { core.call(CallTarget.ObjectMethod(Handle(handle), methodId), methodId, args) }

    /** Releases the object and stops the mirror routing to it. */
    override fun close() {
        core.release(handle)
    }

    /** Protocol constants. */
    companion object {
        /** The signal id that means every signal of a store (`UInt.MAX_VALUE`). */
        const val ALL_SIGNALS: UInt = UInt.MAX_VALUE
    }
}

/**
 * Waits until the main thread ([KeelDispatchers.main], where the mirror applies change-sets) has run
 * everything queued before this call. A change-set a synchronous call produced has reached the
 * mirror's queue by the time the call returns, so after this it has been applied.
 */
fun flushMainThread() {
    val done = CompletableFuture<Unit>()
    KeelDispatchers.main.dispatch(EmptyCoroutineContext, Runnable { done.complete(Unit) })
    try {
        done.get(WAIT_MS, TimeUnit.MILLISECONDS)
    } catch (e: java.util.concurrent.TimeoutException) {
        fail("the main thread did not run a queued task within $WAIT_MS ms")
    }
}
