package dev.keel.runtime.wire

import java.util.UUID
import kotlin.time.Duration
import kotlin.time.Duration.Companion.nanoseconds

/**
 * The built-in [KeelCodec]s (SPEC §3.1 value encoding) and the combinators generated code builds
 * records, enums and containers from.
 *
 * ```kotlin
 * val todos: KeelCodec<List<Todo>> = Codecs.vec(Todo)
 * val byName: KeelCodec<Map<String, Int>> = Codecs.map(Codecs.string, Codecs.i32)
 * ```
 */
public object Codecs {

    /** `bool`: one byte, 0 or 1; any other byte is rejected on decode. */
    public val bool: KeelCodec<Boolean> = object : KeelCodec<Boolean> {
        override fun encode(w: KeelWriter, v: Boolean) = w.writeBool(v)
        override fun decode(r: KeelReader): Boolean = r.readBool()
    }

    /** `u8`: one byte. */
    public val u8: KeelCodec<UByte> = object : KeelCodec<UByte> {
        override fun encode(w: KeelWriter, v: UByte) = w.writeU8(v)
        override fun decode(r: KeelReader): UByte = r.readU8()
    }

    /** `i8`: one byte, two's complement. */
    public val i8: KeelCodec<Byte> = object : KeelCodec<Byte> {
        override fun encode(w: KeelWriter, v: Byte) = w.writeI8(v)
        override fun decode(r: KeelReader): Byte = r.readI8()
    }

    /** `u16`: two bytes, little-endian (also the enum variant index). */
    public val u16: KeelCodec<UShort> = object : KeelCodec<UShort> {
        override fun encode(w: KeelWriter, v: UShort) = w.writeU16(v)
        override fun decode(r: KeelReader): UShort = r.readU16()
    }

    /** `i16`: two bytes, little-endian. */
    public val i16: KeelCodec<Short> = object : KeelCodec<Short> {
        override fun encode(w: KeelWriter, v: Short) = w.writeI16(v)
        override fun decode(r: KeelReader): Short = r.readI16()
    }

    /** `u32`: four bytes, little-endian. */
    public val u32: KeelCodec<UInt> = object : KeelCodec<UInt> {
        override fun encode(w: KeelWriter, v: UInt) = w.writeU32(v)
        override fun decode(r: KeelReader): UInt = r.readU32()
    }

    /** `i32`: four bytes, little-endian. */
    public val i32: KeelCodec<Int> = object : KeelCodec<Int> {
        override fun encode(w: KeelWriter, v: Int) = w.writeI32(v)
        override fun decode(r: KeelReader): Int = r.readI32()
    }

    /** `u64`: eight bytes, little-endian. */
    public val u64: KeelCodec<ULong> = object : KeelCodec<ULong> {
        override fun encode(w: KeelWriter, v: ULong) = w.writeU64(v)
        override fun decode(r: KeelReader): ULong = r.readU64()
    }

    /** `i64`: eight bytes, little-endian. */
    public val i64: KeelCodec<Long> = object : KeelCodec<Long> {
        override fun encode(w: KeelWriter, v: Long) = w.writeI64(v)
        override fun decode(r: KeelReader): Long = r.readI64()
    }

    /** `f32`: IEEE 754 bits, little-endian. NaN payloads round-trip bit-exactly. */
    public val f32: KeelCodec<Float> = object : KeelCodec<Float> {
        override fun encode(w: KeelWriter, v: Float) = w.writeF32(v)
        override fun decode(r: KeelReader): Float = r.readF32()
    }

    /** `f64`: IEEE 754 bits, little-endian. NaN payloads round-trip bit-exactly. */
    public val f64: KeelCodec<Double> = object : KeelCodec<Double> {
        override fun encode(w: KeelWriter, v: Double) = w.writeF64(v)
        override fun decode(r: KeelReader): Double = r.readF64()
    }

    /** `Unit`: zero bytes. */
    public val unit: KeelCodec<Unit> = object : KeelCodec<Unit> {
        override fun encode(w: KeelWriter, v: Unit) = Unit
        override fun decode(r: KeelReader) = Unit
    }

    /**
     * `String`: `u32` byte length then UTF-8. Decoding validates strictly; encoding rejects strings
     * with unpaired surrogates ([IllegalArgumentException]) instead of silently substituting `?`.
     */
    public val string: KeelCodec<String> = object : KeelCodec<String> {
        override fun encode(w: KeelWriter, v: String) = w.writeStr(v)
        override fun decode(r: KeelReader): String = r.readStr()
    }

