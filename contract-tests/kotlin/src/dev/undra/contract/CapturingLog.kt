package dev.undra.contract

import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.UndraReader
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicReference

/** The `Log` port that keeps every record the core emits, for S17 (panics are logged at `undra::panic`). */
class CapturingLog {
    /** One log record: level 0 trace to 5 fatal, the target, the message. */
    data class Record(val level: Int, val target: String, val message: String)

    /** Every record so far, oldest first. */
    val records = CopyOnWriteArrayList<Record>()

    private class Trigger(val matching: (Record) -> Boolean, val run: () -> Unit)

    private val trigger = AtomicReference<Trigger?>(null)

    /**
     * Arms a one-shot hook for S17.5: the next record that [matching] accepts calls [run] once, from inside the
     * Log port (on the thread the core calls it on), and the hook is cleared. Replaces an armed hook.
     */
    fun onNextRecord(matching: (Record) -> Boolean, run: () -> Unit) {
        trigger.set(Trigger(matching, run))
    }

    /** This log as a sync `Log` port. */
    fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Log.LOG] = { args ->
                val r = UndraReader(args)
                val record = Record(r.readU8().toInt(), r.readStr(), r.readStr())
                records.add(record)
                r.finish()
                // Taken out before it runs: the hook calls into the core, which logs again.
                val armed = trigger.get()
                if (armed != null && armed.matching(record) && trigger.compareAndSet(armed, null)) armed.run()
                ByteArray(0)
            }
        },
    )
}
