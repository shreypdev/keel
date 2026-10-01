package dev.undra.android

import dev.undra.runtime.adapters.AppState
import org.junit.Assert.assertEquals
import org.junit.Test

/** The foreground and background logic of the Lifecycle adapter, driven by hand with a manual clock. */
class ProcessStateMachineTest {
    /** A scheduler whose time passes only when told to. */
    private class ManualScheduler {
        class Task(val at: Long, val run: Runnable, var cancelled: Boolean = false)

        var now = 0L
        private val tasks = ArrayList<Task>()

        fun schedule(delay: Long, run: Runnable): ProcessStateMachine.Cancel {
            val task = Task(now + delay, run)
            tasks.add(task)
            return ProcessStateMachine.Cancel { task.cancelled = true }
        }

        fun advance(ms: Long) {
            val until = now + ms
            while (true) {
                val next = tasks.filter { !it.cancelled && it.at <= until }.minByOrNull { it.at } ?: break
                tasks.remove(next)
                now = next.at
                next.run.run()
            }
            now = until
        }
    }

    private val clock = ManualScheduler()
    private val seen = ArrayList<AppState>()
    private fun machine(initial: AppState = AppState.BACKGROUND, settle: Long = 700) =
        ProcessStateMachine(settle, clock::schedule, initial) { seen.add(it) }

    @Test
    fun the_first_resumed_activity_makes_the_app_active_at_once() {
        val m = machine()
        m.onStarted()
        assertEquals(emptyList<AppState>(), seen) // started but not resumed is not in the foreground yet... after the settle
        m.onResumed()
        assertEquals(listOf(AppState.ACTIVE), seen)
        assertEquals(AppState.ACTIVE, m.state)
    }

    @Test
    fun leaving_the_app_is_reported_as_background_once_after_the_settling_time() {
        val m = machine(AppState.ACTIVE)
        m.onStarted(); m.onResumed()
        m.onPaused()
        m.onStopped()
        clock.advance(699)
        assertEquals(emptyList<AppState>(), seen)
        clock.advance(1)
        assertEquals(listOf(AppState.BACKGROUND), seen) // never INACTIVE on the way: the stop followed the pause
    }

    @Test
    fun going_from_one_activity_to_another_is_not_a_trip_to_the_background() {
        val m = machine(AppState.ACTIVE)
        m.onStarted(); m.onResumed()
        m.onPaused() // A pauses ...
        m.onStarted() // ... B starts and resumes before A stops
        m.onResumed()
        m.onStopped()
        clock.advance(5_000)
        assertEquals(emptyList<AppState>(), seen)
        assertEquals(AppState.ACTIVE, m.state)
    }

    @Test
    fun a_rotation_is_not_a_trip_to_the_background() {
        val m = machine(AppState.ACTIVE)
        m.onStarted(); m.onResumed()
        m.onPaused(); m.onStopped() // the old activity goes ...
        clock.advance(100)
        m.onStarted(); m.onResumed() // ... and its replacement comes up within the settling time
        clock.advance(5_000)
        assertEquals(emptyList<AppState>(), seen)
    }

    @Test
    fun a_pause_that_lasts_is_inactive_and_resuming_is_active_again_at_once() {
        val m = machine(AppState.ACTIVE)
        m.onStarted(); m.onResumed()
        m.onPaused() // a permission dialog takes the focus; the activity stays started
        clock.advance(700)
        assertEquals(listOf(AppState.INACTIVE), seen)
        m.onResumed()
        assertEquals(listOf(AppState.INACTIVE, AppState.ACTIVE), seen)
    }

    @Test
    fun an_inactive_app_that_is_then_stopped_reports_background_after_another_settling_time() {
        val m = machine(AppState.ACTIVE)
        m.onStarted(); m.onResumed()
        m.onPaused()
        clock.advance(700)
        m.onStopped()
        clock.advance(699)
        assertEquals(listOf(AppState.INACTIVE), seen)
        clock.advance(1)
        assertEquals(listOf(AppState.INACTIVE, AppState.BACKGROUND), seen)
    }

    @Test
    fun the_state_is_reported_only_when_it_changes() {
        val m = machine(AppState.ACTIVE)
        m.onStarted(); m.onResumed() // already active
        m.onStarted(); m.onResumed() // a second window
        assertEquals(emptyList<AppState>(), seen)
        m.onPaused() // one of two windows loses the focus: still active
        clock.advance(5_000)
        assertEquals(emptyList<AppState>(), seen)
    }

    @Test
    fun a_process_that_starts_in_the_background_stays_quiet_until_an_activity_resumes() {
        val m = machine(AppState.BACKGROUND)
        clock.advance(5_000)
        assertEquals(emptyList<AppState>(), seen)
        m.onStarted(); m.onResumed()
        assertEquals(listOf(AppState.ACTIVE), seen)
    }

    @Test
    fun unbalanced_callbacks_never_drive_the_counts_negative() {
        val m = machine(AppState.ACTIVE)
        m.onPaused(); m.onStopped(); m.onStopped()
        clock.advance(700)
        assertEquals(listOf(AppState.BACKGROUND), seen)
        m.onStarted(); m.onResumed()
        assertEquals(listOf(AppState.BACKGROUND, AppState.ACTIVE), seen)
    }

    @Test
    fun a_disposed_machine_reports_nothing_more() {
        val m = machine(AppState.ACTIVE)
        m.onStarted(); m.onResumed()
        m.onPaused(); m.onStopped()
        m.dispose()
        clock.advance(5_000)
        m.onStarted(); m.onResumed()
        assertEquals(emptyList<AppState>(), seen)
    }

    @Test
    fun with_no_settling_time_every_change_is_reported_when_the_timer_runs() {
        val m = machine(AppState.ACTIVE, settle = 0)
        m.onStarted(); m.onResumed(); m.onPaused()
        clock.advance(0)
        assertEquals(listOf(AppState.INACTIVE), seen)
    }
}
