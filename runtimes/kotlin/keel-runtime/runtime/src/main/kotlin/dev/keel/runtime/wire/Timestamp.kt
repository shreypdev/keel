package dev.keel.runtime.wire

import java.time.Instant

/**
 * A point in time as milliseconds since the Unix epoch (SPEC §3.1 `Timestamp`, encoded as `i64`).
 *
 * `kotlinx-datetime` is deliberately not a dependency of the runtime, so this is the wire-level
 * representation; convert with [toInstant] / [ofInstant]. (`java.time` is available on Android
 * through core-library desugaring.) Precision is one millisecond: [ofInstant] floors finer values.
 *
 * @property epochMillis milliseconds since 1970-01-01T00:00:00Z; negative before the epoch.
 */
@JvmInline
public value class Timestamp(public val epochMillis: Long) : Comparable<Timestamp> {

    /** This timestamp as a [java.time.Instant]. Every `Long` value is representable. */
    public fun toInstant(): Instant = Instant.ofEpochMilli(epochMillis)

    override fun compareTo(other: Timestamp): Int = epochMillis.compareTo(other.epochMillis)

    override fun toString(): String = "Timestamp($epochMillis)"

    public companion object {
        /**
         * Converts [instant], flooring to a whole millisecond (so instants before the epoch round
         * towards the past, never towards zero).
         *
         * @throws IllegalArgumentException if [instant] is outside the range a `Long` of milliseconds can hold.
         */
        public fun ofInstant(instant: Instant): Timestamp = try {
            Timestamp(instant.toEpochMilli())
        } catch (e: ArithmeticException) {
            throw IllegalArgumentException("instant $instant is outside the range of a Timestamp", e)
        }
    }
}
