package dev.undra.runtime.adapters

import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraLog
import dev.undra.runtime.NamedDaemonThreads
import dev.undra.runtime.PortImpl
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.encodeToByteArray
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
internal inline fun <T> readArgs(args: ByteArray, read: (UndraReader) -> T): T {
    val r = UndraReader(args)
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
                val r = UndraReader(args)
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

/**
 * The `Diagnostics` port (ADR-046): the core calls `panicked(report)` once for every panic it contained, fire and forget, on the thread
 * it panicked on, possibly with the core lock held. The adapter decodes the one [UndraPanicReport] and hands it to [handler] **on that
 * thread**, so [handler] must return quickly, must not call into Undra and must not throw (a failure of the handler is the port's failure:
 * logged, and the core is answered "unavailable", which it ignores). The core registers its own adapter for every core it loads; that
 * handler hops to the runtime's main thread and calls `LoadOptions.onPanic` there. Register another one in `LoadOptions.adapters` to
 * take the reports yourself, as the testing kit's `CaptureDiagnostics` does.
 *
 * @param handler receives each decoded report.
 */
public class DiagnosticsAdapter(private val handler: (UndraPanicReport) -> Unit) {
    /** Hands [report] to the handler. */
    public fun panicked(report: UndraPanicReport) {
        handler(report)
    }

    /** This adapter as a sync [PortImpl] for [StandardPorts.Diagnostics]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Diagnostics.PANICKED] = { args ->
                panicked(readArgs(args) { UndraPanicReport.decode(it) })
                NO_REPLY
            }
        },
    )
}

/** The mapping of Undra log levels (SPEC 8: 0 trace ... 5 fatal) to `java.util.logging`. */
public object JulLog {
    /** Writes [message] to the logger named [target] (`undra` when empty) at the level for [level]. */
    public fun log(level: UByte, target: String, message: String) {
        val julLevel = when (level.toInt()) {
            0 -> Level.FINEST
            1 -> Level.FINE
            2 -> Level.INFO
            3 -> Level.WARNING
            4, 5 -> Level.SEVERE
            else -> Level.INFO
        }
        Logger.getLogger(target.ifEmpty { "undra" }).log(julLevel, message)
    }
}

/**
 * The `Timer` port over a `ScheduledThreadPoolExecutor` with one daemon thread named `undra-timer`
 * (SPEC 5.8): the core asks for `set(timer_id, delay_ms)`, and when the delay has passed [fire] is called
 * with the id (which must tell the core: `UndraCore.timerFired`).
 *
 * The thread starts with the first timer and exits after the adapter has been idle for a few seconds
 * (never while a timer is pending), so an adapter nobody closes does not keep a thread around.
 *
 * @param fire called on the timer thread when a timer is due; exceptions are logged and dropped.
 */
public class TimerAdapter internal constructor(private val fire: (UInt) -> Unit, private val idleMillis: Long) : AutoCloseable {
    /** A timer adapter that calls [fire] when a timer is due. */
    public constructor(fire: (UInt) -> Unit) : this(fire, IDLE_MILLIS)

    private val executor: ScheduledThreadPoolExecutor by lazy {
        ScheduledThreadPoolExecutor(1, NamedDaemonThreads("undra-timer")).also {
            it.removeOnCancelPolicy = true
            it.setKeepAliveTime(idleMillis, TimeUnit.MILLISECONDS)
            it.allowCoreThreadTimeOut(true) // ThreadPoolExecutor keeps the last worker while delayed tasks are queued
        }
    }

    internal val hasLiveThread: Boolean get() = executor.poolSize > 0

    /** Schedules timer [timerId] to fire after [delayMs] milliseconds. */
    public fun set(timerId: UInt, delayMs: ULong) {
        val delay = if (delayMs > Long.MAX_VALUE.toULong()) Long.MAX_VALUE else delayMs.toLong()
        executor.schedule(
            {
                try {
                    fire(timerId)
                } catch (e: Exception) {
                    UndraLog.warn("firing timer $timerId failed", e)
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

    private companion object {
        const val IDLE_MILLIS: Long = 5_000L
    }

    /** This adapter as a sync [PortImpl] for [StandardPorts.Timer]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Timer.SET] = { args ->
                val r = UndraReader(args)
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
 * stub for tests and desktop hosts to drive by hand; the `android-adapters` module feeds it from
 * `ConnectivityManager` (`AndroidConnectivityAdapter`). Until told otherwise the core assumes the network is up.
 */
public class ConnectivityEvents(private val core: UndraCore = UndraCore.shared) {
    /** Reports that the network is [online] (or not) and of what [kind] it is. */
    public fun changed(online: Boolean, kind: NetKind) {
        val w = UndraWriter(3)
        w.writeBool(online)
        NetKind.encode(w, kind)
        core.event(StandardPorts.Connectivity.PORT_ID, StandardPorts.Connectivity.CHANGED, w.toByteArray())
    }
}

/**
 * Sends `Lifecycle.changed` to the core. A stub on a plain JVM (nothing observes an app lifecycle there);
 * the `android-adapters` module feeds it from the app's activities (`AndroidLifecycleAdapter`).
 */
public class LifecycleEvents(private val core: UndraCore = UndraCore.shared) {
    /** Reports that the app moved to [state]. */
    public fun changed(state: AppState) {
        core.event(StandardPorts.Lifecycle.PORT_ID, StandardPorts.Lifecycle.CHANGED, AppState.encodeToByteArray(state))
    }
}
