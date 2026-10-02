package dev.undra.runtime.wire

/**
 * An object handle issued by the core's object table (SPEC §1.2).
 *
 * A handle is a `u64`: the low 24 bits are the slot [index], the high 40 bits the [generation]
 * (starting at 1, so a reused slot never matches an old handle; ADR-040 decision 8). `0` is the null
 * handle. Handles are only meaningful inside the runtime instance that issued them: two cores hand out
 * the same numbers (ADR-044). Hosts never interpret them; these accessors are for logs and tests.
 *
 * On the wire a handle is just its [raw] `u64`; `Codecs.handle` encodes the raw value.
 *
 * @property raw the handle as the `u64` bit pattern (Kotlin has no unsigned `Long`, so handles with
 *   a generation of 2^39 or more appear negative here; use [index] and [generation] to inspect it).
 */
@JvmInline
public value class Handle(public val raw: Long) {

    /** Slot index in the core's object table (low 24 bits). */
    public val index: UInt get() = (raw and INDEX_MASK).toUInt()

    /** Generation counter of the slot (high 40 bits); `0` only for the null handle. */
    public val generation: ULong get() = (raw ushr INDEX_BITS).toULong()

    /** `true` for the null handle (`raw == 0`). */
    public val isNull: Boolean get() = raw == 0L

    override fun toString(): String = if (isNull) "Handle(null)" else "Handle(index=$index, generation=$generation)"

    public companion object {
        /** Bits of slot index in a handle. */
        public const val INDEX_BITS: Int = 24

        /** The largest slot index a handle can carry (16,777,215). */
        public const val MAX_INDEX: UInt = (1u shl INDEX_BITS) - 1u

        /** The largest generation a handle can carry (2^40 - 1). */
        public const val MAX_GENERATION: ULong = (1uL shl (64 - INDEX_BITS)) - 1uL

        private const val INDEX_MASK: Long = (1L shl INDEX_BITS) - 1L

        /** The null handle. */
        public val NULL: Handle = Handle(0L)

        /**
         * Builds a handle from its slot [index] and [generation].
         *
         * @throws IllegalArgumentException if [index] is past [MAX_INDEX] or [generation] past [MAX_GENERATION].
         */
        public fun make(index: UInt, generation: ULong): Handle {
            require(index <= MAX_INDEX) { "a handle's slot index is 24 bits; $index is too large" }
            require(generation <= MAX_GENERATION) { "a handle's generation is 40 bits; $generation is too large" }
            return Handle((generation.toLong() shl INDEX_BITS) or index.toLong())
        }
    }
}
