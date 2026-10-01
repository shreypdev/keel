package dev.undra.android

import android.content.Context
import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.FileKv
import java.io.File

/**
 * The `Kv` port over files in the app's private storage: `<filesDir>/undra/kv`.
 *
 * Each key is one file, named after the SHA-256 of the key and holding `key length u32, key, value` (the format of
 * `FileKv`, which does the work, and of the Swift adapter's files), so any string is a valid key whatever the file
 * system's rules, `list` can recover the keys, and a hash collision can never return another key's value. A write goes
 * to a temporary file that is moved into place, so a process killed mid-write leaves the old value or the new one,
 * never a torn one; nothing is lost when the app is killed (the offline queue and the persisted query cache of
 * `undra-query` live here). `list` costs a directory scan.
 *
 * The directory is under `Context.getFilesDir()`, so only the app can read it and Android's Auto Backup includes it
 * (a restored cache is checked against the schema hash by the core, which drops what another build wrote). Secrets
 * belong in [AndroidSecureStoreAdapter], not here.
 *
 * All operations run on `Dispatchers.IO`.
 *
 * @param directory where the entries live; created on the first write.
 */
public class AndroidKvAdapter(directory: File) {
    /** The adapter over `<filesDir>/undra/kv` of [context]'s application. */
    public constructor(context: Context) : this(File(context.applicationContext.filesDir, DEFAULT_PATH))

    private val store = FileKv(directory.toPath())

    /** The value stored under [key], or `null`. */
    public suspend fun get(key: String): ByteArray? = store.get(key)

    /** Stores [value] under [key], replacing what was there. */
    public suspend fun set(key: String, value: ByteArray): Unit = store.set(key, value)

    /** Removes [key]; removing a missing key is not an error. */
    public suspend fun delete(key: String): Unit = store.delete(key)

    /** Every stored key that starts with [prefix], sorted. */
    public suspend fun list(prefix: String): List<String> = store.list(prefix)

    /** This adapter as an async [PortImpl] for [dev.undra.runtime.adapters.StandardPorts.Kv]. */
    public fun portImpl(): PortImpl = store.portImpl("Kv")

    private companion object {
        const val DEFAULT_PATH = "undra/kv"
    }
}
