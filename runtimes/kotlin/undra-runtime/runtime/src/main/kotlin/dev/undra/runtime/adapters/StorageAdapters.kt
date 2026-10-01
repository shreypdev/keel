package dev.undra.runtime.adapters

import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.encodeToByteArray
import java.nio.file.FileSystemException

/**
 * The storage behind a `Kv` or `SecureStore` port (SPEC section 8): four operations on byte values under string keys.
 *
 * Every method reports a failure as a [StorageError] (ADR-049): [StorageError.Full] when the disk or quota is
 * exhausted, [StorageError.Corrupt] for stored bytes that cannot be read back, [StorageError.Locked] for protected data
 * that cannot be read now, [StorageError.Unavailable] when there is no backend, [StorageError.Io] for anything else.
 * [StoragePort.portImpl] answers the core with it. Throwing anything else is a bug in the backend: the core is
 * answered `unavailable` and the runtime logs it at error level.
 *
 * [FileKv] implements it on the JVM; `android-adapters` has `AndroidKvAdapter` and `AndroidSecureStoreAdapter`.
 */
public interface KeyValueBackend {
    /**
     * The value stored under [key], or `null` if there is none.
     *
     * @throws StorageError if it cannot be read.
     */
    public suspend fun get(key: String): ByteArray?

    /**
     * Stores [value] under [key], replacing what was there.
     *
     * @throws StorageError if it cannot be written.
     */
    public suspend fun set(key: String, value: ByteArray)

    /**
     * Removes [key]; removing a missing key is not an error.
     *
     * @throws StorageError if it cannot be removed.
     */
    public suspend fun delete(key: String)

    /**
     * Every stored key that starts with [prefix], in ascending order.
     *
     * @throws StorageError if the keys cannot be listed.
     */
    public suspend fun list(prefix: String): List<String>
}

/**
 * The two key-value ports of SPEC section 8, `Kv` and `SecureStore`, which differ only in their ids, and the one
 * method table both are served by.
 *
 * ```kotlin
 * core.registerPort(StoragePort.KV.portId, StoragePort.KV.portImpl(myBackend))
 * ```
 *
 * @property portId `fnv1a32("port.<Trait>")`.
 * @property getId the id of `get`.
 * @property setId the id of `set`.
 * @property deleteId the id of `delete`.
 * @property listId the id of `list`.
 */
public enum class StoragePort(
    public val portId: UInt,
    public val getId: UInt,
    public val setId: UInt,
    public val deleteId: UInt,
    public val listId: UInt,
) {
    /** `Kv`: the app's persistent key-value store ([StandardPorts.Kv]). */
    KV(StandardPorts.Kv.PORT_ID, StandardPorts.Kv.GET, StandardPorts.Kv.SET, StandardPorts.Kv.DELETE, StandardPorts.Kv.LIST),

    /** `SecureStore`: the same four methods for secrets ([StandardPorts.SecureStore]). */
    SECURE_STORE(
        StandardPorts.SecureStore.PORT_ID,
        StandardPorts.SecureStore.GET,
        StandardPorts.SecureStore.SET,
        StandardPorts.SecureStore.DELETE,
        StandardPorts.SecureStore.LIST,
    ),
    ;

    /** The trait's name as `undra-ports` declares it: `"Kv"` or `"SecureStore"`. */
    public val traitName: String
        get() = if (this == KV) "Kv" else "SecureStore"

    /**
     * [backend] as an async [PortImpl] for this port. Each method decodes its arguments, calls [backend] and encodes
     * the result (`Option<Bytes>`, nothing, nothing, `Vec<String>`); a [StorageError] the backend throws answers the
     * core with that error (port status 1, ADR-049), and anything else it throws answers `unavailable` (status 2) and
     * is logged at error level.
     */
    public fun portImpl(backend: KeyValueBackend): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[getId] = { args ->
                val key = readArgs(args) { it.readStr() }
                storageResult { OPTION_BYTES.encodeToByteArray(backend.get(key)) }
            }
            this[setId] = { args ->
                val r = UndraReader(args)
                val key = r.readStr()
                val value = r.readBytes()
                r.finish()
                storageResult {
                    backend.set(key, value)
                    NO_STORAGE_REPLY
                }
            }
            this[deleteId] = { args ->
                val key = readArgs(args) { it.readStr() }
                storageResult {
                    backend.delete(key)
                    NO_STORAGE_REPLY
                }
            }
            this[listId] = { args ->
                val prefix = readArgs(args) { it.readStr() }
                storageResult { STRING_LIST.encodeToByteArray(backend.list(prefix)) }
            }
        },
    )

    /** Lookups. */
    public companion object {
        /**
         * The port named [trait] (`"Kv"` or `"SecureStore"`).
         *
         * @throws IllegalArgumentException for any other name.
         */
        public fun of(trait: String): StoragePort = entries.firstOrNull { it.traitName == trait }
            ?: throw IllegalArgumentException("trait must be Kv or SecureStore, was $trait")
    }
}

private val OPTION_BYTES: UndraCodec<ByteArray?> = Codecs.option(Codecs.bytes)
private val STRING_LIST: UndraCodec<List<String>> = Codecs.vec(Codecs.string)
private val NO_STORAGE_REPLY = ByteArray(0)

/** Runs [block], turning a [StorageError] into the port's typed error reply. */
private inline fun storageResult(block: () -> ByteArray): ByteArray =
    try {
        block()
    } catch (e: StorageError) {
        throw UndraPortException(StorageError.encodeToByteArray(e))
    }

/** How the runtime's storage and file adapters read a platform failure. */
internal object StorageFailures {
    /** How each platform spells an exhausted disk or quota (`strerror` and the errno names Android puts in its messages). */
    private val OUT_OF_SPACE = listOf("ENOSPC", "EDQUOT", "No space left on device", "Disk quota exceeded", "not enough space on the disk")

    /** How deep the cause chain is searched; a chain is short, and a cyclic one must not loop. */
    private const val MAX_CAUSES = 8

    /**
     * Whether [error], or one of its causes, says the disk or the quota is exhausted: the JVM's
     * `IOException("No space left on device")` and `FileSystemException(file, null, "No space left on device")`,
     * Android's `IOException("write failed: ENOSPC (No space left on device)")` around an `ErrnoException`, `EDQUOT`,
     * and Windows' `There is not enough space on the disk`.
     */
    fun isOutOfSpace(error: Throwable): Boolean {
        var t: Throwable? = error
        var depth = 0
        while (t != null && depth < MAX_CAUSES) {
            val text = t.message
            if (text != null && OUT_OF_SPACE.any { text.contains(it, ignoreCase = true) }) return true
            val next = t.cause
            t = if (next === t) null else next
            depth++
        }
        return false
    }

    /**
     * The platform's description of [error] for an `Io` variant: its message, except for a `FileSystemException`
     * without a reason (`AccessDeniedException(file)` and friends), whose message is only the file, so the class
     * names what happened.
     */
    fun describe(error: Throwable): String = when {
        error is FileSystemException && error.reason == null -> "${error.javaClass.simpleName}: ${error.file}"
        else -> error.message?.takeIf { it.isNotBlank() } ?: error.javaClass.simpleName
    }
}
