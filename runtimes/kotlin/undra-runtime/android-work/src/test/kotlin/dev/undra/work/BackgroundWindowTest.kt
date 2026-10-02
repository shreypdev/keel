package dev.undra.work

import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.UndraStats
import dev.undra.runtime.BackgroundStats
import dev.undra.runtime.adapters.StandardFunctions
import dev.undra.runtime.adapters.UndraBackgroundReport
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** What the worker does with its window (the pure logic behind [UndraWorker]): the deadline it asks for and how the core's answer maps onto a result. */
class BackgroundWindowTest {
    /** A core whose `run_background` answers with [answer] and remembers what it was asked. */
    private class FakeCore(var answer: () -> ByteArray) : UndraCore() {
        val targets = CopyOnWriteArrayList<CallTarget>()
        val deadlines = CopyOnWriteArrayList<ULong>()

        override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
            targets.add(target)
            deadlines.add(UndraReader(args).readU64())
            return answer()
        }
    }

    private fun report(finished: Boolean, still: Int = 0) = UndraBackgroundReport.encodeToByteArray(UndraBackgroundReport(finished, 1, 0, still))

    @Test
    fun the_deadline_is_nine_minutes_less_a_margin_less_what_was_spent() {
        assertEquals(525_000L, BackgroundWindow.deadlineMs(0))
        assertEquals(515_000L, BackgroundWindow.deadlineMs(10_000))
        assertEquals(9 * 60_000L - 15_000L, BackgroundWindow.deadlineMs(0))
        // WorkManager stops a worker at ten minutes: the core always has the last minute and a quarter to spare.
        assertTrue(BackgroundWindow.deadlineMs(0) + 60_000L < BackgroundWindow.WORKER_LIMIT_MS)
    }

    @Test
    fun a_window_that_loading_the_core_used_up_still_gets_a_short_run() {
        assertEquals(BackgroundWindow.MIN_DEADLINE_MS, BackgroundWindow.deadlineMs(9 * 60_000L))
        assertEquals(BackgroundWindow.MIN_DEADLINE_MS, BackgroundWindow.deadlineMs(Long.MAX_VALUE))
        assertEquals(525_000L, BackgroundWindow.deadlineMs(-5L)) // a clock that stepped back spends nothing
        assertEquals(35_000L, BackgroundWindow.deadlineMs(elapsedMs = 5_000, windowMs = 60_000, marginMs = 20_000))
    }

    @Test
    fun a_finished_run_is_a_success_and_asks_run_background_for_the_deadline() {
        val core = FakeCore { report(finished = true) }
        assertEquals(WindowOutcome.SUCCESS, runBlocking { BackgroundWindow.run(core, 525_000L) })
        assertEquals(listOf<CallTarget>(CallTarget.FreeFunction(StandardFunctions.RUN_BACKGROUND)), core.targets.toList())
        assertEquals(listOf(525_000uL), core.deadlines.toList())
    }

    @Test
    fun a_run_that_did_not_finish_is_retried() {
        val core = FakeCore { report(finished = false, still = 3) }
        assertEquals(WindowOutcome.RETRY, runBlocking { BackgroundWindow.run(core, 1_000L) })
    }

    @Test
    fun a_failed_run_is_retried_and_never_throws() {
        val failures = listOf(
            { throw UndraReplyException(ReplyStatus.CANCELLED, ByteArray(0)) }, // the core cancelled it
            { throw dev.undra.runtime.UndraTransportException(dev.undra.runtime.UndraTransportException.Reason.CLOSED, "closed") },
            { byteArrayOf(1, 2, 3) }, // a body that is not a BackgroundReport
            { throw IllegalStateException("a bug in a test double") },
        )
        for (failure in failures) {
            val core = FakeCore { failure() }
            assertEquals(WindowOutcome.RETRY, runBlocking { BackgroundWindow.run(core, 1_000L) })
        }
        // The first two arrive as UndraCallError from runInBackground (what the worker logs).
        val closed = FakeCore { throw dev.undra.runtime.UndraTransportException(dev.undra.runtime.UndraTransportException.Reason.CLOSED, "closed") }
        try {
            runBlocking { closed.runInBackground(1L) }
            fail("a closed core must fail")
        } catch (e: UndraCallError.Unavailable) {
            assertEquals(dev.undra.runtime.UndraTransportException.Reason.CLOSED, e.transport.reason)
        }
    }

    @Test
    fun a_stopped_worker_cancels_the_call_and_the_cancellation_propagates() {
        val started = CompletableDeferred<Unit>()
        val sawCancel = AtomicBoolean(false)
        val core = object : UndraCore() {
            override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
                started.complete(Unit)
                try {
                    awaitCancellation()
                } catch (e: CancellationException) {
                    sawCancel.set(true)
                    throw e
                }
            }
        }
        var outcome: WindowOutcome? = null
        var thrown: Throwable? = null
        runBlocking {
            val job = launch(Dispatchers.Default) {
                try {
                    outcome = BackgroundWindow.run(core, 30_000L)
                } catch (e: Throwable) {
                    thrown = e
                    throw e
                }
            }
            started.await()
            job.cancelAndJoin()
        }
        assertTrue("the call saw the cancellation", sawCancel.get())
        assertTrue("a cancellation is not turned into a result: $thrown", thrown is CancellationException)
        assertNull(outcome)
    }

    @Test
    fun the_loader_is_what_configure_registered() {
        UndraWork.reset()
        assertNull(UndraWork.loader)
        val core = FakeCore { report(true) }
        UndraWork.configure { core }
        assertTrue(UndraWork.loader!!.invoke(contextStub) === core)
        UndraWork.reset()
        assertNull(UndraWork.loader)
    }

    @Test
    fun scheduling_asks_the_core_whether_there_is_work_first() {
        // scheduleIfPending reads stats().background; the decision is on the stats alone.
        fun coreWith(pending: Int) = object : UndraCore() {
            override fun stats(): UndraStats = UndraStats(0, background = BackgroundStats(3, pending, 0L, 0L, 0L, 0L))
        }
        assertTrue(coreWith(2).stats().background.hasPendingWork)
        assertFalse(coreWith(0).stats().background.hasPendingWork)
        assertFalse(UndraStats(0).background.hasPendingWork) // a core that does not say: nothing to ask the OS for
    }

    /** The loader test never uses the context it passes; the JVM unit tests have no Android context, so a stand-in. */
    private val contextStub: android.content.Context get() = android.content.ContextWrapper(null)
}
