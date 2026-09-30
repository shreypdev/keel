package dev.undra.contract

import dev.undra.runtime.UndraCore

/**
 * One reading of `core.stats()` with the fields scenarios.md counts (live handles, transactions,
 * panics and the `crossings` counters). `UndraStats` models the first few; the crossings and the schema
 * hash are read from the document it carries as `raw`. A scenario that counts takes two readings and
 * compares them: other scenarios share the core, so absolute numbers mean nothing.
 */
class Stats(val raw: String) {
    private val doc: Map<String, Any?> = Json.parseObject(raw)
    private val crossings: Map<String, Any?> = doc["crossings"].asObject()

    /** Objects in the core's object table. */
    val liveHandles: Long get() = number(doc, "live_handles")

    /** Host calls the core is still working on. */
    val activeCalls: Long get() = number(doc, "active_calls")

    /** Streams that have not ended. */
    val openStreams: Long get() = number(doc, "open_streams")

    /** Change-sets emitted (`transactions`; equal to the change-sets delivered). */
    val transactions: Long get() = number(doc, "transactions")

    /** Panics the core caught. */
    val panics: Long get() = number(doc, "panics")

    /** Calls that crossed the boundary (`crossings.calls`). */
    val calls: Long get() = number(crossings, "calls")

    /** Change-sets delivered (`crossings.change_sets`). */
    val changeSets: Long get() = number(crossings, "change_sets")

    /** Calls the platform cancelled (`crossings.cancelled`). */
    val cancelled: Long get() = number(crossings, "cancelled")

    /** Requests the core rejected (`crossings.bad_requests`). */
    val badRequests: Long get() = number(crossings, "bad_requests")

    /** The schema hash the core reports, parsed from its `0x...` text. */
    val schemaHash: ULong
        get() {
            val text = doc["schema_hash"] as? String ?: fail("the statistics carry no schema_hash: $raw")
            return text.removePrefix("0x").toULong(16)
        }

    private fun number(from: Map<String, Any?>, key: String): Long =
        (from[key] as? Long) ?: fail("the statistics carry no number \"$key\": $raw")

    @Suppress("UNCHECKED_CAST")
    private fun Any?.asObject(): Map<String, Any?> =
        this as? Map<String, Any?> ?: fail("the statistics carry no crossings object: $raw")
}

/** Reads the core's statistics now. */
fun UndraCore.readStats(): Stats = Stats(stats().raw)
