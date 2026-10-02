package dev.undra.testkit

private const val DIGITS = "0123456789abcdef"

/** [this] as lower-case hex, the encoding of every payload in a recording. */
internal fun ByteArray.toHex(): String {
    val out = StringBuilder(size * 2)
    for (b in this) {
        val v = b.toInt() and 0xff
        out.append(DIGITS[v shr 4]).append(DIGITS[v and 15])
    }
    return out.toString()
}

/** Hex (either case) back to bytes; `null` for an odd length or a character that is not a hex digit. */
internal fun String.fromHex(): ByteArray? {
    if (length % 2 != 0) return null
    val out = ByteArray(length / 2)
    for (i in out.indices) {
        val high = Character.digit(this[2 * i], 16)
        val low = Character.digit(this[2 * i + 1], 16)
        if (high < 0 || low < 0) return null
        out[i] = (high * 16 + low).toByte()
    }
    return out
}

/**
 * Orders strings by their UTF-8 bytes (which is code-point order), as the Rust fakes and the platform adapters do. Kotlin's
 * `compareTo` compares UTF-16 code units, which disagrees for characters outside the Basic Multilingual Plane.
 */
internal fun compareUtf8(a: String, b: String): Int {
    var i = 0
    var j = 0
    while (i < a.length && j < b.length) {
        val p = a.codePointAt(i)
        val q = b.codePointAt(j)
        if (p != q) return if (p < q) -1 else 1
        i += Character.charCount(p)
        j += Character.charCount(q)
    }
    return (a.length - i).compareTo(b.length - j)
}

/** The comparator of [compareUtf8]. */
internal val utf8Order: Comparator<String> = Comparator { a, b -> compareUtf8(a, b) }

/** `0x` and sixteen lower-case hex digits of an unsigned 64-bit value. */
internal fun hex64(value: ULong): String = "0x" + value.toString(16).padStart(16, '0')

/** [handle] (a signed long holding an unsigned handle) as `0x` and sixteen digits. */
internal fun hex64(handle: Long): String = hex64(handle.toULong())

/** Parses `0x..`; `null` if [text] is not that. */
internal fun parseHex64(text: String): ULong? {
    if (!text.startsWith("0x") || text.length < 3 || text.length > 18) return null
    return text.substring(2).toULongOrNull(16)
}
