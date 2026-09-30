package dev.keel.runtime.testing

import java.math.BigInteger

/**
 * A JSON value exactly as it appears in `contract-tests/wire-vectors.json`. Numbers keep their
 * source text so 64-bit integers and floats lose nothing; object entries keep their source order
 * (the map vector relies on that to prove the encoder sorts).
 */
sealed class JV {
    object Null : JV()

    class Bool(val value: Boolean) : JV()

    class Num(val text: String) : JV()

    class Str(val value: String) : JV()

    class Arr(val items: List<JV>) : JV()

    class Obj(val entries: List<Pair<String, JV>>) : JV() {
        operator fun get(key: String): JV =
            entries.firstOrNull { it.first == key }?.second ?: fail("vector object has no field '$key'")

        fun has(key: String): Boolean = entries.any { it.first == key }
    }

    /** Integer given either as a JSON number or, for 64-bit values, as a decimal string. */
    fun asBigInteger(): BigInteger = when (this) {
        is Num -> BigInteger(text)
        is Str -> BigInteger(value)
        else -> fail("not an integer: $this")
    }

    fun asLong(): Long = asBigInteger().toLong()

    fun asInt(): Int = asBigInteger().toInt()

    fun asULong(): ULong = asBigInteger().toLong().toULong()

    fun asDouble(): Double = when (this) {
        is Num -> text.toDouble()
        else -> fail("not a number: $this")
    }

    fun asString(): String = (this as? Str)?.value ?: fail("not a string: $this")

    fun asBool(): Boolean = (this as? Bool)?.value ?: fail("not a bool: $this")

    fun asList(): List<JV> = (this as? Arr)?.items ?: fail("not an array: $this")

    fun asObj(): Obj = this as? Obj ?: fail("not an object: $this")
}

/** One entry of `wire-vectors.json`: `hex` is the canonical encoding of `value` for `type`. */
class WireVector(
    val name: String,
    val type: String,
    val value: JV,
    val hex: String,
    val note: String,
)
