package dev.keel.runtime.adapters

import dev.keel.runtime.KeelCore
import dev.keel.runtime.KeelLog
import dev.keel.runtime.NamedDaemonThreads
import dev.keel.runtime.PortImpl
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.KeelReader
import dev.keel.runtime.wire.KeelWriter
import dev.keel.runtime.wire.encodeToByteArray
import java.security.SecureRandom
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.logging.Level
import java.util.logging.Logger

internal typealias PortMethod = suspend (ByteArray) -> ByteArray

/** Builds the method table of a [PortImpl]: `portMethods { this[ID] = { args -> reply } }`. */
internal fun portMethods(fill: MutableMap<UInt, PortMethod>.() -> Unit): Map<UInt, PortMethod> =
    LinkedHashMap<UInt, PortMethod>().apply(fill)

private val NO_REPLY = ByteArray(0)

/** Decodes the arguments of a port method with [read] and requires that nothing is left over. */
private inline fun <T> readArgs(args: ByteArray, read: (KeelReader) -> T): T {
    val r = KeelReader(args)
    val value = read(r)
    r.finish()
    return value
}

/**
 * The `Clock` port over the JVM clocks: `now_ms` is `System.currentTimeMillis()`, `monotonic_ns` counts
 * nanoseconds from when the adapter was created (`System.nanoTime()` has an arbitrary, possibly negative, origin).
 *
 * @param wallMillis the wall clock, in milliseconds since the Unix epoch.
 * @param nanoTime the monotonic clock, in nanoseconds from an arbitrary origin.
 */
public class ClockAdapter(
    private val wallMillis: () -> Long = System::currentTimeMillis,
    private val nanoTime: () -> Long = System::nanoTime,
) {
    private val origin = nanoTime()

    /** Milliseconds since the Unix epoch. */
    public fun nowMs(): Long = wallMillis()

    /** Nanoseconds since this adapter was created; never decreases. */
    public fun monotonicNs(): ULong = (nanoTime() - origin).coerceAtLeast(0L).toULong()

    /** This adapter as a sync [PortImpl] for [StandardPorts.Clock]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Clock.NOW_MS] = { Codecs.i64.encodeToByteArray(nowMs()) }
            this[StandardPorts.Clock.MONOTONIC_NS] = { Codecs.u64.encodeToByteArray(monotonicNs()) }
        },
    )
}

/**
 * The `Rng` port over [SecureRandom].
 *
 * @param random the source of bytes.
 */
public class RngAdapter(private val random: SecureRandom = SecureRandom()) {
    /**
     * [len] random bytes.
     *
     * @throws IllegalArgumentException if [len] is above [MAX_FILL] (a runaway request must not exhaust memory).
     */
    public fun fill(len: UInt): ByteArray {
        require(len <= MAX_FILL) { "Rng.fill($len) exceeds the limit of $MAX_FILL bytes" }
        return ByteArray(len.toInt()).also { random.nextBytes(it) }
    }

    /** This adapter as a sync [PortImpl] for [StandardPorts.Rng]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Rng.FILL] = { args -> Codecs.bytes.encodeToByteArray(fill(readArgs(args) { it.readU32() })) }
        },
    )

    /** Limits. */
    public companion object {
        /** The most bytes one `fill` will produce: 16 MiB. */
        public const val MAX_FILL: UInt = 16_777_216u
    }
}

/**
 * The `Log` port over `java.util.logging` (levels: 0 trace to `FINEST`, 1 debug to `FINE`, 2 info to `INFO`,
 * 3 warn to `WARNING`, 4 error and 5 fatal to `SEVERE`). The logger is named after the record's target.
 */
public class LogAdapter {
    /** Writes one record. */
    public fun log(level: UByte, target: String, message: String) {
        JulLog.log(level, target, message)
    }

    /** This adapter as a sync [PortImpl] for [StandardPorts.Log]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Log.LOG] = { args ->
                val r = KeelReader(args)
                val level = r.readU8()
                val target = r.readStr()
                val message = r.readStr()
                r.finish()
                log(level, target, message)
                NO_REPLY
            }
        },
    )
}

/** The mapping of Keel log levels (SPEC 8: 0 trace ... 5 fatal) to `java.util.logging`. */
public object JulLog {
    /** Writes [message] to the logger named [target] (`keel` when empty) at the level for [level]. */
    public fun log(level: UByte, target: String, message: String) {
        val julLevel = when (level.toInt()) {
            0 -> Level.FINEST
            1 -> Level.FINE
            2 -> Level.INFO
            3 -> Level.WARNING
            4, 5 -> Level.SEVERE
            else -> Level.INFO
        }
        Logger.getLogger(target.ifEmpty { "keel" }).log(julLevel, message)
    }
}

/**
 * The `Timer` port over a `ScheduledThreadPoolExecutor` with one daemon thread named `keel-timer`
 * (SPEC 5.8): the core asks for `set(timer_id, delay_ms)`, and when the delay has passed [fire] is called
 * with the id (which must tell the core: `KeelCore.timerFired`).
 *
 * @param fire called on the timer thread when a timer is due; exceptions are logged and dropped.
 */
public class TimerAdapter(private val fire: (UInt) -> Unit) : AutoCloseable {
    private val executor: ScheduledThreadPoolExecutor by lazy {
        ScheduledThreadPoolExecutor(1, NamedDaemonThreads("keel-timer")).also { it.removeOnCancelPolicy = true }
    }

    /** Schedules timer [timerId] to fire after [delayMs] milliseconds. */
    public fun set(timerId: UInt, delayMs: ULong) {
        val delay = if (delayMs > Long.MAX_VALUE.toULong()) Long.MAX_VALUE else delayMs.toLong()
        executor.schedule(
            {
                try {
                    fire(timerId)
                } catch (e: Exception) {
                    KeelLog.warn("firing timer $timerId failed", e)
                }
            },
            delay,
            TimeUnit.MILLISECONDS,
        )
    }

    /** Cancels every pending timer and stops the timer thread. */
    override fun close() {
        executor.shutdownNow()
    }

    /** This adapter as a sync [PortImpl] for [StandardPorts.Timer]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Timer.SET] = { args ->
                val r = KeelReader(args)
                val id = r.readU32()
                val delay = r.readU64()
                r.finish()
                this@TimerAdapter.set(id, delay)
                NO_REPLY
            }
        },
    )
}

/**
 * Sends `Connectivity.changed` to the core. There is no connectivity source on a plain JVM, so this is a
 * stub for tests and desktop hosts to drive by hand; the `android-adapters` module will feed it from
 * `ConnectivityManager`. Until told otherwise the core assumes the network is up.
 */
public class ConnectivityEvents(private val core: KeelCore = KeelCore.shared) {
    /** Reports that the network is [online] (or not) and of what [kind] it is. */
    public fun changed(online: Boolean, kind: NetKind) {
        val w = KeelWriter(3)
        w.writeBool(online)
        NetKind.encode(w, kind)
        core.event(StandardPorts.Connectivity.PORT_ID, StandardPorts.Connectivity.CHANGED, w.toByteArray())
    }
}

/**
 * Sends `Lifecycle.changed` to the core. A stub on a plain JVM (nothing observes an app lifecycle there);
 * the `android-adapters` module will feed it from `ProcessLifecycleOwner`.
 */
public class LifecycleEvents(private val core: KeelCore = KeelCore.shared) {
    /** Reports that the app moved to [state]. */
    public fun changed(state: AppState) {
        core.event(StandardPorts.Lifecycle.PORT_ID, StandardPorts.Lifecycle.CHANGED, AppState.encodeToByteArray(state))
    }
}
