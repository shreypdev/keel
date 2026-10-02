package dev.undra.testkit

/** The JSON the kit reads: recordings and seeds. Not a general-purpose parser (no streaming, no big numbers), and the kit has no dependency for it. */
internal sealed interface Json {
    class Obj(val fields: Map<String, Json>) : Json
    class Arr(val items: List<Json>) : Json
    class Str(val value: String) : Json
    class Num(val raw: String) : Json
    class Bool(val value: Boolean) : Json
    data object Null : Json
}

internal class JsonException(message: String) : RuntimeException(message)

internal fun parseJson(text: String): Json = JsonParser(text).parseDocument()

private class JsonParser(private val s: String) {
    private var i = 0

    fun parseDocument(): Json {
        val v = value()
        ws()
        if (i != s.length) fail("unexpected data after the document")
        return v
    }

    private fun fail(message: String): Nothing = throw JsonException("$message (at offset $i)")

    private fun ws() {
        while (i < s.length && (s[i] == ' ' || s[i] == '\n' || s[i] == '\r' || s[i] == '\t')) i++
    }

    private fun value(): Json {
        ws()
        if (i >= s.length) fail("unexpected end of input")
        return when (val c = s[i]) {
            '{' -> obj()
            '[' -> arr()
            '"' -> Json.Str(string())
            't' -> literal("true", Json.Bool(true))
            'f' -> literal("false", Json.Bool(false))
            'n' -> literal("null", Json.Null)
            else -> if (c == '-' || c in '0'..'9') number() else fail("unexpected character '$c'")
        }
    }

    private fun literal(word: String, v: Json): Json {
        if (!s.startsWith(word, i)) fail("expected $word")
        i += word.length
        return v
    }

    private fun number(): Json {
        val start = i
        if (s[i] == '-') i++
        while (i < s.length && (s[i].isDigit() || s[i] == '.' || s[i] == 'e' || s[i] == 'E' || s[i] == '+' || s[i] == '-')) i++
        if (i == start) fail("expected a number")
        return Json.Num(s.substring(start, i))
    }

    private fun obj(): Json {
        i++
        val fields = LinkedHashMap<String, Json>()
        ws()
        if (i < s.length && s[i] == '}') {
            i++
            return Json.Obj(fields)
        }
        while (true) {
            ws()
            if (i >= s.length || s[i] != '"') fail("expected a string key")
            val key = string()
            ws()
            if (i >= s.length || s[i] != ':') fail("expected ':'")
            i++
            fields[key] = value()
            ws()
            if (i >= s.length) fail("unterminated object")
            if (s[i] == ',') {
                i++
                continue
            }
            if (s[i] == '}') {
                i++
                return Json.Obj(fields)
            }
            fail("expected ',' or '}'")
        }
    }

    private fun arr(): Json {
        i++
        val items = ArrayList<Json>()
        ws()
        if (i < s.length && s[i] == ']') {
            i++
            return Json.Arr(items)
        }
        while (true) {
            items += value()
            ws()
            if (i >= s.length) fail("unterminated array")
            if (s[i] == ',') {
                i++
                continue
            }
            if (s[i] == ']') {
                i++
                return Json.Arr(items)
            }
            fail("expected ',' or ']'")
        }
    }

    private fun string(): String {
        i++
        val out = StringBuilder()
        while (true) {
            if (i >= s.length) fail("unterminated string")
            val c = s[i++]
            when {
                c == '"' -> return out.toString()
                c == '\\' -> {
                    if (i >= s.length) fail("unterminated escape")
                    when (val e = s[i++]) {
                        '"' -> out.append('"')
                        '\\' -> out.append('\\')
                        '/' -> out.append('/')
                        'b' -> out.append('\b')
                        'f' -> out.append('\u000c')
                        'n' -> out.append('\n')
                        'r' -> out.append('\r')
                        't' -> out.append('\t')
                        'u' -> {
                            if (i + 4 > s.length) fail("short \\u escape")
                            out.append(s.substring(i, i + 4).toIntOrNull(16)?.toChar() ?: fail("bad \\u escape"))
                            i += 4
                        }
                        else -> fail("bad escape '\\$e'")
                    }
                }
                c < ' ' -> fail("control character in a string")
                else -> out.append(c)
            }
        }
    }
}

/** A JSON string escaped only where JSON requires, so every language's writer gives the same bytes. */
internal fun jsonString(text: String): String {
    val out = StringBuilder("\"")
    for (ch in text) {
        when {
            ch == '"' -> out.append("\\\"")
            ch == '\\' -> out.append("\\\\")
            ch == '\n' -> out.append("\\n")
            ch == '\r' -> out.append("\\r")
            ch == '\t' -> out.append("\\t")
            ch < ' ' -> out.append("\\u").append(ch.code.toString(16).padStart(4, '0'))
            else -> out.append(ch)
        }
    }
    return out.append('"').toString()
}

internal fun Json.obj(what: String): Map<String, Json> = (this as? Json.Obj)?.fields ?: throw JsonException("$what must be an object")
