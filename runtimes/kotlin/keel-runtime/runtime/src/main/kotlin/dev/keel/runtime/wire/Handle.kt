package dev.keel.runtime.wire

/**
 * An object handle issued by the core's object table (SPEC §1.2).
 *
 * A handle is a `u64`: the low 32 bits are the slot [index], the high 32 bits the [generation]
 * (starting at 1, so a reused slot never matches an old handle). `0` is the null handle. Handles are
 * only meaningful inside the runtime instance that issued them.
 *
 * On the wire a handle is just its [raw] `u64`; `Codecs.handle` encodes the raw value.
 *
 * @property raw the handle as the `u64` bit pattern (Kotlin has no unsigned `Long`, so handles with
 *   a generation of 2^31 or more appear negative here; use [index] and [generation] to inspect it).
 */
@JvmInline
public value class Handle(public val raw: Long) {

    /** Slot index in the core's object table (low 32 bits). */
    public val index: UInt get() = raw.toUInt()

    /** Generation counter of the slot (high 32 bits); `0` only for the null handle. */
    public val generation: UInt get() = (raw ushr 32).toUInt()

    /** `true` for the null handle (`raw == 0`). */
    public val isNull: Boolean get() = raw == 0L

    override fun toString(): String = if (isNull) "Handle(null)" else "Handle(index=$index, generation=$generation)"

    public companion object {
        /** The null handle. */
        public val NULL: Handle = Handle(0L)

        /** Builds a handle from its slot [index] and [generation]. */
        public fun make(index: UInt, generation: UInt): Handle =
            Handle((generation.toLong() shl 32) or index.toLong())
    }
}
