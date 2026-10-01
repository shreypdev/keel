package dev.undra.runtime

/**
 * A snapshot of the core's counters (`undra_stats_json`, SPEC section 6) and of the host side of the
 * link, from [UndraCore.stats].
 *
 * The core-side numbers are `-1` when the core cannot be asked: over a remote transport there is no
 * request for statistics, so a `Mode.REMOTE` core reports only the host-side fields.
 *
 * @property liveHandles objects in the core's object table (stores and plain objects). Tests use it to
 *   check that nothing leaks: it returns to its earlier value once every object is closed.
 * @property liveStores how many of those are stores.
 * @property tasks tasks alive in the core's executor.
 * @property activeCalls host calls the core is still working on.
 * @property openStreams streams that have not ended.
 * @property pendingPortCalls port calls the core is waiting on the host for.
 * @property pendingTimers timers set but not yet fired.
 * @property transactions change-sets emitted so far.
 * @property panics panics the core caught.
 * @property hostPendingCalls calls this host sent and has no reply for yet (including open streams).
 * @property hostMirrorHandles stores registered with the mirror.
 * @property raw the core's statistics document exactly as received (`"{}"` when unavailable).
 * @property mirror the mirror's delivery counters: change-sets and entries received, entries applied after
 *   merging, drains, compactions, resyncs, the backlog (all zero when not reported).
 */
public class UndraStats(
    public val liveHandles: Int,
    public val liveStores: Int = UNKNOWN,
    public val tasks: Int = UNKNOWN,
    public val activeCalls: Int = UNKNOWN,
    public val openStreams: Int = UNKNOWN,
    public val pendingPortCalls: Int = UNKNOWN,
    public val pendingTimers: Int = UNKNOWN,
    public val transactions: Long = UNKNOWN.toLong(),
    public val panics: Long = UNKNOWN.toLong(),
    public val hostPendingCalls: Int = 0,
    public val hostMirrorHandles: Int = 0,
    public val raw: String = "{}",
    public val mirror: MirrorStats = NO_MIRROR_STATS,
) {
    override fun toString(): String =
        "UndraStats(liveHandles=$liveHandles, liveStores=$liveStores, tasks=$tasks, activeCalls=$activeCalls, " +
            "openStreams=$openStreams, pendingPortCalls=$pendingPortCalls, pendingTimers=$pendingTimers, " +
            "transactions=$transactions, panics=$panics, hostPendingCalls=$hostPendingCalls, hostMirrorHandles=$hostMirrorHandles, " +
            "mirror=$mirror)"

    /** The marker for numbers that are not known. */
    public companion object {
        /** The value of a core-side number that is not known. */
        public const val UNKNOWN: Int = -1

        /**
         * Reads the core's statistics document. Unknown fields are ignored and missing ones are
         * [UNKNOWN]; a document that is not a JSON object yields an all-unknown result carrying [json] as [raw].
         */
        internal fun fromCoreJson(
            json: String,
            hostPendingCalls: Int,
            hostMirrorHandles: Int,
            mirror: MirrorStats = NO_MIRROR_STATS,
        ): UndraStats {
            val doc = MiniJson.parseObject(json) ?: return UndraStats(
                liveHandles = UNKNOWN,
                hostPendingCalls = hostPendingCalls,
                hostMirrorHandles = hostMirrorHandles,
                raw = json,
                mirror = mirror,
            )
            fun int(key: String): Int = (doc[key] as? Long)?.coerceIn(0L, Int.MAX_VALUE.toLong())?.toInt() ?: UNKNOWN
            fun long(key: String): Long = (doc[key] as? Long) ?: UNKNOWN.toLong()
            return UndraStats(
                liveHandles = int("live_handles"),
                liveStores = int("live_stores"),
                tasks = int("tasks"),
                activeCalls = int("active_calls"),
                openStreams = int("open_streams"),
                pendingPortCalls = int("pending_port_calls"),
                pendingTimers = int("pending_timers"),
                transactions = long("transactions"),
                panics = long("panics"),
                hostPendingCalls = hostPendingCalls,
                hostMirrorHandles = hostMirrorHandles,
                raw = json,
                mirror = mirror,
            )
        }
    }
}

