package dev.keel.runtime.adapters

import dev.keel.runtime.KeelPortException
import dev.keel.runtime.PortImpl
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.KeelCodec
import dev.keel.runtime.wire.KeelReader
import dev.keel.runtime.wire.KeelWriter
import dev.keel.runtime.wire.WireException
import dev.keel.runtime.wire.encodeToByteArray
import java.io.IOException
import java.nio.file.AccessDeniedException
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.DirectoryNotEmptyException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.NoSuchFileException
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.PosixFilePermissions
import java.security.MessageDigest
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * A key-value store in a directory, behind both the `Kv` and the `SecureStore` ports on the JVM.
 *
 * Each entry is one file named after the SHA-256 of its key (so any key is a valid file name, whatever
 * the case rules or length limits of the file system) and holding `key length u32, key, value`. Writes
 * go to a temporary file that is moved into place, so a crash leaves the old or the new value, never a
 * torn one. `list` reads the key of every file, so it costs a directory scan.
 *
 * **This is not secure storage.** The files are only readable by their owner where the file system
 * supports permissions; nothing is encrypted. On a JVM that is the best a default can do; a real
 * `SecureStore` needs the platform's keystore (see `android-adapters`).
 *
 * @param dir the directory holding the entries; created on first write.
 */
public class FileKv(private val dir: Path) {

    /** The value stored under [key], or `null`. */
    public suspend fun get(key: String): ByteArray? = withContext(Dispatchers.IO) {
        val file = fileFor(key)
        try {
            val stored = readEntry(Files.readAllBytes(file))
            if (stored.first == key) stored.second else null // a hash collision is not a hit
        } catch (e: NoSuchFileException) {
            null
        }
    }

    /** Stores [value] under [key], replacing what was there. */
    public suspend fun set(key: String, value: ByteArray): Unit = withContext(Dispatchers.IO) {
        Files.createDirectories(dir)
        val w = KeelWriter(8 + key.length + value.size)
        w.writeStr(key)
        w.writeRaw(value)
        atomicWrite(fileFor(key), w.toByteArray(), ownerOnly = true)
    }

    /** Removes [key]; removing a missing key is not an error. */
    public suspend fun delete(key: String) {
        withContext(Dispatchers.IO) { Files.deleteIfExists(fileFor(key)) }
    }

    /** Every stored key that starts with [prefix], sorted. */
    public suspend fun list(prefix: String): List<String> = withContext(Dispatchers.IO) {
        if (!Files.isDirectory(dir)) return@withContext emptyList()
        val keys = ArrayList<String>()
        Files.newDirectoryStream(dir).use { entries ->
            for (file in entries) {
                if (file.fileName.toString().endsWith(TEMP_SUFFIX)) continue
                val key = try {
                    readEntry(Files.readAllBytes(file)).first
                } catch (e: IOException) {
                    continue // removed meanwhile
                } catch (e: WireException) {
                    continue // not one of ours
                }
                if (key.startsWith(prefix)) keys.add(key)
            }
        }
        keys.sorted()
    }

    /** This store as an async [PortImpl] for the port whose trait is named [trait] (`"Kv"` or `"SecureStore"`). */
    public fun portImpl(trait: String): PortImpl {
        require(trait == "Kv" || trait == "SecureStore") { "trait must be Kv or SecureStore, was $trait" }
        val ids = if (trait == "Kv") {
            arrayOf(StandardPorts.Kv.GET, StandardPorts.Kv.SET, StandardPorts.Kv.DELETE, StandardPorts.Kv.LIST)
        } else {
            arrayOf(StandardPorts.SecureStore.GET, StandardPorts.SecureStore.SET, StandardPorts.SecureStore.DELETE, StandardPorts.SecureStore.LIST)
        }
        return PortImpl(
            sync = false,
            methods = portMethods {
                this[ids[0]] = { args -> optionBytes.encodeToByteArray(this@FileKv.get(readArgs(args) { it.readStr() })) }
                this[ids[1]] = { args ->
                    val r = KeelReader(args)
                    val key = r.readStr()
                    val value = r.readBytes()
                    r.finish()
                    this@FileKv.set(key, value)
                    NO_BYTES
                }
                this[ids[2]] = { args ->
                    this@FileKv.delete(readArgs(args) { it.readStr() })
                    NO_BYTES
                }
                this[ids[3]] = { args -> stringList.encodeToByteArray(this@FileKv.list(readArgs(args) { it.readStr() })) }
            },
        )
    }

    private fun fileFor(key: String): Path = dir.resolve(sha256Hex(key))

    private fun readEntry(bytes: ByteArray): Pair<String, ByteArray> {
        val r = KeelReader(bytes)
        val key = r.readStr()
        return key to r.readRemaining()
    }

    private companion object {
        val optionBytes: KeelCodec<ByteArray?> = Codecs.option(Codecs.bytes)
        val stringList: KeelCodec<List<String>> = Codecs.vec(Codecs.string)
    }
}

/**
 * The `Fs` port over a directory: paths are relative to [root] (a leading `/` is ignored), and nothing outside
 * it can be reached: a path that resolves outside, whether through `..` or a symbolic link, is
 * [FsError.Denied]. Writes create missing parent directories and are atomic.
 *
 * @param root the directory acting as the file system's root; created on first write.
 */
public class FsAdapter(root: Path) {
    private val root: Path = root.toAbsolutePath().normalize()

