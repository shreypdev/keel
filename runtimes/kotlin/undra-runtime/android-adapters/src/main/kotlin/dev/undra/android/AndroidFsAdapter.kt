package dev.undra.android

import android.content.Context
import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.adapters.FsAdapter
import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.encodeToByteArray
import java.io.File

/**
 * The `Fs` port over a directory of the app's private storage: `<filesDir>/undra/fs`.
 *
 * It behaves like the Swift (`FileManager`) and TypeScript (OPFS) adapters:
 *
 *  - Paths from the core are relative to the root (`"notes/a.txt"`); a leading `/` is ignored. A path with a `..`
 *    component is [FsError.Denied], and so is one that leads out of the root through a symbolic link, so the core cannot
 *    reach anything outside the root.
 *  - `write` creates missing parent directories and is atomic (a temporary file moved into place).
 *  - `delete` removes a file, or a directory with everything in it; the root itself cannot be deleted ([FsError.Denied]).
 *  - `list` returns the names of the entries of one directory, sorted; a directory has no trailing slash.
 *  - A missing file or directory is [FsError.NotFound]; a refused permission is [FsError.Denied]; any other failure
 *    (reading a directory, listing a file, a full disk, ...) is [FsError.Io] with the platform's description.
 *
 * Files under `Context.getFilesDir()` are private to the app and included in Android's Auto Backup. To serve another
 * directory (`Context.getExternalFilesDir`, the no-backup directory) pass it as [root]. All operations run on
 * `Dispatchers.IO`; [FsAdapter] does the confinement and the atomic writes.
 *
 * @param root the directory acting as the file system's root; created on the first write.
 */
public class AndroidFsAdapter(root: File) {
    /** The adapter over `<filesDir>/undra/fs` of [context]'s application. */
    public constructor(context: Context) : this(File(context.applicationContext.filesDir, DEFAULT_PATH))

    private val fs = FsAdapter(root.toPath())

    /**
     * The contents of the file at [path].
     *
     * @throws FsError if it cannot be read.
     */
    public suspend fun read(path: String): ByteArray = fs.read(confined(path))

    /**
     * Writes [data] to [path], replacing the file.
     *
     * @throws FsError if it cannot be written.
     */
    public suspend fun write(path: String, data: ByteArray): Unit = fs.write(confined(path), data)

    /**
     * Deletes the file at [path], or the directory with everything in it.
     *
     * @throws FsError.NotFound if it does not exist.
     * @throws FsError.Denied for the root.
     */
    public suspend fun delete(path: String) {
        val target = confined(path)
        if (segments(target).isEmpty()) throw FsError.Denied
        deleteTree(target)
    }

    /**
     * The names of the entries of the directory [dir] (the empty string is the root), sorted.
     *
     * @throws FsError if it cannot be listed.
     */
    public suspend fun list(dir: String): List<String> = fs.list(confined(dir))

    /** This adapter as an async [PortImpl] for [StandardPorts.Fs]; failures answer with the [FsError]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Fs.READ] = { args ->
                val path = readArgs(args) { it.readStr() }
                fsResult { Codecs.bytes.encodeToByteArray(this@AndroidFsAdapter.read(path)) }
            }
            this[StandardPorts.Fs.WRITE] = { args ->
                val reader = UndraReader(args)
                val path = reader.readStr()
                val data = reader.readBytes()
                reader.finish()
                fsResult {
                    this@AndroidFsAdapter.write(path, data)
                    NO_REPLY
                }
            }
            this[StandardPorts.Fs.DELETE] = { args ->
                val path = readArgs(args) { it.readStr() }
                fsResult {
                    this@AndroidFsAdapter.delete(path)
                    NO_REPLY
                }
            }
            this[StandardPorts.Fs.LIST] = { args ->
                val dir = readArgs(args) { it.readStr() }
                fsResult { Codecs.vec(Codecs.string).encodeToByteArray(this@AndroidFsAdapter.list(dir)) }
            }
        },
    )

    /** Deletes [path] and, if it is a directory, everything below it first. Every step goes through [fs], so none can leave the root. */
    private suspend fun deleteTree(path: String) {
        // `list` of a file is an I/O error; of a missing path, NotFound (which is the answer to deleting it).
        val children = try {
            fs.list(path)
        } catch (e: FsError.Io) {
            null
        }
        if (children != null) for (child in children) deleteTree("$path/$child")
        fs.delete(path)
    }

    /** Runs [block], turning an [FsError] into the port's typed error reply. */
    private inline fun fsResult(block: () -> ByteArray): ByteArray =
        try {
            block()
        } catch (e: FsError) {
            throw UndraPortException(FsError.encodeToByteArray(e))
        }

    private companion object {
        const val DEFAULT_PATH = "undra/fs"

        /** The non-empty components of [path] (`.` and empty ones dropped). */
        fun segments(path: String): List<String> = path.split('/').filter { it.isNotEmpty() && it != "." }

        /** [path] if no component is `..`: the other platforms refuse it outright, even where it would stay inside the root. */
        fun confined(path: String): String {
            if (segments(path).any { it == ".." }) throw FsError.Denied
            return path
        }
    }
}
