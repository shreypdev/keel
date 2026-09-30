package dev.keel.runtime.support

/**
 * Builds a port-method map with the expected suspend function type spelled out.
 *
 * Kotlin 2.0 cannot adopt a plain lambda into `suspend (ByteArray) -> ByteArray` when the
 * expected type is hidden inside a bare `mapOf` argument (2.4 can); with the type on this
 * helper's signature, the conversion is the ordinary lambda adoption every 2.x supports.
 * The runtime's floor is Kotlin 2.0, so the tests go through here.
 */
internal fun portMethods(
    vararg methods: Pair<UInt, suspend (ByteArray) -> ByteArray>,
): Map<UInt, suspend (ByteArray) -> ByteArray> = mapOf(*methods)

/**
 * Pairs a method id with its handler, typing the lambda DIRECTLY on the parameter: unlike
 * `id to { .. }`, the expected suspend type reaches the lambda before any `Pair` inference
 * runs, which Kotlin 2.0 needs (2.4 infers either way).
 */
internal infix fun UInt.handledBy(
    method: suspend (ByteArray) -> ByteArray,
): Pair<UInt, suspend (ByteArray) -> ByteArray> = this to method
