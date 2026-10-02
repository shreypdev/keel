package dev.undra.runtime

/**
 * The rule a core's namespace follows (`undra.toml` `[core] namespace`, SPEC 13): a lowercase letter, then lowercase letters,
 * digits and `_`, at most [MAX_LENGTH] bytes. The default stores are kept in directories, Keystore aliases and database
 * files named after it (ADR-044 amendment A), so what [LoadOptions.namespace] or an adapter's constructor is given is
 * checked before it can reach a path: `..`, `/`, a NUL, an empty or a long one is refused.
 */
public object CoreNamespace {
    /** The longest namespace, in bytes. */
    public const val MAX_LENGTH: Int = 32

    /** What is wrong with [namespace], in words, or `null` when it is a core namespace. */
    public fun problem(namespace: String): String? {
        if (namespace.isEmpty()) return "it is empty"
        val bytes = namespace.toByteArray(Charsets.UTF_8)
        if (bytes.size > MAX_LENGTH) return "it is ${bytes.size} bytes long, and a namespace has at most $MAX_LENGTH"
        if (namespace[0] !in 'a'..'z') return "it must start with a lowercase letter"
        val bad = namespace.firstOrNull { it !in 'a'..'z' && it !in '0'..'9' && it != '_' }
        if (bad != null) return "'\\u${bad.code.toString(16).padStart(4, '0')}' is not allowed (lowercase letters, digits and `_` only)"
        return null
    }

    /** [namespace] if it is a core namespace, else an [IllegalArgumentException] that says what is wrong. */
    public fun require(namespace: String): String {
        val problem = problem(namespace) ?: return namespace
        val shown = if (namespace.length > 40) namespace.take(40) + "..." else namespace
        throw IllegalArgumentException(
            "`${shown.map { if (it.code in 0x20..0x7e) it.toString() else "\\u" + it.code.toString(16).padStart(4, '0') }.joinToString("")}` " +
                "is not an Undra core namespace: $problem. It is the core's `UndraIds.NAMESPACE` (`[core] namespace` in undra.toml)",
        )
    }

    /**
     * [namespace] if the default stores of a core may be named after it: a core namespace, or [UndraCore.UNNAMED_NAMESPACE]
     * (what a core loaded without one has, so two such cores share their stores). An [IllegalArgumentException] otherwise.
     */
    public fun requireForStores(namespace: String): String =
        if (namespace == UndraCore.UNNAMED_NAMESPACE) namespace else require(namespace)
}
