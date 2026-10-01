package dev.undra.android

import dev.undra.runtime.wire.UndraReader

/** The method table of a [dev.undra.runtime.PortImpl]: a function from the encoded arguments to the encoded reply, by method id. */
internal typealias PortMethods = Map<UInt, suspend (ByteArray) -> ByteArray>

/**
 * Builds a [PortMethods] as `portMethods { this[StandardPorts.Kv.GET] = { args -> reply } }`. The expected type is what
 * makes the lambdas `suspend`: `mapOf(id to { args -> reply })` infers plain lambdas, which `PortImpl` does not take.
 */
internal fun portMethods(fill: MutableMap<UInt, suspend (ByteArray) -> ByteArray>.() -> Unit): PortMethods =
    LinkedHashMap<UInt, suspend (ByteArray) -> ByteArray>().apply(fill)

/** The reply of a port method that returns nothing. */
internal val NO_REPLY: ByteArray = ByteArray(0)

/** Decodes the arguments of a port method with [read] and requires that nothing is left over. */
internal inline fun <T> readArgs(args: ByteArray, read: (UndraReader) -> T): T {
    val reader = UndraReader(args)
    val value = read(reader)
    reader.finish()
    return value
}
