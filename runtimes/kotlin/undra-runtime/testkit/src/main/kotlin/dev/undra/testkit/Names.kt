package dev.undra.testkit

import dev.undra.runtime.wire.Fnv

/** `(trait, methods)` of the standard ports (docs/SPEC.md section 8; `Diagnostics` is ADR-046's). */
private val STANDARD: List<Pair<String, List<String>>> = listOf(
    "Clock" to listOf("now_ms", "monotonic_ns"),
    "Rng" to listOf("fill"),
    "Log" to listOf("log"),
    "Http" to listOf("request"),
    "Kv" to listOf("get", "set", "delete", "list"),
    "SecureStore" to listOf("get", "set", "delete", "list"),
    "Fs" to listOf("read", "write", "delete", "list"),
    "Timer" to listOf("set"),
    "Connectivity" to listOf("changed"),
    "Lifecycle" to listOf("changed"),
    "Diagnostics" to listOf("panicked"),
)

/** `"Http.request"` for the standard port method ([port], [method]), `null` for anything else. The ids stay authoritative; the name is for the person reading a recording. */
public fun standardName(port: UInt, method: UInt): String? {
    for ((name, methods) in STANDARD) {
        if (Fnv.fnv1a32("port.$name") != port) continue
        for (m in methods) if (Fnv.fnv1a32("$name.$m") == method) return "$name.$m"
    }
    return null
}

/** The port id of `Trait`: `Fnv.fnv1a32("port.<Trait>")`. */
public fun portId(trait: String): UInt = Fnv.fnv1a32("port.$trait")

/** The method id of `Trait.method`: `Fnv.fnv1a32("<Trait>.<method>")`. */
public fun methodId(trait: String, method: String): UInt = Fnv.fnv1a32("$trait.$method")