    /**
     * `Bytes`: `u32` length then raw bytes. Decoding returns a copy. Note that `ByteArray` has
     * identity equality in Kotlin, so records holding one need to compare it with `contentEquals`.
     */
    public val bytes: KeelCodec<ByteArray> = object : KeelCodec<ByteArray> {
        override fun encode(w: KeelWriter, v: ByteArray) = w.writeBytes(v)
        override fun decode(r: KeelReader): ByteArray = r.readBytes()
    }

    /**
     * `Duration`: `i64` nanoseconds.
     *
     * The wire duration is unsigned on the core side, so a negative value is rejected in both
     * directions with [WireException.NegativeDuration]. Encoding also rejects [Duration.INFINITE] and
     * anything above `Long.MAX_VALUE` nanoseconds (about 292 years) with [IllegalArgumentException].
     *
     * `kotlin.time.Duration` keeps nanosecond precision only up to about 146 years and whole
     * milliseconds beyond that, so decoded values above that lose their sub-millisecond part.
     */
    public val duration: KeelCodec<Duration> = object : KeelCodec<Duration> {
        override fun encode(w: KeelWriter, v: Duration) {
            if (v.isNegative()) throw WireException.NegativeDuration(v.inWholeNanoseconds, w.size)
            require(!v.isInfinite() && v.inWholeMilliseconds <= MAX_DURATION_MILLIS) {
                "duration $v does not fit in i64 nanoseconds"
            }
            w.writeI64(v.inWholeNanoseconds)
        }

        override fun decode(r: KeelReader): Duration {
            val at = r.position
            val nanos = r.readI64()
            if (nanos < 0) throw WireException.NegativeDuration(nanos, at)
            return nanos.nanoseconds
        }
    }

    /** `Timestamp`: `i64` milliseconds since the Unix epoch. */
    public val timestamp: KeelCodec<Timestamp> = object : KeelCodec<Timestamp> {
        override fun encode(w: KeelWriter, v: Timestamp) = w.writeI64(v.epochMillis)
        override fun decode(r: KeelReader): Timestamp = Timestamp(r.readI64())
    }

    /** `Uuid`: 16 raw bytes, big-endian as in RFC 4122 (the most significant half first). */
    public val uuid: KeelCodec<UUID> = object : KeelCodec<UUID> {
        override fun encode(w: KeelWriter, v: UUID) {
            w.writeI64BigEndian(v.mostSignificantBits)
            w.writeI64BigEndian(v.leastSignificantBits)
        }

        override fun decode(r: KeelReader): UUID {
            val msb = r.readI64BigEndian()
            val lsb = r.readI64BigEndian()
            return UUID(msb, lsb)
        }
    }

    /**
     * Object handle: the raw `u64` (see [Handle]; wrap with `Handle(raw)` to inspect index and
     * generation).
     */
    public val handle: KeelCodec<Long> = i64

    /**
     * `Option<T>`: a tag byte (0 = `None`, 1 = `Some`) followed by the item. Represented as `T?`, so
     * `T` must be non-null: nested options (`Option<Option<T>>`) would collapse and are therefore
     * rejected by the type system.
     */
    public fun <T : Any> option(item: KeelCodec<T>): KeelCodec<T?> = OptionCodec(item)

    /**
     * `Vec<T>`: a `u32` count followed by the items. Decoding preallocates exactly `count` slots
     * after checking that the count fits in the remaining bytes, so every item must encode to at
     * least one byte: a `Vec` of `Unit` (or of another zero-width type) with a count larger than the
     * remaining input is rejected with [WireException.LengthTooLarge].
     */
    public fun <T> vec(item: KeelCodec<T>): KeelCodec<List<T>> = VecCodec(item)

    /**
     * `Map<K, V>`: a `u32` count followed by (key, value) pairs.
     *
     * Encoding sorts the pairs by the **encoded key bytes** (unsigned, lexicographic) so that equal
     * maps always produce identical bytes regardless of iteration order; note that for strings this
     * orders by length prefix first. Encoding rejects keys with byte-identical encodings and decoding
     * rejects repeated keys, both with [WireException.DuplicateKey]. Decoding accepts any pair order
     * and returns a [LinkedHashMap] that preserves it.
     */
    public fun <K, V> map(key: KeelCodec<K>, value: KeelCodec<V>): KeelCodec<Map<K, V>> = MapCodec(key, value)

    /** `Result<T, E>`: tag 0 = [KeelResult.Ok] + `T`, tag 1 = [KeelResult.Err] + `E`. */
    public fun <T, E> result(ok: KeelCodec<T>, err: KeelCodec<E>): KeelCodec<KeelResult<T, E>> = ResultCodec(ok, err)

    /** `Duration` values above this many whole milliseconds cannot be expressed in i64 nanoseconds. */
    private const val MAX_DURATION_MILLIS: Long = Long.MAX_VALUE / 1_000_000L
}

