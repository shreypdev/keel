package dev.undra.contract

import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.KeyValueBackend
import dev.undra.runtime.adapters.StorageError
import dev.undra.runtime.adapters.StoragePort
import dev.undra.runtime.wire.Fnv
import dev.undra.runtime.wire.UndraReader

/**
 * The `Kv` port of the harness (scenarios.md, "Adapters"): a map in memory that records every operation the core
 * asks for, in order, and fails on demand.
 *
 * A failure is a [StorageError] (ADR-049). The harness serves the port through the runtime's own key-value port table
 * ([StoragePort.portImpl], what `FileKv` and the Android adapters use), so a failure is answered exactly as a platform
 * adapter answers one: port status 1 with the encoded error. [fail] makes the next `times` operations of one kind (or
 * of one key) fail, or every one of them until [heal].
 *
 * @param initial the contents to start with (build B starts from what build A persisted, S14 step 8).
 */
class MemoryKv(initial: Map<String, ByteArray> = emptyMap()) : KeyValueBackend {
    /** The kinds of operation of the `Kv` port. */
    enum class Kind { GET, SET, DELETE, LIST }

    /**
     * One operation the core asked for.
     *
     * @property kind which method.
     * @property key the key (the prefix of a `list`).
     * @property value the bytes of a `set`, else `null`.
     * @property failure the error the harness answered with, `null` for a success.
     */
    class Operation(val kind: Kind, val key: String, val value: ByteArray?, val failure: StorageError?) {
        /** Whether this is a `set` that succeeded. */
        val isSet: Boolean get() = kind == Kind.SET && failure == null

        override fun toString(): String =
            "${kind.name.lowercase()}($key${if (value != null) ", ${value.size} bytes" else ""})" + (failure?.let { " -> $it" } ?: "")
    }

    private class Rule(val kind: Kind?, val key: String?, val error: StorageError, var remaining: Int?) {
        fun matches(kind: Kind, key: String): Boolean = (this.kind == null || this.kind == kind) && (this.key == null || this.key == key)
    }

    private val lock = Any()
    private val stored = HashMap(initial)
    private val log = ArrayList<Operation>()
    private val rules = ArrayList<Rule>()

    /**
     * Makes operations of [kind] (every kind if `null`) on [key] (every key if `null`) fail with [error]: the next
     * [times] of them, or every one until [heal] if [times] is `null`.
     */
    fun fail(kind: Kind?, key: String? = null, error: StorageError, times: Int? = null) {
        synchronized(lock) { rules.add(Rule(kind, key, error, times)) }
    }

    /** Removes every failure the test asked for. */
    fun heal() {
        synchronized(lock) { rules.clear() }
    }

    /** Every operation so far, oldest first, failed ones included. */
    val operations: List<Operation> get() = synchronized(lock) { log.toList() }

    /** Every key and its value now. */
    val entries: Map<String, ByteArray> get() = synchronized(lock) { HashMap(stored) }

    /** The value stored under [key], if any. */
    fun value(key: String): ByteArray? = synchronized(lock) { stored[key] }

    /** This store as the async `Kv` port, through the runtime's own key-value port table. */
    fun portImpl(): PortImpl = StoragePort.KV.portImpl(this)

    /** Records one operation and either fails it with the first rule that applies or performs it with [body]. */
    private fun <T> perform(kind: Kind, key: String, value: ByteArray?, body: (MutableMap<String, ByteArray>) -> T): T {
        val outcome: Result<T> = synchronized(lock) {
            val index = rules.indexOfFirst { it.matches(kind, key) }
            if (index >= 0) {
                val rule = rules[index]
                val left = rule.remaining
                if (left != null) {
                    if (left <= 1) rules.removeAt(index) else rule.remaining = left - 1
                }
                log.add(Operation(kind, key, value, rule.error))
                Result.failure(rule.error)
            } else {
                log.add(Operation(kind, key, value, null))
                Result.success(body(stored))
            }
        }
        return outcome.getOrThrow()
    }

    override suspend fun get(key: String): ByteArray? = perform(Kind.GET, key, null) { it[key] }

    override suspend fun set(key: String, value: ByteArray) {
        perform(Kind.SET, key, value) { it[key] = value }
    }

    override suspend fun delete(key: String) {
        perform(Kind.DELETE, key, null) { it.remove(key) }
    }

    override suspend fun list(prefix: String): List<String> =
        perform(Kind.LIST, prefix, null) { entries -> entries.keys.filter { it.startsWith(prefix) }.sorted() }
}

/** The keys and layouts of what the query client persists (ADR-037), as the scenarios read them. */
object Persisted {
    /** The offline queue, format 2. */
    const val QUEUE_KEY: String = "undra.query.queue2"

    /** The dead-letter queue. */
    const val DEAD_LETTER_KEY: String = "undra.query.queue.dead"

    /** The number of items of a format-2 queue (`format u16 = 2, schema_hash u64, count u32, ..`: the `u32` at offset 10), or `null`. */
    fun queueCount(value: ByteArray): Long? {
        if (value.size < 14 || value[0] != 2.toByte() || value[1] != 0.toByte()) return null
        return UndraReader(value, 10, 4).readU32().toLong()
    }

    /** The `u64` at [offset] of [value] (a fingerprint), or `null` if the value is shorter. */
    fun fingerprint(value: ByteArray, offset: Int): ULong? =
        if (value.size < offset + 8) null else UndraReader(value, offset, 8).readU64()

    /** `undra.types.<fingerprint as 16 hex digits>`: where a closure description is stored. */
    fun typesKey(fingerprint: ULong): String = "undra.types." + fingerprint.toString(16).padStart(16, '0')

    /** `undra.query.cache2.<query id as 8 hex digits>.<fnv1a64 of the encoded arguments as 16>`: a cache entry's key. */
    fun cacheKey(queryId: UInt, arguments: ByteArray): String =
        "undra.query.cache2." + queryId.toString(16).padStart(8, '0') + "." + Fnv.fnv1a64(arguments).toString(16).padStart(16, '0')

    /** Whether [operation] leaves the offline queue empty: a delete, or a write of a format-2 queue whose count is 0. */
    fun emptiesQueue(operation: MemoryKv.Operation): Boolean =
        operation.failure == null && when (operation.kind) {
            MemoryKv.Kind.DELETE -> true
            MemoryKv.Kind.SET -> operation.value?.let { queueCount(it) } == 0L
            else -> false
        }
}
