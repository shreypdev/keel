package dev.undra.work

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import androidx.work.Configuration
import androidx.work.ListenableWorker
import androidx.work.NetworkType
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.testing.SynchronousExecutor
import androidx.work.testing.TestListenableWorkerBuilder
import androidx.work.testing.WorkManagerTestInitHelper
import dev.undra.runtime.BackgroundStats
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraStats
import dev.undra.runtime.adapters.StandardFunctions
import dev.undra.runtime.adapters.UndraBackgroundReport
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * [UndraWorker] and [UndraWork] on a device (ADR-046): the worker's `doWork` over a fake core, driven by WorkManager's own test builder, and
 * the unique, network-constrained work [UndraWork.schedule] enqueues, run by WorkManager's test driver once its constraints are met.
 */
class UndraWorkerOnDeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()

    /** A core whose `run_background` answers with what the test says, and which remembers every call and the stats it was asked for. */
    private class FakeCore(var report: UndraBackgroundReport = UndraBackgroundReport(true, 1, 0, 0), var pending: Int = 0) : UndraCore() {
        val calls = CopyOnWriteArrayList<Pair<CallTarget, ULong>>()

        override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
            calls.add(target to UndraReader(args).readU64())
            return UndraBackgroundReport.encodeToByteArray(report)
        }

        override fun stats(): UndraStats = UndraStats(0, background = BackgroundStats(3, pending, 0L, 0L, 0L, 0L))
    }

    @Before
    fun setUp() {
        UndraWork.reset()
    }

    @After
    fun tearDown() {
        UndraWork.reset()
    }

    private fun worker(): UndraWorker = TestListenableWorkerBuilder<UndraWorker>(context).build()

    private fun doWork(): ListenableWorker.Result = runBlocking { worker().doWork() }

    @Test
    fun a_run_that_finished_is_a_success_and_asked_for_nine_minutes_less_a_margin() {
        val core = FakeCore()
        UndraWork.configure { core }
        assertEquals(ListenableWorker.Result.success(), doWork())
        val (target, deadline) = core.calls.single()
        assertEquals(CallTarget.FreeFunction(StandardFunctions.RUN_BACKGROUND), target)
        // 9 minutes less the 15 s margin, less what loading took (nothing here): WorkManager stops a worker at 10.
        assertTrue("the deadline is $deadline ms", deadline <= 525_000uL && deadline > 500_000uL)
    }

    @Test
    fun a_run_that_did_not_finish_is_retried() {
        val core = FakeCore(UndraBackgroundReport(finished = false, replayed = 0, refetched = 0, stillPending = 2))
        UndraWork.configure { core }
        assertEquals(ListenableWorker.Result.retry(), doWork())
        assertEquals(1, core.calls.size)
    }

    @Test
    fun the_loader_gets_the_application_context_and_runs_once_per_run() {
        val core = FakeCore()
        val contexts = CopyOnWriteArrayList<Context>()
        UndraWork.configure {
            contexts.add(it)
            core
        }
        doWork()
        doWork()
        assertEquals(2, contexts.size)
        assertTrue(contexts.all { it === context.applicationContext })
    }

    @Test
    fun without_a_loader_the_worker_fails_for_good() {
        assertEquals(ListenableWorker.Result.failure(), doWork())
    }

    @Test
    fun a_loader_that_throws_is_retried_not_thrown_into_work_manager() {
        UndraWork.configure { throw IllegalStateException("the storage is locked until the first unlock") }
        assertEquals(ListenableWorker.Result.retry(), doWork())
    }

    @Test
    fun a_core_that_fails_the_run_is_retried() {
        val closed = object : UndraCore() {
            override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray =
                throw dev.undra.runtime.UndraTransportException(dev.undra.runtime.UndraTransportException.Reason.CLOSED, "closed")
        }
        UndraWork.configure { closed }
        assertEquals(ListenableWorker.Result.retry(), doWork())
    }

    @Test
    fun a_stopped_worker_cancels_the_run_in_the_core() {
        val started = CompletableDeferred<Unit>()
        val cancelled = CompletableDeferred<Unit>()
        UndraWork.configure {
            object : UndraCore() {
                override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
                    started.complete(Unit)
                    try {
                        awaitCancellation()
                    } finally {
                        cancelled.complete(Unit)
                    }
                }
            }
        }
        runBlocking {
            val job = launch(Dispatchers.Default) { worker().doWork() }
            started.await()
            job.cancelAndJoin() // what WorkManager does to a CoroutineWorker it stops
            cancelled.await()
        }
    }

    // ---- scheduling ----------------------------------------------------------------------------------------------

    private fun testWorkManager(): WorkManager {
        val configuration = Configuration.Builder().setExecutor(SynchronousExecutor()).setTaskExecutor(SynchronousExecutor()).build()
        WorkManagerTestInitHelper.initializeTestWorkManager(context, configuration)
        return WorkManager.getInstance(context)
    }

    private fun infos(manager: WorkManager): List<WorkInfo> = manager.getWorkInfosForUniqueWork(UndraWork.UNIQUE_WORK_NAME).get()

    @Test
    fun schedule_enqueues_one_unique_job_that_needs_a_network() {
        val manager = testWorkManager()
        UndraWork.schedule(context).result.get()
        UndraWork.schedule(context).result.get() // KEEP: a second request while one waits adds nothing
        val job = infos(manager).single()
        assertEquals(WorkInfo.State.ENQUEUED, job.state)
        assertEquals(NetworkType.CONNECTED, job.constraints.requiredNetworkType)
        UndraWork.cancel(context).result.get()
        assertEquals(WorkInfo.State.CANCELLED, infos(manager).single().state)
    }

    @Test
    fun the_scheduled_job_runs_the_core_when_the_network_is_there_and_ends_when_it_finished() {
        val manager = testWorkManager()
        val core = FakeCore()
        UndraWork.configure { core }
        UndraWork.schedule(context).result.get()
        val id = infos(manager).single().id
        assertTrue("nothing runs before the constraints hold", core.calls.isEmpty())
        WorkManagerTestInitHelper.getTestDriver(context)!!.setAllConstraintsMet(id)
        assertEquals(WorkInfo.State.SUCCEEDED, manager.getWorkInfoById(id).get()!!.state)
        assertEquals(1, core.calls.size)
        // The job is done: a later request enqueues a new one.
        UndraWork.schedule(context).result.get()
        assertEquals(WorkInfo.State.ENQUEUED, infos(manager).single { it.id != id }.state)
    }

    @Test
    fun a_job_whose_run_did_not_finish_stays_enqueued_for_a_retry() {
        val manager = testWorkManager()
        val core = FakeCore(UndraBackgroundReport(false, 0, 0, 1))
        UndraWork.configure { core }
        UndraWork.schedule(context).result.get()
        val id = infos(manager).single().id
        WorkManagerTestInitHelper.getTestDriver(context)!!.setAllConstraintsMet(id)
        val info = manager.getWorkInfoById(id).get()!!
        assertEquals(WorkInfo.State.ENQUEUED, info.state)
        assertEquals(1, info.runAttemptCount)
    }

    @Test
    fun schedule_if_pending_asks_the_core_first() {
        val manager = testWorkManager()
        val idle = FakeCore(pending = 0)
        assertFalse(UndraWork.scheduleIfPending(context, idle))
        assertTrue(infos(manager).isEmpty())
        val busy = FakeCore(pending = 2)
        assertTrue(UndraWork.scheduleIfPending(context, busy))
        assertNotNull(infos(manager).singleOrNull())
        // The shared core is the default: with none loaded it reports nothing pending.
        UndraWork.cancel(context).result.get()
        assertFalse(UndraWork.scheduleIfPending(context))
    }
}
