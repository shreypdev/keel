package dev.undra.contract

import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.UndraReader
import java.util.concurrent.CopyOnWriteArrayList

/** The `Log` port that keeps every record the core emits, for S17 (panics are logged at `undra::panic`). */
class CapturingLog {
    /** One log record: level 0 trace to 5 fatal, the target, the message. */
    data class Record(val level: Int, val target: String, val message: String)

    /** Every record so far, oldest first. */
    val records = CopyOnWriteArrayList<Record>()

    /** This log as a sync `Log` port. */
    fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Log.LOG] = { args ->
                val r = UndraReader(args)
                records.add(Record(r.readU8().toInt(), r.readStr(), r.readStr()))
                r.finish()
                ByteArray(0)
            }
        },
    )
}
