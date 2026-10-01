package dev.undra.contract

/** The method table of a `PortImpl`: a function from the encoded arguments to the encoded reply, by method id. */
typealias PortMethods = Map<UInt, suspend (ByteArray) -> ByteArray>

/**
 * Builds a [PortMethods] as `portMethods { this[StandardPorts.Kv.GET] = { args -> reply } }`. The expected
 * type is what makes the lambdas `suspend`: `mapOf(id to { args -> reply })` infers plain lambdas (Kotlin
 * 2.0), which `PortImpl` does not take.
 */
fun portMethods(fill: MutableMap<UInt, suspend (ByteArray) -> ByteArray>.() -> Unit): PortMethods =
    LinkedHashMap<UInt, suspend (ByteArray) -> ByteArray>().apply(fill)