private class OptionCodec<T : Any>(private val item: KeelCodec<T>) : KeelCodec<T?> {
    override fun encode(w: KeelWriter, v: T?) {
        if (v == null) {
            w.writeU8(0u)
        } else {
            w.writeU8(1u)
            item.encode(w, v)
        }
    }

    override fun decode(r: KeelReader): T? {
        val at = r.position
        return when (val tag = r.readU8().toInt()) {
            0 -> null
            1 -> item.decode(r)
            else -> throw WireException.InvalidTag(tag.toUInt(), at, "Option")
        }
    }
}

private class VecCodec<T>(private val item: KeelCodec<T>) : KeelCodec<List<T>> {
    override fun encode(w: KeelWriter, v: List<T>) {
        val n = v.size
        w.writeLen(n)
        if (v is RandomAccess) {
            for (i in 0 until n) item.encode(w, v[i])
        } else {
            for (x in v) item.encode(w, x)
        }
    }

    override fun decode(r: KeelReader): List<T> {
        val n = r.readLen()
        val out = ArrayList<T>(n)
        for (i in 0 until n) out.add(item.decode(r))
        return out
    }
}

private class MapCodec<K, V>(
    private val key: KeelCodec<K>,
    private val value: KeelCodec<V>,
) : KeelCodec<Map<K, V>> {

    override fun encode(w: KeelWriter, v: Map<K, V>) {
        val n = v.size
        w.writeLen(n)
        if (n == 0) return
        if (n == 1) {
            for ((k, x) in v) {
                key.encode(w, k)
                value.encode(w, x)
            }
            return
        }
        val mapStart = w.size
        // Encode every entry (key then value) into one scratch buffer, remembering the boundaries,
        // then emit the entries ordered by their encoded key bytes.
        val scratch = KeelWriter()
        val entryStart = IntArray(n + 1)
        val keyEnd = IntArray(n)
        var i = 0
        for ((k, x) in v) {
            entryStart[i] = scratch.size
            key.encode(scratch, k)
            keyEnd[i] = scratch.size
            value.encode(scratch, x)
            i++
        }
        entryStart[n] = scratch.size
        val bytes = scratch.backingArray()
        val order = Array(n) { it }
        order.sortWith { a, b ->
            compareUnsigned(bytes, entryStart[a], keyEnd[a], entryStart[b], keyEnd[b])
        }
        for (j in 1 until n) {
            val a = order[j - 1]
            val b = order[j]
            if (compareUnsigned(bytes, entryStart[a], keyEnd[a], entryStart[b], keyEnd[b]) == 0) {
                throw WireException.DuplicateKey(mapStart)
            }
        }
        for (j in 0 until n) {
            val e = order[j]
            w.writeRaw(bytes, entryStart[e], entryStart[e + 1] - entryStart[e])
        }
    }

    override fun decode(r: KeelReader): Map<K, V> {
        val n = r.readLen()
        val out = LinkedHashMap<K, V>(hashCapacity(n))
        for (i in 0 until n) {
            val at = r.position
            val k = key.decode(r)
            val x = value.decode(r)
            val before = out.size
            out[k] = x
            if (out.size == before) throw WireException.DuplicateKey(at)
        }
        return out
    }

    private fun compareUnsigned(bytes: ByteArray, aFrom: Int, aTo: Int, bFrom: Int, bTo: Int): Int {
        val la = aTo - aFrom
        val lb = bTo - bFrom
        val common = if (la < lb) la else lb
        for (i in 0 until common) {
            val d = (bytes[aFrom + i].toInt() and 0xFF) - (bytes[bFrom + i].toInt() and 0xFF)
            if (d != 0) return d
        }
        return la - lb
    }

    /** Initial capacity that holds [n] entries without rehashing at the default 0.75 load factor. */
    private fun hashCapacity(n: Int): Int {
        if (n < 3) return n + 1
        return (n.toLong() * 4 / 3 + 1).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
    }
}

private class ResultCodec<T, E>(
    private val ok: KeelCodec<T>,
    private val err: KeelCodec<E>,
) : KeelCodec<KeelResult<T, E>> {
    override fun encode(w: KeelWriter, v: KeelResult<T, E>) {
        when (v) {
            is KeelResult.Ok -> {
                w.writeU8(0u)
                ok.encode(w, v.value)
            }
            is KeelResult.Err -> {
                w.writeU8(1u)
                err.encode(w, v.error)
            }
        }
    }

    override fun decode(r: KeelReader): KeelResult<T, E> {
        val at = r.position
        return when (val tag = r.readU8().toInt()) {
            0 -> KeelResult.Ok(ok.decode(r))
            1 -> KeelResult.Err(err.decode(r))
            else -> throw WireException.InvalidTag(tag.toUInt(), at, "Result")
        }
    }
}
