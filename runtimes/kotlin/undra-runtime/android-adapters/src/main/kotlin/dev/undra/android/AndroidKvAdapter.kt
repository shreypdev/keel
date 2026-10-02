package dev.undra.android

import android.content.Context
import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.FileKv
import dev.undra.runtime.adapters.KeyValueBackend
import dev.undra.runtime.adapters.StorageError
import dev.undra.runtime.adapters.StoragePort
import java.io.File
import java.nio.file.Path

/**
 * The `Kv` port over files in the app's private storage: `<filesDir>/undra/<namespace>/kv` (the core's namespace, so two
 * cores of one app never see each other's keys: ADR-044 amendment A).
 *
 * Each key is one file, named after the SHA-256 of the key and holding `key length u32, key, value` (the format of
 * `FileKv`, which does the work, and of the Swift adapter's files), so any string is a valid key whatever the file
 * system's rules, `list` can recover the keys, and a hash collision can never return another key's value. A write goes
 * to a temporary file that is moved into place, so a process killed mid-write leaves the old value or the new one,
 * never a torn one; nothing is lost when the app is killed (the offline queue and the persisted query cache of
 * `undra-query` live here). `list` costs a directory scan.
 *
 * Failures are [StorageError]s, which the port answers the core with (ADR-049): a full disk or quota (`ENOSPC`,
 * `EDQUOT`) is [StorageError.Full] and leaves the old value in place; an entry file that does not decode is
 * [StorageError.Corrupt] (the file stays, so the core can move what it can read aside instead of overwriting it); any
 * other I/O failure is [StorageError.Io] with Android's description.
 *
 * The directory is under `Context.getFilesDir()`, so only the app can read it and Android's Auto Backup includes it
 * (a restored cache is checked against the schema hash by the core, which drops what another build wrote). Secrets
 * belong in [AndroidSecureStoreAdapter], not here.
 *
 * All operations run on `Dispatchers.IO`.
 */
public class AndroidKvAdapter internal constructor(directory: Path) : KeyValueBackend {
    /** The adapter over [directory]; created on the first write. */
    public constructor(directory: File) : this(directory.toPath())

    /** The adapter over `<filesDir>/undra/<namespace>/kv` of [context]'s application: [namespace] is the core's (`UndraCore.namespace`). */
    public constructor(context: Context, namespace: String) : this(directoryOf(context, namespace))

    private val store = FileKv(directory)

    /**
     * The value stored under [key], or `null`.
     *
     * @throws StorageError.Corrupt if the entry file of [key] does not decode.
     * @throws StorageError if it cannot be read.
     */
    override suspend fun get(key: String): ByteArray? = store.get(key)

    /**
     * Stores [value] under [key], replacing what was there.
     *
     * @throws StorageError.Full if the disk or the quota is exhausted (the old value stays).
     * @throws StorageError if it cannot be written.
     */
    override suspend fun set(key: String, value: ByteArray): Unit = store.set(key, value)

    /**
     * Removes [key]; removing a missing key is not an error.
     *
     * @throws StorageError if it cannot be removed.
     */
    override suspend fun delete(key: String): Unit = store.delete(key)

    /**
     * Every stored key that starts with [prefix], sorted.
     *
     * @throws StorageError if the directory cannot be read.
     */
    override suspend fun list(prefix: String): List<String> = store.list(prefix)

    /** This adapter as an async [PortImpl] for [dev.undra.runtime.adapters.StandardPorts.Kv]; a failure answers the core with its [StorageError]. */
    public fun portImpl(): PortImpl = StoragePort.KV.portImpl(this)

    /** Where the default adapter keeps a core's keys. */
    public companion object {
        /** `<filesDir>/undra/<namespace>/kv`: the directory of [context]'s application for the core [namespace]. */
        public fun directoryOf(context: Context, namespace: String): File =
            File(context.applicationContext.filesDir, "undra/$namespace/kv")
    }
}
