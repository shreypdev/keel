package dev.keel.contract

import dev.keel.runtime.PortImpl
import dev.keel.runtime.adapters.StandardPorts
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.encodeToByteArray
import java.util.concurrent.atomic.AtomicLong

/**
 * The `Clock` port the test drives by hand (scenarios.md, "Adapters"): it starts at
 * [START_MS] and moves only when the test says so. The query cache reads it to decide what is stale;
 * timers do not (they use the `Timer` port, which stays real).
 */
class ManualClock {
    private val millis = AtomicLong(START_MS)

    /** The clock's current time, in milliseconds since the Unix epoch. */
    val nowMs: Long get() = millis.get()

    /** Moves the clock forward by [ms] milliseconds. */
    fun advance(ms: Long) {
        millis.addAndGet(ms)
    }

    /** This clock as a sync `Clock` port. The monotonic reading is the wall reading since [START_MS], in ns. */
    fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Clock.NOW_MS] = { Codecs.i64.encodeToByteArray(nowMs) }
            this[StandardPorts.Clock.MONOTONIC_NS] = { Codecs.u64.encodeToByteArray((nowMs - START_MS).toULong() * 1_000_000uL) }
        },
    )

    /** The start time. */
    companion object {
        /** `1_700_000_000_000` ms, the start time scenarios.md prescribes. */
        const val START_MS: Long = 1_700_000_000_000L
    }
}
