package dev.undra.runtime.wire

import java.math.BigDecimal
import java.math.BigInteger
import java.math.RoundingMode
import java.util.UUID
import kotlin.time.Duration
import kotlin.time.Duration.Companion.nanoseconds

/**
 * The built-in [UndraCodec]s (SPEC §3.1 value encoding) and the combinators generated code builds
 * records, enums and containers from.
 *
 * ```kotlin
 * val todos: UndraCodec<List<Todo>> = Codecs.vec(Todo)
 * val byName: UndraCodec<Map<String, Int>> = Codecs.map(Codecs.string, Codecs.i32)
 * ```
 */
public object Codecs {

    /** `bool`: one byte, 0 or 1; any other byte is rejected on decode. */
    public val bool: UndraCodec<Boolean> = object : UndraCodec<Boolean> {
        override fun encode(w: UndraWriter, v: Boolean) = w.writeBool(v)
        override fun decode(r: UndraReader): Boolean = r.readBool()
    }

    /** `u8`: one byte. */
    public val u8: UndraCodec<UByte> = object : UndraCodec<UByte> {
        override fun encode(w: UndraWriter, v: UByte) = w.writeU8(v)
        override fun decode(r: UndraReader): UByte = r.readU8()
    }

    /** `i8`: one byte, two's complement. */
    public val i8: UndraCodec<Byte> = object : UndraCodec<Byte> {
        override fun encode(w: UndraWriter, v: Byte) = w.writeI8(v)
        override fun decode(r: UndraReader): Byte = r.readI8()
    }

    /** `u16`: two bytes, little-endian (also the enum variant index). */
    public val u16: UndraCodec<UShort> = object : UndraCodec<UShort> {
        override fun encode(w: UndraWriter, v: UShort) = w.writeU16(v)
        override fun decode(r: UndraReader): UShort = r.readU16()
    }

    /** `i16`: two bytes, little-endian. */
    public val i16: UndraCodec<Short> = object : UndraCodec<Short> {
        override fun encode(w: UndraWriter, v: Short) = w.writeI16(v)
        override fun decode(r: UndraReader): Short = r.readI16()
    }

    /** `u32`: four bytes, little-endian. */
    public val u32: UndraCodec<UInt> = object : UndraCodec<UInt> {
        override fun encode(w: UndraWriter, v: UInt) = w.writeU32(v)
        override fun decode(r: UndraReader): UInt = r.readU32()
    }

    /** `i32`: four bytes, little-endian. */
    public val i32: UndraCodec<Int> = object : UndraCodec<Int> {
        override fun encode(w: UndraWriter, v: Int) = w.writeI32(v)
        override fun decode(r: UndraReader): Int = r.readI32()
    }

    /** `u64`: eight bytes, little-endian. */
    public val u64: UndraCodec<ULong> = object : UndraCodec<ULong> {
        override fun encode(w: UndraWriter, v: ULong) = w.writeU64(v)
        override fun decode(r: UndraReader): ULong = r.readU64()
    }

    /** `i64`: eight bytes, little-endian. */
    public val i64: UndraCodec<Long> = object : UndraCodec<Long> {
        override fun encode(w: UndraWriter, v: Long) = w.writeI64(v)
        override fun decode(r: UndraReader): Long = r.readI64()
    }

    /** `f32`: IEEE 754 bits, little-endian. NaN payloads round-trip bit-exactly. */
    public val f32: UndraCodec<Float> = object : UndraCodec<Float> {
        override fun encode(w: UndraWriter, v: Float) = w.writeF32(v)
        override fun decode(r: UndraReader): Float = r.readF32()
    }

    /** `f64`: IEEE 754 bits, little-endian. NaN payloads round-trip bit-exactly. */
    public val f64: UndraCodec<Double> = object : UndraCodec<Double> {
        override fun encode(w: UndraWriter, v: Double) = w.writeF64(v)
        override fun decode(r: UndraReader): Double = r.readF64()
    }

    /** `Unit`: zero bytes. */
    public val unit: UndraCodec<Unit> = object : UndraCodec<Unit> {
        override fun encode(w: UndraWriter, v: Unit) = Unit
        override fun decode(r: UndraReader) = Unit
    }

    /**
     * `String`: `u32` byte length then UTF-8. Decoding validates strictly; encoding rejects strings
     * with unpaired surrogates ([IllegalArgumentException]) instead of silently substituting `?`.
     */
    public val string: UndraCodec<String> = object : UndraCodec<String> {
        override fun encode(w: UndraWriter, v: String) = w.writeStr(v)
        override fun decode(r: UndraReader): String = r.readStr()
    }

