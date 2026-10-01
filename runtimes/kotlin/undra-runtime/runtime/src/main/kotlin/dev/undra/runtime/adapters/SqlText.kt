package dev.undra.runtime.adapters

/**
 * SQL text as SQLite's tokenizer sees it, for [DbAdapter]s whose SQLite API compiles only the first statement of a string
 * and does not say where it ended (JDBC, `android.database.sqlite`): where statements end, and how many parameters one has.
 *
 * Strings (`'...'` with `''`), quoted identifiers (`"..."`, `` `...` ``, `[...]`) and comments (`-- ...`, `/* ... */`) are
 * skipped; a `;` ends a statement, except inside the body of a `CREATE TRIGGER`, which ends at `; END ;` (the rule of
 * `sqlite3_complete`).
 */
public object SqlText {
    private enum class Kind { WORD, SEMI, OTHER }

    private class Token(val kind: Kind, val start: Int, val end: Int)

    /**
     * The statements of [sql] in order, each without its terminating `;` and without the whitespace and comments around it.
     * Empty statements (`;;`, only comments) are left out.
     */
    public fun statements(sql: String): List<String> {
        val out = ArrayList<String>()
        var first = -1
        var last = -1
        var words = 0
        var trigger = false
        var previous: Token? = null
        var beforePrevious: Token? = null
        fun close() {
            if (first >= 0) out.add(sql.substring(first, last))
            first = -1
            last = -1
            words = 0
            trigger = false
        }
        tokens(sql) { token ->
            if (token.kind == Kind.SEMI) {
                // Inside a trigger body only `; END ;` ends the statement.
                val endsTrigger = previous?.let { isWord(sql, it, "END") } == true && beforePrevious?.kind == Kind.SEMI
                if (!trigger || endsTrigger) {
                    close()
                    previous = null
                    beforePrevious = null
                    return@tokens
                }
            } else {
                if (first < 0) first = token.start
                if (token.kind == Kind.WORD && words < 4) {
                    // CREATE [TEMP | TEMPORARY] TRIGGER, possibly after EXPLAIN [QUERY PLAN].
                    words++
                    val before = previous
                    if (isWord(sql, token, "TRIGGER") && before != null &&
                        (isWord(sql, before, "CREATE") || isWord(sql, before, "TEMP") || isWord(sql, before, "TEMPORARY"))
                    ) {
                        trigger = true
                    }
                }
            }
            if (first >= 0) last = token.end
            beforePrevious = previous
            previous = token
        }
        close()
        return out
    }

    /**
     * The number of parameters of [statement] as `sqlite3_bind_parameter_count` reports it: the largest index, where `?NNN`
     * has index NNN, a plain `?` the next index after the largest so far, and a named parameter (`:a`, `@a`, `$a`) the index
     * of its first appearance.
     */
    public fun parameterCount(statement: String): Int {
        var largest = 0
        val named = HashMap<String, Int>()
        tokens(statement, parameters = { text ->
            when {
                text == "?" -> largest++
                text.startsWith("?") -> largest = maxOf(largest, text.substring(1).toIntOrNull() ?: 0)
                else -> if (text !in named) {
                    largest++
                    named[text] = largest
                }
            }
        }) {}
        return largest
    }

    private fun isWord(sql: String, token: Token, word: String): Boolean =
        token.kind == Kind.WORD && token.end - token.start == word.length && sql.regionMatches(token.start, word, 0, word.length, ignoreCase = true)

    private fun isWordChar(c: Char): Boolean = c.isLetterOrDigit() || c == '_' || c == '$' || c.code >= 0x80

    /** Calls [emit] with every token of [sql] (comments and whitespace are not tokens) and [parameters] with every parameter's text. */
    private inline fun tokens(sql: String, noinline parameters: ((String) -> Unit)? = null, emit: (Token) -> Unit) {
        var i = 0
        val n = sql.length
        while (i < n) {
            val c = sql[i]
            when {
                c.isWhitespace() -> i++
                c == '-' && i + 1 < n && sql[i + 1] == '-' -> {
                    while (i < n && sql[i] != '\n') i++
                }
                c == '/' && i + 1 < n && sql[i + 1] == '*' -> {
                    val close = sql.indexOf("*/", i + 2)
                    i = if (close < 0) n else close + 2
                }
                c == '\'' || c == '"' || c == '`' -> {
                    val start = i
                    i++
                    while (i < n) {
                        if (sql[i] == c) {
                            if (i + 1 < n && sql[i + 1] == c) {
                                i += 2
                                continue
                            }
                            i++
                            break
                        }
                        i++
                    }
                    emit(Token(if (c == '\'') Kind.OTHER else Kind.WORD, start, i))
                }
                c == '[' -> {
                    val start = i
                    val close = sql.indexOf(']', i + 1)
                    i = if (close < 0) n else close + 1
                    emit(Token(Kind.WORD, start, i))
                }
                c == ';' -> {
                    emit(Token(Kind.SEMI, i, i + 1))
                    i++
                }
                c == '?' -> {
                    val start = i
                    i++
                    while (i < n && sql[i].isDigit()) i++
                    parameters?.invoke(sql.substring(start, i))
                    emit(Token(Kind.OTHER, start, i))
                }
                (c == ':' || c == '@' || c == '$') && i + 1 < n && isWordChar(sql[i + 1]) && sql[i + 1] != '$' -> {
                    val start = i
                    i++
                    while (i < n && (isWordChar(sql[i]) || (c == '$' && sql[i] == ':' && i + 1 < n && sql[i + 1] == ':'))) {
                        i += if (sql[i] == ':') 2 else 1
                    }
                    parameters?.invoke(sql.substring(start, i))
                    emit(Token(Kind.OTHER, start, i))
                }
                isWordChar(c) -> {
                    val start = i
                    while (i < n && isWordChar(sql[i])) i++
                    emit(Token(Kind.WORD, start, i))
                }
                else -> {
                    emit(Token(Kind.OTHER, i, i + 1))
                    i++
                }
            }
        }
    }
}
