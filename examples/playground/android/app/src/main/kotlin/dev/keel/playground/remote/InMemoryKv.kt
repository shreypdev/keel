package dev.keel.playground.remote

import dev.keel.runtime.PortImpl
import dev.keel.runtime.adapters.StandardPorts
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.KeelReader
import dev.keel.runtime.wire.encodeToByteArray
import java.util.concurrent.ConcurrentHashMap

/**
 * The `Kv` port backed by a map: the core keeps the persisted query cache and the offline queue here.
 * Nothing survives the process, which is what the demo wants (see `KeelApp`).
 */
class InMemoryKv {
    private val stored = ConcurrentHashMap<String, ByteArray>()

    /** This store as an async `Kv` port: `get`, `set`, `delete` and `list`, each over the wire encoding of its arguments. */
    fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Kv.GET] = { args ->
                Codecs.option(Codecs.bytes).encodeToByteArray(stored[key(args)])
            }
            this[StandardPorts.Kv.SET] = { args ->
                val reader = KeelReader(args)
                stored[reader.readStr()] = reader.readBytes()
                reader.finish()
                NOTHING
            }
            this[StandardPorts.Kv.DELETE] = { args ->
                stored.remove(key(args))
                NOTHING
            }
            this[StandardPorts.Kv.LIST] = { args ->
                val prefix = key(args)
                Codecs.vec(Codecs.string).encodeToByteArray(stored.keys.filter { it.startsWith(prefix) }.sorted())
            }
        },
    )

    /** The single string argument of `get`, `delete` and `list`. */
    private fun key(args: ByteArray): String {
        val reader = KeelReader(args)
        val key = reader.readStr()
        reader.finish()
        return key
    }

    private companion object {
        /** The empty reply of a port method that returns nothing. */
        val NOTHING = ByteArray(0)
    }
}