    /**
     * `Bytes`: `u32` length then raw bytes. Decoding returns a copy. Note that `ByteArray` has
     * identity equality in Kotlin, so records holding one need to compare it with `contentEquals`.
     */
    public val bytes: UndraCodec<ByteArray> = object : UndraCodec<ByteArray> {
        override fun encode(w: UndraWriter, v: ByteArray) = w.writeBytes(v)
        override fun decode(r: UndraReader): ByteArray = r.readBytes()
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
    public val duration: UndraCodec<Duration> = object : UndraCodec<Duration> {
        override fun encode(w: UndraWriter, v: Duration) {
            if (v.isNegative()) throw WireException.NegativeDuration(v.inWholeNanoseconds, w.size)
            require(!v.isInfinite() && v.inWholeMilliseconds <= MAX_DURATION_MILLIS) {
                "duration $v does not fit in i64 nanoseconds"
            }
            w.writeI64(v.inWholeNanoseconds)
        }

        override fun decode(r: UndraReader): Duration {
            val at = r.position
            val nanos = r.readI64()
            if (nanos < 0) throw WireException.NegativeDuration(nanos, at)
            return nanos.nanoseconds
        }
    }

    /** `Timestamp`: `i64` milliseconds since the Unix epoch. */
    public val timestamp: UndraCodec<Timestamp> = object : UndraCodec<Timestamp> {
        override fun encode(w: UndraWriter, v: Timestamp) = w.writeI64(v.epochMillis)
        override fun decode(r: UndraReader): Timestamp = Timestamp(r.readI64())
    }

    /** `Uuid`: 16 raw bytes, big-endian as in RFC 4122 (the most significant half first). */
    public val uuid: UndraCodec<UUID> = object : UndraCodec<UUID> {
        override fun encode(w: UndraWriter, v: UUID) {
            w.writeI64BigEndian(v.mostSignificantBits)
            w.writeI64BigEndian(v.leastSignificantBits)
        }

        override fun decode(r: UndraReader): UUID {
            val msb = r.readI64BigEndian()
            val lsb = r.readI64BigEndian()
            return UUID(msb, lsb)
        }
    }

    /**
     * `Decimal` (ADR-042): a 128-bit two's-complement mantissa in 16 little-endian bytes, then a
     * scale byte of at most 38; value = mantissa x 10^-scale, as a [BigDecimal] of
     * `BigDecimal(BigInteger, scale)` (exact, and `equals` is scale-sensitive, as the wire
     * encoding is: `1.0` and `1.00` are different values to `equals`, the same to `compareTo`).
     *
     * Decoding is exact. Encoding is exact for every value the wire can hold; the rest is
     * documented, not hidden: a negative scale (`1E+3`) becomes whole digits (scale 0), digits past
     * the 38th after the point are rounded half up, and a value whose mantissa needs more than 128
     * bits saturates at the largest or smallest mantissa.
     */
    public val decimal: UndraCodec<BigDecimal> = object : UndraCodec<BigDecimal> {
        override fun encode(w: UndraWriter, v: BigDecimal) {
            var value = v
            if (value.scale() < 0) value = value.setScale(0)
            if (value.scale() > MAX_DECIMAL_SCALE) value = value.setScale(MAX_DECIMAL_SCALE, RoundingMode.HALF_UP)
            var mantissa = value.unscaledValue()
            var scale = value.scale()
            if (mantissa.bitLength() > 127) {
                mantissa = if (mantissa.signum() < 0) I128_MIN else I128_MAX
                scale = 0
            }
            w.writeI64(mantissa.toLong())
            w.writeI64(mantissa.shiftRight(64).toLong())
            w.writeU8(scale.toUByte())
        }

        override fun decode(r: UndraReader): BigDecimal {
            val low = r.readI64()
            val high = r.readI64()
            val at = r.position
            val scale = r.readU8()
            if (scale > MAX_DECIMAL_SCALE.toUByte()) throw WireException.InvalidTag(scale.toUInt(), at, "decimal scale")
            val unsignedLow = if (low >= 0) BigInteger.valueOf(low) else BigInteger.valueOf(low).add(TWO_POW_64)
            val mantissa = BigInteger.valueOf(high).shiftLeft(64).add(unsignedLow)
            return BigDecimal(mantissa, scale.toInt())
        }
    }

    /**
     * Object handle: the raw `u64` (see [Handle]; wrap with `Handle(raw)` to inspect index and
     * generation).
     */
    public val handle: UndraCodec<Long> = i64

    /**
     * `Option<T>`: a tag byte (0 = `None`, 1 = `Some`) followed by the item. Represented as `T?`, so
     * `T` must be non-null: nested options (`Option<Option<T>>`) would collapse and are therefore
     * rejected by the type system.
     */
    public fun <T : Any> option(item: UndraCodec<T>): UndraCodec<T?> = OptionCodec(item)

    /**
     * `Vec<T>`: a `u32` count followed by the items. Decoding preallocates exactly `count` slots
     * after checking that the count fits in the remaining bytes, so every item must encode to at
     * least one byte: a `Vec` of `Unit` (or of another zero-width type) with a count larger than the
     * remaining input is rejected with [WireException.LengthTooLarge].
     */
    public fun <T> vec(item: UndraCodec<T>): UndraCodec<List<T>> = VecCodec(item)

    /**
     * `Map<K, V>`: a `u32` count followed by (key, value) pairs.
     *
     * Encoding sorts the pairs by the **encoded key bytes** (unsigned, lexicographic) so that equal
     * maps always produce identical bytes regardless of iteration order; note that for strings this
     * orders by length prefix first. Encoding rejects keys with byte-identical encodings and decoding
     * rejects repeated keys, both with [WireException.DuplicateKey]. Decoding accepts any pair order
     * and returns a [LinkedHashMap] that preserves it.
     */
    public fun <K, V> map(key: UndraCodec<K>, value: UndraCodec<V>): UndraCodec<Map<K, V>> = MapCodec(key, value)

    /** `Result<T, E>`: tag 0 = [UndraResult.Ok] + `T`, tag 1 = [UndraResult.Err] + `E`. */
    public fun <T, E> result(ok: UndraCodec<T>, err: UndraCodec<E>): UndraCodec<UndraResult<T, E>> = ResultCodec(ok, err)

    /** `Duration` values above this many whole milliseconds cannot be expressed in i64 nanoseconds. */
    private const val MAX_DURATION_MILLIS: Long = Long.MAX_VALUE / 1_000_000L
}

private class OptionCodec<T : Any>(private val item: UndraCodec<T>) : UndraCodec<T?> {
    override fun encode(w: UndraWriter, v: T?) {
        if (v == null) {
            w.writeU8(0u)
        } else {
            w.writeU8(1u)
            item.encode(w, v)
        }
    }