/**
 * The little JSON the statistics document needs: objects, strings, integers, and skipping over
 * everything else. Not a general parser; it is here because the runtime has no dependencies.
 */
internal object MiniJson {
    /** Parses [text] as one JSON object into a map of `String`, `Long` (integers), `Map` (objects) or `null` (anything else). Returns `null` on malformed input. */
    @Suppress("UNCHECKED_CAST")
    fun parseObject(text: String): Map<String, Any?>? {
        val p = Parser(text)
        return try {
            p.skipSpace()
            val value = p.value()
            p.skipSpace()
            if (!p.atEnd()) null else value as? Map<String, Any?>
        } catch (e: IllegalArgumentException) {
            null
        }
    }

    private class Parser(private val s: String) {
        private var i = 0

        fun atEnd(): Boolean = i >= s.length

        fun skipSpace() {
            while (i < s.length && s[i].isWhitespace()) i++
        }

        fun value(): Any? {
            skipSpace()
            require(i < s.length) { "unexpected end" }
            return when (val c = s[i]) {
                '{' -> obj()
                '"' -> string()
                '[' -> skipArray()
                't' -> literal("true", true)
                'f' -> literal("false", false)
                'n' -> literal("null", null)
                else -> if (c == '-' || c.isDigit()) number() else throw IllegalArgumentException("unexpected '$c'")
            }
        }

        private fun obj(): Map<String, Any?> {
            val out = LinkedHashMap<String, Any?>()
            i++ // {
            skipSpace()
            if (peek() == '}') {
                i++
                return out
            }
            while (true) {
                skipSpace()
                require(peek() == '"') { "expected a key" }
                val key = string()
                skipSpace()
                require(peek() == ':') { "expected ':'" }
                i++
                out[key] = value()
                skipSpace()
                when (peek()) {
                    ',' -> i++
                    '}' -> {
                        i++
                        return out
                    }
                    else -> throw IllegalArgumentException("expected ',' or '}'")
                }
            }
        }

        private fun skipArray(): Any? {
            i++ // [
            skipSpace()
            if (peek() == ']') {
                i++
                return null
            }
            while (true) {
                value()
                skipSpace()
                when (peek()) {
                    ',' -> i++
                    ']' -> {
                        i++
                        return null
                    }
                    else -> throw IllegalArgumentException("expected ',' or ']'")
                }
            }
        }

        private fun string(): String {
            i++ // opening quote
            val sb = StringBuilder()
            while (true) {
                require(i < s.length) { "unterminated string" }
                val c = s[i++]
                when (c) {
                    '"' -> return sb.toString()
                    '\\' -> {
                        require(i < s.length) { "unterminated escape" }
                        when (val e = s[i++]) {
                            '"', '\\', '/' -> sb.append(e)
                            'n' -> sb.append('\n')
                            't' -> sb.append('\t')
                            'r' -> sb.append('\r')
                            'b' -> sb.append('\b')
                            'f' -> sb.append('\u000c')
                            'u' -> {
                                require(i + 4 <= s.length) { "short \\u escape" }
                                sb.append(s.substring(i, i + 4).toInt(16).toChar())
                                i += 4
                            }
                            else -> throw IllegalArgumentException("bad escape")
                        }
                    }
                    else -> sb.append(c)
                }
            }
        }

        private fun number(): Any? {
            val start = i
            if (s[i] == '-') i++
            while (i < s.length && (s[i].isDigit() || s[i] == '.' || s[i] == 'e' || s[i] == 'E' || s[i] == '+' || s[i] == '-')) i++
            val text = s.substring(start, i)
            return text.toLongOrNull() ?: run {
                require(text.toDoubleOrNull() != null) { "bad number" }
                null // a non-integer: not needed by the statistics
            }
        }

        private fun literal(word: String, result: Any?): Any? {
            require(s.startsWith(word, i)) { "bad literal" }
            i += word.length
            return result
        }

        private fun peek(): Char {
            require(i < s.length) { "unexpected end" }
            return s[i]
        }
    }
}
