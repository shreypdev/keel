package dev.keel.contract

/**
 * A small JSON reader for the two documents the core exports as JSON (`keel_stats_json` and
 * `keel_schema_json`), which the runtime hands over as text. Objects become `Map<String, Any?>`,
 * arrays `List<Any?>`, integers `Long` (or `ULong` above `Long.MAX_VALUE`), other numbers `Double`.
 * The runner has no JSON dependency, like the runtime.
 */
object Json {
    /** Parses [text] as one JSON value. */
    fun parse(text: String): Any? {
        val p = Parser(text)
        val value = p.value()
        p.skipSpace()
        require(p.atEnd()) { "trailing characters after the JSON value" }
        return value
    }

    /** Parses [text], which must be an object. */
    @Suppress("UNCHECKED_CAST")
    fun parseObject(text: String): Map<String, Any?> =
        parse(text) as? Map<String, Any?> ?: throw IllegalArgumentException("not a JSON object: ${text.take(80)}")

    private class Parser(private val s: String) {
        private var i = 0

        fun atEnd(): Boolean = i >= s.length

        fun skipSpace() {
            while (i < s.length && s[i].isWhitespace()) i++
        }

        fun value(): Any? {
            skipSpace()
            require(i < s.length) { "unexpected end of JSON" }
            return when (val c = s[i]) {
                '{' -> obj()
                '[' -> arr()
                '"' -> str()
                't' -> literal("true", true)
                'f' -> literal("false", false)
                'n' -> literal("null", null)
                else -> if (c == '-' || c.isDigit()) number() else throw IllegalArgumentException("unexpected '$c' at $i")
            }
        }

        private fun obj(): Map<String, Any?> {
            val out = LinkedHashMap<String, Any?>()
            i++
            skipSpace()
            if (s[i] == '}') {
                i++
                return out
            }
            while (true) {
                skipSpace()
                val key = str()
                skipSpace()
                require(s[i++] == ':') { "expected ':' at ${i - 1}" }
                out[key] = value()
                skipSpace()
                when (s[i++]) {
                    ',' -> Unit
                    '}' -> return out
                    else -> throw IllegalArgumentException("expected ',' or '}' at ${i - 1}")
                }
            }
        }

        private fun arr(): List<Any?> {
            val out = ArrayList<Any?>()
            i++
            skipSpace()
            if (s[i] == ']') {
                i++
                return out
            }
            while (true) {
                out.add(value())
                skipSpace()
                when (s[i++]) {
                    ',' -> Unit
                    ']' -> return out
                    else -> throw IllegalArgumentException("expected ',' or ']' at ${i - 1}")
                }
            }
        }

        private fun str(): String {
            require(s[i] == '"') { "expected a string at $i" }
            i++
            val sb = StringBuilder()
            while (true) {
                val c = s[i++]
                when (c) {
                    '"' -> return sb.toString()
                    '\\' -> when (val e = s[i++]) {
                        'n' -> sb.append('\n')
                        't' -> sb.append('\t')
                        'r' -> sb.append('\r')
                        'b' -> sb.append('\b')
                        'f' -> sb.append('\u000c')
                        'u' -> {
                            sb.append(s.substring(i, i + 4).toInt(16).toChar())
                            i += 4
                        }
                        else -> sb.append(e)
                    }
                    else -> sb.append(c)
                }
            }
        }

        private fun number(): Any {
            val start = i
            while (i < s.length && (s[i].isDigit() || s[i] in "+-.eE")) i++
            val text = s.substring(start, i)
            return text.toLongOrNull() ?: text.toULongOrNull() ?: text.toDouble()
        }

        private fun literal(word: String, result: Any?): Any? {
            require(s.startsWith(word, i)) { "bad literal at $i" }
            i += word.length
            return result
        }
    }
}
