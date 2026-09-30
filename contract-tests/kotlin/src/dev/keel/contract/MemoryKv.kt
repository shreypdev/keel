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

    private val stored = ConcurrentHashMap<String, ByteArray>()

    /** Every write so far, oldest first. */
    val writes = CopyOnWriteArrayList<Write>()

    /** This store as an async `Kv` port, the way the default file-backed adapter is. */
    fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Kv.GET] = { args ->
                val key = string(args)
                Codecs.option(Codecs.bytes).encodeToByteArray(stored[key])
            }
            this[StandardPorts.Kv.SET] = { args ->
                val r = KeelReader(args)
                val key = r.readStr()
                val value = r.readBytes()
                r.finish()
                stored[key] = value
                writes.add(Write(key, value))
                ByteArray(0)
            }
            this[StandardPorts.Kv.DELETE] = { args ->
                val key = string(args)
                stored.remove(key)
                writes.add(Write(key, null))
                ByteArray(0)
            }
            this[StandardPorts.Kv.LIST] = { args ->
                val prefix = string(args)
                Codecs.vec(Codecs.string).encodeToByteArray(stored.keys.filter { it.startsWith(prefix) }.sorted())
            }
        },
    )

    private fun string(args: ByteArray): String {
        val r = KeelReader(args)
        val s = r.readStr()
        r.finish()
        return s
    }
}
