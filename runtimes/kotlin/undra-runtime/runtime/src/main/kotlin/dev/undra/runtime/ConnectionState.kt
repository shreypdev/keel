package dev.undra.runtime

import java.util.concurrent.ThreadLocalRandom
import kotlin.time.Duration
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds

/**
 * What the connection to the core is doing ([UndraCore.connectionState]). Only a [Mode.REMOTE] core ever
 * leaves [Connected]: it is [Reconnecting] after the connection drops, [Connected] again once its stores
 * are observed again, and [Closed] for good when the app closes it, its schema changed, the dev server lost
 * its session, or the transport gave up (ADR-034).
 *
 * ```kotlin
 * val state by UndraCore.shared.connectionState.collectAsState()
 * if (state is ConnectionState.Reconnecting) Text("Reconnecting to the dev server...")
 * ```
 */
public sealed interface ConnectionState {
    /** [UndraCore.load] is connecting; only [LoadOptions.onConnectionChange] can see it. */
    public data object Connecting : ConnectionState

    /** The core is reachable. */
    public data object Connected : ConnectionState

    /**
     * The connection dropped and the transport is trying again: attempt [attempt] (from 1) is waiting for its
     * backoff or connecting. Calls and `observe` fail at once ([UndraException]); what was in flight when the
     * connection dropped failed with it. [cause] is why the connection was lost (or the last attempt failed).
     */
    public data class Reconnecting(public val attempt: Int, public val cause: Throwable?) : ConnectionState

    /** The core is closed for good ([reason]); a new `UndraCore.load` is the way back. [cause] says why, unless the app closed it. */
    public data class Closed(public val reason: ClosedReason, public val cause: Throwable? = null) : ConnectionState
}

/** Why a core is [ConnectionState.Closed]. */
public enum class ClosedReason {
    /** The app called [UndraCore.close]. */
    REQUESTED,

    /** The core was rebuilt with another schema ([UndraSchemaMismatchException]). */
    SCHEMA_MISMATCH,

    /** The dev server no longer holds this core's objects: it was restarted, or the session expired ([UndraSessionLostException]). */
    SESSION_LOST,

    /** The connection failed for good: a protocol error, or the reconnect policy gave up. */
    FAILED,
}

/**
 * How a remote core reconnects after its connection drops (ADR-034). Attempt `n` (from 1) waits
 * `min(maxDelay, initialDelay * 2^(n-1))`, less a random share of up to [jitter] of that, so many clients of
 * one server do not retry in step. The same schedule in the TypeScript and Swift runtimes.
 *
 * @property initialDelay the wait before the first retry.
 * @property maxDelay the longest wait.
 * @property jitter the share of the wait that is randomised away, from 0 (none) to 1 (down to nothing).
 * @property maxAttempts give up (and close the core) after this many failed attempts; by default never.
 * @property random numbers in `[0, 1)` for the jitter; tests pass a fixed one.
 */
public class ReconnectPolicy(
    public val initialDelay: Duration = 250.milliseconds,
    public val maxDelay: Duration = 5.seconds,
    public val jitter: Double = 0.5,
    public val maxAttempts: Int = Int.MAX_VALUE,
    public val random: () -> Double = { ThreadLocalRandom.current().nextDouble() },
) {
    init {
        require(initialDelay.isPositive() && maxDelay >= initialDelay) { "need 0 < initialDelay <= maxDelay, got $initialDelay and $maxDelay" }
        require(jitter in 0.0..1.0) { "jitter must be between 0 and 1, got $jitter" }
        require(maxAttempts > 0) { "maxAttempts must be positive, got $maxAttempts" }
    }

    /** How long reconnect attempt [attempt] (from 1) waits. */
    public fun delayFor(attempt: Int): Duration {
        val doublings = (attempt - 1).coerceIn(0, 30)
        val base = minOf(maxDelay.inWholeMilliseconds, initialDelay.inWholeMilliseconds * (1L shl doublings))
        return Math.round(base * (1.0 - jitter * random())).milliseconds
    }

    override fun toString(): String = "ReconnectPolicy(initialDelay=$initialDelay, maxDelay=$maxDelay, jitter=$jitter, maxAttempts=$maxAttempts)"
}