    override fun decode(r: UndraReader): T? {
        val at = r.position
        return when (val tag = r.readU8().toInt()) {
            0 -> null
            1 -> item.decode(r)
            else -> throw WireException.InvalidTag(tag.toUInt(), at, "Option")
        }
    }
}

private class VecCodec<T>(private val item: UndraCodec<T>) : UndraCodec<List<T>> {
    override fun encode(w: UndraWriter, v: List<T>) {
        val n = v.size
        w.writeLen(n)
        if (v is RandomAccess) {
            for (i in 0 until n) item.encode(w, v[i])
        } else {
            for (x in v) item.encode(w, x)
        }
    }

    override fun decode(r: UndraReader): List<T> {
        val n = r.readLen()
        val out = ArrayList<T>(n)
        for (i in 0 until n) out.add(item.decode(r))
        return out
    }
}

private class MapCodec<K, V>(
    private val key: UndraCodec<K>,
    private val value: UndraCodec<V>,
) : UndraCodec<Map<K, V>> {

    override fun encode(w: UndraWriter, v: Map<K, V>) {
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
        val scratch = UndraWriter()
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

    override fun decode(r: UndraReader): Map<K, V> {
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
    private val ok: UndraCodec<T>,
    private val err: UndraCodec<E>,
) : UndraCodec<UndraResult<T, E>> {
    override fun encode(w: UndraWriter, v: UndraResult<T, E>) {
        when (v) {
            is UndraResult.Ok -> {
                w.writeU8(0u)
                ok.encode(w, v.value)
            }
            is UndraResult.Err -> {
                w.writeU8(1u)
                err.encode(w, v.error)
            }
        }
    }

    override fun decode(r: UndraReader): UndraResult<T, E> {
        val at = r.position
        return when (val tag = r.readU8().toInt()) {
            0 -> UndraResult.Ok(ok.decode(r))
            1 -> UndraResult.Err(err.decode(r))
            else -> throw WireException.InvalidTag(tag.toUInt(), at, "Result")
        }
    }
}

private const val MAX_DECIMAL_SCALE = 38
private val TWO_POW_64: BigInteger = BigInteger.ONE.shiftLeft(64)
private val I128_MAX: BigInteger = BigInteger.ONE.shiftLeft(127).subtract(BigInteger.ONE)
private val I128_MIN: BigInteger = BigInteger.ONE.shiftLeft(127).negate()
