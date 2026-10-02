package dev.undra.playground

import dev.undra.playground.core.explode
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.adapters.UndraPanicReport
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update

/**
 * The panics of the core this process has seen (ADR-046), for the Debug section of the Remote tab: the last report and how many
 * there were. [UndraApp] feeds it from `LoadOptions.onPanic`; an app would also hand each report to its crash reporter there.
 */
object PanicLog {
    private val _last = MutableStateFlow<UndraPanicReport?>(null)
    private val _count = MutableStateFlow(0)

    /** The report of the most recent panic, or `null` while the core has not panicked. */
    val last: StateFlow<UndraPanicReport?> get() = _last

    /** How many panics have been reported. */
    val count: StateFlow<Int> get() = _count

    /** Records [report] (on the main thread, from `onPanic`). */
    fun record(report: UndraPanicReport) {
        _last.value = report
        _count.update { it + 1 }
    }

    /** The report as one line: what was running, what it said and where. */
    fun describe(report: UndraPanicReport): String = report.summary

    /**
     * Makes the core panic inside a call: the call fails with `UndraCallError.Panicked`, which is what is caught here, the core keeps
     * working, and `onPanic` receives the report. The playground's `explode` exists for this.
     */
    fun trigger() {
        try {
            explode("from the Debug section")
        } catch (e: UndraCallError.Panicked) {
            // Expected: the panic was contained and the report arrives through onPanic.
        }
    }
}
