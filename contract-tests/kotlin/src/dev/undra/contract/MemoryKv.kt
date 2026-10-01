package dev.undra.contract

import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList

/**
 * The `Kv` port in memory, recording every write so that a scenario can see what the core persisted
 * (S14: the offline queue under `undra.query.queue`).
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
                val r = UndraReader(args)
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
        val r = UndraReader(args)
        val s = r.readStr()
        r.finish()
        return s
    }
}
