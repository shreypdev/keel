package dev.undra.android

import android.accessibilityservice.AccessibilityService
import android.app.Activity
import android.app.Application
import android.content.Context
import android.content.Intent
import android.os.Looper
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.platform.app.InstrumentationRegistry
import dev.undra.runtime.adapters.AppState
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.UndraReader
import java.util.concurrent.CopyOnWriteArrayList
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The Lifecycle adapter driven by real activities: `ActivityScenario` launches them, and Home takes the app to the background. */
class LifecycleOnDeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()

    /** An adapter whose reports are collected, with a short settling time so the tests do not wait long. */
    private class Recorded(context: Context, settleMs: Long) {
        val states = CopyOnWriteArrayList<AppState>()
        val threads = CopyOnWriteArrayList<Thread>()
        val adapter = AndroidLifecycleAdapter(context, settleMs)

        init {
            adapter.start {
                states.add(it)
                threads.add(Thread.currentThread())
            }
        }

        fun awaitLast(expected: AppState, timeoutMs: Long = 5_000): Boolean {
            val deadline = System.nanoTime() + timeoutMs * 1_000_000
            while (System.nanoTime() < deadline) {
                if (states.lastOrNull() == expected) return true
                Thread.sleep(20)
            }
            return states.lastOrNull() == expected
        }
    }

    private val instrumentation = InstrumentationRegistry.getInstrumentation()

    private fun idle() = instrumentation.waitForIdleSync()

    /** Presses Home: the launcher comes forward and the app's activity is paused and stopped. */
    private fun goHome() {
        assertTrue(instrumentation.uiAutomation.performGlobalAction(AccessibilityService.GLOBAL_ACTION_HOME))
    }

    /** Brings the app back with a new activity (the one the scenario launched stays stopped). */
    private fun comeBack(): Activity =
        instrumentation.startActivitySync(Intent(context, TestActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))

    @Test
    fun an_activity_resuming_and_the_user_going_home_and_coming_back_is_active_background_active() {
        val r = Recorded(context, settleMs = 200)
        try {
            ActivityScenario.launch(TestActivity::class.java).use {
                assertTrue("active once an activity resumes: ${r.states}", r.awaitLast(AppState.ACTIVE))

                goHome()
                assertTrue("background once the launcher is in front: ${r.states}", r.awaitLast(AppState.BACKGROUND))

                val back = comeBack()
                assertTrue("active again when the app returns: ${r.states}", r.awaitLast(AppState.ACTIVE, timeoutMs = 3_000))
                back.finish()
            }
            println("Lifecycle states: ${r.states}")
        } finally {
            r.adapter.close()
        }
    }

    @Test
    fun going_home_is_reported_as_background_once_and_never_as_inactive_on_the_way() {
        // With the default settling time: the stop follows the pause by a few hundred milliseconds (the window animation),
        // and a shorter settling time would report the paused-but-visible moment in between as Inactive.
        val r = Recorded(context, settleMs = AndroidLifecycleAdapter.DEFAULT_SETTLE_MS)
        try {
            ActivityScenario.launch(TestActivity::class.java).use {
                assertTrue(r.awaitLast(AppState.ACTIVE))
                val before = r.states.size
                goHome() // onPause then onStop, back to back
                assertTrue(r.awaitLast(AppState.BACKGROUND, timeoutMs = 10_000))
                Thread.sleep(500)
                assertEquals("the states after leaving: ${r.states}", listOf(AppState.BACKGROUND), r.states.drop(before))
                comeBack().finish()
            }
        } finally {
            r.adapter.close()
        }
    }

    @Test
    fun recreating_the_activity_is_not_a_trip_to_the_background() {
        val r = Recorded(context, settleMs = AndroidLifecycleAdapter.DEFAULT_SETTLE_MS)
        try {
            ActivityScenario.launch(TestActivity::class.java).use { scenario ->
                assertTrue(r.awaitLast(AppState.ACTIVE))
                val before = r.states.size
                scenario.recreate() // what a rotation does
                idle()
                Thread.sleep(1_500)
                assertEquals("no change of state while recreating: ${r.states}", emptyList<AppState>(), r.states.drop(before))
            }
        } finally {
            r.adapter.close()
        }
    }

    @Test
    fun reports_come_on_the_main_thread() {
        val r = Recorded(context, settleMs = 100)
        try {
            ActivityScenario.launch(TestActivity::class.java).use {
                assertTrue(r.awaitLast(AppState.ACTIVE))
                goHome()
                assertTrue(r.awaitLast(AppState.BACKGROUND))
                comeBack().finish()
            }
            assertTrue("every report on the main thread (the first is made on the caller's)", r.threads.drop(1).all { it === Looper.getMainLooper().thread })
        } finally {
            r.adapter.close()
        }
    }

    @Test
    fun attach_sends_lifecycle_changed_events_to_the_core() {
        val core = RecordingCore()
        val adapter = AndroidLifecycleAdapter(context, settleMs = 100)
        try {
            adapter.attach(core)
            ActivityScenario.launch(TestActivity::class.java).use {
                core.awaitEvents { events -> events.any { decode(it) == AppState.ACTIVE } }
                goHome()
                val events = core.awaitEvents { list -> list.lastOrNull()?.let(::decode) == AppState.BACKGROUND }
                assertTrue(events.all { it.portId == StandardPorts.Lifecycle.PORT_ID && it.methodId == StandardPorts.Lifecycle.CHANGED })
                assertEquals(AppState.BACKGROUND, decode(events.last()))
                assertTrue(events.any { decode(it) == AppState.ACTIVE })
                comeBack().finish()
            }
        } finally {
            adapter.close()
        }
    }

    @Test
    fun going_home_with_background_work_pending_asks_the_app_for_a_window_once() {
        val core = RecordingCore()
        core.backgroundPending = 2
        val asked = java.util.concurrent.atomic.AtomicInteger()
        val adapter = AndroidLifecycleAdapter(context, settleMs = 100)
        try {
            adapter.attach(core, onBackgroundWorkPending = { asked.incrementAndGet() })
            ActivityScenario.launch(TestActivity::class.java).use {
                core.awaitEvents { events -> events.any { decode(it) == AppState.ACTIVE } }
                assertEquals("the state the adapter starts in is not a move to the background", 0, asked.get())
                goHome()
                core.awaitEvents { list -> list.lastOrNull()?.let(::decode) == AppState.BACKGROUND }
                val deadline = System.nanoTime() + 5_000_000_000L
                while (asked.get() == 0 && System.nanoTime() < deadline) Thread.sleep(20)
                assertEquals("asked for a window when the app went to the background with work pending", 1, asked.get())
                comeBack().finish()
            }
        } finally {
            adapter.close()
        }
    }

    @Test
    fun going_home_with_nothing_pending_asks_for_nothing() {
        val core = RecordingCore() // backgroundPending is 0
        val asked = java.util.concurrent.atomic.AtomicInteger()
        val adapter = AndroidLifecycleAdapter(context, settleMs = 100)
        try {
            adapter.attach(core, onBackgroundWorkPending = { asked.incrementAndGet() })
            ActivityScenario.launch(TestActivity::class.java).use {
                core.awaitEvents { events -> events.any { decode(it) == AppState.ACTIVE } }
                goHome()
                core.awaitEvents { list -> list.lastOrNull()?.let(::decode) == AppState.BACKGROUND }
                Thread.sleep(300)
                assertEquals(0, asked.get())
                comeBack().finish()
            }
        } finally {
            adapter.close()
        }
    }

    @Test
    fun close_unregisters_the_callbacks_and_stops_reports() {
        val r = Recorded(context, settleMs = 100)
        r.adapter.close()
        r.adapter.close() // idempotent
        val before = r.states.size
        ActivityScenario.launch(TestActivity::class.java).use {
            Thread.sleep(500)
        }
        assertEquals("nothing reported after close", before, r.states.size)
    }

    @Test
    fun the_state_before_start_is_what_the_process_importance_says() {
        val adapter = AndroidLifecycleAdapter(context)
        assertTrue(adapter.state == AppState.ACTIVE || adapter.state == AppState.BACKGROUND)
        assertFalse(adapter.state == AppState.INACTIVE)
        assertTrue(context.applicationContext is Application)
    }

    private fun decode(event: RecordingCore.Event): AppState {
        val reader = UndraReader(event.payload)
        val state = AppState.decode(reader)
        reader.finish()
        return state
    }
}
