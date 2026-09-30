package dev.keel.contract

import dev.keel.runtime.PortImpl
import dev.keel.runtime.adapters.StandardPorts
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.KeelReader
import dev.keel.runtime.wire.encodeToByteArray
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList

/**
 * The `Kv` port in memory, recording every write so that a scenario can see what the core persisted
 * (S14: the offline queue under `keel.query.queue`).
 */
class MemoryKv {
    /** One `set` (with its bytes) or `delete` (with `null`) the core asked for, in order. */
    class Write(val key: String, val value: ByteArray?)

    private val entries = ConcurrentHashMap<String, ByteArray>()

    /** Every write so far, oldest first. */
    val writes = CopyOnWriteArrayList<Write>()

    /** The value stored under [key] now, or `null`. */
    fun get(key: String): ByteArray? = entries[key]

    /** This store as an async `Kv` port, the way the default file-backed adapter is. */
    fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = mapOf(
            StandardPorts.Kv.GET to { args: ByteArray ->
                val key = string(args)
                Codecs.option(Codecs.bytes).encodeToByteArray(entries[key])
            },
            StandardPorts.Kv.SET to { args: ByteArray ->
                val r = KeelReader(args)
                val key = r.readStr()
                val value = r.readBytes()
                r.finish()
                entries[key] = value
                writes.add(Write(key, value))
                ByteArray(0)
            },
            StandardPorts.Kv.DELETE to { args: ByteArray ->
                val key = string(args)
                entries.remove(key)
                writes.add(Write(key, null))
                ByteArray(0)
            },
            StandardPorts.Kv.LIST to { args: ByteArray ->
                val prefix = string(args)
                Codecs.vec(Codecs.string).encodeToByteArray(entries.keys.filter { it.startsWith(prefix) }.sorted())
            },
        ),
    )

    private fun string(args: ByteArray): String {
        val r = KeelReader(args)
        val s = r.readStr()
        r.finish()
        return s
    }
}
