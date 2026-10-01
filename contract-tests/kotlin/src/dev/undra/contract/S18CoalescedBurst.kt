package dev.undra.contract

import dev.undra.playground.core.Stress
import dev.undra.playground.core.StressMode
import dev.undra.playground.core.UndraIds
import dev.undra.runtime.UndraDispatchers
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.decodeAll
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import kotlin.coroutines.EmptyCoroutineContext

/**
 * S18: the core commits one change-set per transaction, and the mirror merges what arrived before it
 * drains (ADR-031). The calls are made on the main thread, where a synchronous call drains the mirror
 * before it returns, as a UI's would be.
 */
fun s18CoalescedBurst(w: World) {
    val ids = UndraIds.Objects.Stress

    // 1. Raw: burst(FIREHOSE, 1000) reaches the raw mirror callback once, with the final value.
    val raw = RawStore(w.core, ids.TYPE_ID, ids.NEW)
    raw.observe()
    val mark = raw.mark()
    val before = w.stats()
    val mirrorBefore = w.core.mirror.stats()
    val seenOnReturn = onMain {
        raw.callSync(ids.BURST, burstArgs(StressMode.FIREHOSE, 1000u))
        raw.entries.size - mark
    }
    expectEq("raw mirror callbacks when burst(FIREHOSE, 1000) returned", 1, seenOnReturn)
    val entries = raw.since(mark)
    expectEq("entries delivered by the burst", 1, entries.size)
    expectEq("the entry's signal", 0u, entries[0].signalId)
    expectEq("the entry's op", ChangeOp.FULL, entries[0].op)
    expectEq("the final value", 1000uL, Codecs.u64.decodeAll(entries[0].value))
    expectEq("transactions grown by the burst", 1000L, w.stats().transactions - before.transactions)
    val mirrorAfter = w.core.mirror.stats()
    expectEq("change-sets the mirror received", 1000L, mirrorAfter.changeSetsReceived - mirrorBefore.changeSetsReceived)
    expectEq("entries the mirror applied", 1L, mirrorAfter.entriesApplied - mirrorBefore.entriesApplied)
    raw.close()

    // 2. Generated: the final value is there when burst returns (read-your-writes).
    val value = onMain {
        Stress(w.core).use { stress ->
            stress.burst(StressMode.FIREHOSE, 1000u)
            stress.value.value
        }
    }
    expectEq("value right after burst(FIREHOSE, 1000)", 1000uL, value)

    // 3. Generated, no_coalesce: every progress entry is applied (the StateFlow conflates what it shows).
    val (progress, applied) = onMain {
        Stress(w.core).use { stress ->
            val appliedBefore = w.core.mirror.stats().entriesApplied
            stress.burst(StressMode.PROGRESS, 10u)
            stress.progress.value to (w.core.mirror.stats().entriesApplied - appliedBefore)
        }
    }
    expectEq("progress right after burst(PROGRESS, 10)", 10u, progress)
    expectEq("progress entries the mirror applied", 10L, applied)

    // Kotlin only: the waits of this runner drain the mirror before each look (NOTES.md), so this checks the
    // frame path itself. A burst made off the main thread, where nothing drains for the caller, reaches the
    // store at a frame of the runtime's own pacer, with no drain from the test.
    val offMain = onMain { Stress(w.core) }
    try {
        offMain.burst(StressMode.FIREHOSE, 1000u)
        val deadline = System.nanoTime() + WAIT_MS * 1_000_000L
        while (offMain.value.value != 1000uL) {
            if (System.nanoTime() > deadline) fail("burst(FIREHOSE, 1000) made off the main thread never reached the store at a frame")
            Thread.sleep(2L)
        }
    } finally {
        onMain { offMain.close() }
    }
}

private fun burstArgs(mode: StressMode, transactions: UInt): ByteArray {
    val w = UndraWriter()
    StressMode.encode(w, mode)
    w.writeU32(transactions)
    return w.toByteArray()
}

/** Runs [block] on the main thread ([UndraDispatchers.main]) and returns what it returned, within [timeoutMs]. */
internal fun <T> onMain(timeoutMs: Long = WAIT_MS, block: () -> T): T {
    val result = CompletableFuture<T>()
    UndraDispatchers.main.dispatch(
        EmptyCoroutineContext,
        Runnable {
            try {
                result.complete(block())
            } catch (e: Throwable) {
                result.completeExceptionally(e)
            }
        },
    )
    try {
        return result.get(timeoutMs, TimeUnit.MILLISECONDS)
    } catch (e: ExecutionException) {
        throw e.cause ?: e
    } catch (e: java.util.concurrent.TimeoutException) {
        fail("the main thread did not run the scenario's block within $timeoutMs ms")
    }
}