    /**
     * The contents of the file at [path].
     *
     * @throws FsError if it cannot be read.
     */
    public suspend fun read(path: String): ByteArray = io { Files.readAllBytes(resolve(path)) }

    /**
     * Writes [data] to [path], replacing the file.
     *
     * @throws FsError if it cannot be written.
     */
    public suspend fun write(path: String, data: ByteArray): Unit = io {
        val target = resolve(path)
        Files.createDirectories(target.parent ?: root)
        atomicWrite(target, data, ownerOnly = false)
    }

    /**
     * Deletes the file or empty directory at [path].
     *
     * @throws FsError.NotFound if it does not exist.
     */
    public suspend fun delete(path: String): Unit = io { Files.delete(resolve(path)) }

    /**
     * The names of the entries of the directory [dir] (empty string for the root), sorted.
     *
     * @throws FsError if it cannot be listed.
     */
    public suspend fun list(dir: String): List<String> = io {
        Files.newDirectoryStream(resolve(dir)).use { entries -> entries.map { it.fileName.toString() }.sorted() }
    }

    /** This adapter as an async [PortImpl] for [StandardPorts.Fs]; failures answer with the [FsError]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Fs.READ] = { args ->
                val path = readArgs(args) { it.readStr() }
                fsResult { Codecs.bytes.encodeToByteArray(this@FsAdapter.read(path)) }
            }
            this[StandardPorts.Fs.WRITE] = { args ->
                val r = KeelReader(args)
                val path = r.readStr()
                val data = r.readBytes()
                r.finish()
                fsResult {
                    this@FsAdapter.write(path, data)
                    NO_BYTES
                }
            }
            this[StandardPorts.Fs.DELETE] = { args ->
                val path = readArgs(args) { it.readStr() }
                fsResult {
                    this@FsAdapter.delete(path)
                    NO_BYTES
                }
            }
            this[StandardPorts.Fs.LIST] = { args ->
                val dir = readArgs(args) { it.readStr() }
                fsResult { Codecs.vec(Codecs.string).encodeToByteArray(this@FsAdapter.list(dir)) }
            }
        },
    )

    private suspend fun <T> io(block: () -> T): T =
        withContext(Dispatchers.IO) {
            try {
                block()
            } catch (e: FsError) {
                throw e
            } catch (e: NoSuchFileException) {
                throw FsError.NotFound
            } catch (e: AccessDeniedException) {
                throw FsError.Denied
            } catch (e: SecurityException) {
                throw FsError.Denied
            } catch (e: DirectoryNotEmptyException) {
                throw FsError.Io("directory not empty: ${e.file}")
            } catch (e: IOException) {
                throw FsError.Io(e.message ?: e.javaClass.simpleName)
            }
        }

    /** Runs [block], turning an [FsError] into the port's typed error reply. */
    private inline fun fsResult(block: () -> ByteArray): ByteArray =
        try {
            block()
        } catch (e: FsError) {
            throw KeelPortException(FsError.encodeToByteArray(e))
        }

    private fun resolve(path: String): Path {
        val resolved = root.resolve(path.trimStart('/', '\\')).normalize()
        if (!resolved.startsWith(root)) throw FsError.Denied
        // Symbolic links must not lead out either: the nearest existing path inside the root (a link
        // counts even when dangling) must really live inside the root.
        var existing: Path = resolved
        while (existing != root && !Files.exists(existing, LinkOption.NOFOLLOW_LINKS)) existing = existing.parent
        if (Files.exists(existing, LinkOption.NOFOLLOW_LINKS)) {
            val realRoot = if (Files.exists(root)) root.toRealPath() else root
            val real = try {
                existing.toRealPath()
            } catch (e: IOException) {
                throw FsError.Denied // a dangling link
            }
            if (!real.startsWith(realRoot)) throw FsError.Denied
        }
        return resolved
    }
}

private val NO_BYTES = ByteArray(0)

/** Decodes the arguments of a port method with [read] and requires that nothing is left over. */
private inline fun <T> readArgs(args: ByteArray, read: (KeelReader) -> T): T {
    val r = KeelReader(args)
    val value = read(r)
    r.finish()
    return value
}

private const val TEMP_SUFFIX = ".tmp"

private fun sha256Hex(text: String): String {
    val digest = MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8))
    val sb = StringBuilder(digest.size * 2)
    for (b in digest) {
        sb.append("0123456789abcdef"[(b.toInt() shr 4) and 0xF]).append("0123456789abcdef"[b.toInt() and 0xF])
    }
    return sb.toString()
}

/** Writes [bytes] to a temporary sibling and moves it over [target], so readers never see a partial file. */
private fun atomicWrite(target: Path, bytes: ByteArray, ownerOnly: Boolean) {
    val temp = target.resolveSibling("${target.fileName}.${java.util.UUID.randomUUID()}$TEMP_SUFFIX")
    try {
        Files.write(temp, bytes)
        if (ownerOnly) {
            try {
                Files.setPosixFilePermissions(temp, PosixFilePermissions.fromString("rw-------"))
            } catch (e: UnsupportedOperationException) {
                // not a POSIX file system: nothing to restrict
            }
        }
        try {
            Files.move(temp, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } catch (e: AtomicMoveNotSupportedException) {
            Files.move(temp, target, StandardCopyOption.REPLACE_EXISTING)
        }
    } finally {
        Files.deleteIfExists(temp)
    }
}
