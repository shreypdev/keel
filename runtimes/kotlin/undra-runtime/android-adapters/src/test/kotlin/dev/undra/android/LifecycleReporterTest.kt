package dev.undra.android

import dev.undra.runtime.adapters.AppState
import org.junit.Assert.assertEquals
import org.junit.Test

/** What the Lifecycle adapter does with each state it reports: the event to the core, and asking for a background window (ADR-046). */
class LifecycleReporterTest {
    private val sent = ArrayList<AppState>()
    private var pending = true
    private var asked = 0

    private fun reporter(callback: Boolean = true, send: (AppState) -> Unit = { sent.add(it) }) =
        LifecycleReporter(send, { pending }, if (callback) ({ asked++ }) else null)

    @Test
    fun every_state_is_sent_to_the_core_in_order() {
        val r = reporter()
        listOf(AppState.ACTIVE, AppState.INACTIVE, AppState.BACKGROUND, AppState.ACTIVE).forEach(r::changed)
        assertEquals(listOf(AppState.ACTIVE, AppState.INACTIVE, AppState.BACKGROUND, AppState.ACTIVE), sent)
    }

    @Test
    fun a_move_to_the_background_with_work_pending_asks_for_a_window_once() {
        val r = reporter()
        r.changed(AppState.ACTIVE)
        r.changed(AppState.BACKGROUND)
        assertEquals(1, asked)
        r.changed(AppState.ACTIVE)
        r.changed(AppState.INACTIVE)
        assertEquals(1, asked) // only the background asks
        r.changed(AppState.BACKGROUND)
        assertEquals(2, asked)
    }

    @Test
    fun nothing_pending_asks_for_nothing() {
        pending = false
        val r = reporter()
        r.changed(AppState.ACTIVE)
        r.changed(AppState.BACKGROUND)
        assertEquals(0, asked)
        assertEquals(listOf(AppState.ACTIVE, AppState.BACKGROUND), sent)
    }

    @Test
    fun the_state_the_adapter_starts_in_is_not_a_move_to_the_background() {
        // A process WorkManager started in the background reports Background first: that is where it is, not where it went.
        val r = reporter()
        r.changed(AppState.BACKGROUND)
        assertEquals(0, asked)
        r.changed(AppState.ACTIVE)
        r.changed(AppState.BACKGROUND)
        assertEquals(1, asked)
    }

    @Test
    fun without_a_callback_the_core_is_not_even_asked() {
        var asks = 0
        val r = LifecycleReporter({ sent.add(it) }, { asks++; true }, null)
        r.changed(AppState.ACTIVE)
        r.changed(AppState.BACKGROUND)
        assertEquals(0, asks)
    }

    @Test
    fun a_failing_send_or_callback_or_stats_read_never_escapes() {
        var failingAsks = 0
        val r = LifecycleReporter({ throw IllegalStateException("the core is closed") }, { true }, { failingAsks++; throw IllegalStateException("no WorkManager") })
        r.changed(AppState.ACTIVE)
        r.changed(AppState.BACKGROUND)
        assertEquals(1, failingAsks) // the send failed, and the app was still offered the work
        val unreadable = LifecycleReporter({ }, { throw IllegalStateException("stats failed") }, { failingAsks++ })
        unreadable.changed(AppState.ACTIVE)
        unreadable.changed(AppState.BACKGROUND)
        assertEquals(1, failingAsks)
    }
}
