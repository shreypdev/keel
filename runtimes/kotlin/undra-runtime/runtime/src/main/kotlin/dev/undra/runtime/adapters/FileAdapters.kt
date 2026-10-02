package dev.undra.runtime.adapters

import dev.undra.runtime.UndraPortException
import dev.undra.runtime.PortImpl
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.encodeToByteArray
import java.io.IOException
import java.io.UncheckedIOException
import java.nio.file.AccessDeniedException
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.DirectoryIteratorException
import java.nio.file.DirectoryNotEmptyException
import java.nio.file.FileVisitResult
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.NoSuchFileException
import java.nio.file.Path
import java.nio.file.SimpleFileVisitor
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.BasicFileAttributes
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
 * torn one. `list` reads only the key at the start of every file (4 + the key's bytes), so it costs a directory scan
 * and an `open` per entry, not the size of the values.
 *
 * Failures are [StorageError]s (ADR-049): a full disk or quota (`ENOSPC`, `EDQUOT`) is [StorageError.Full], an entry
 * file that does not decode is [StorageError.Corrupt] (from [get]; the file stays, so nothing is lost silently), any
 * other I/O failure is [StorageError.Io] with the platform's description.
 *
 * **This is not secure storage.** The files are only readable by their owner where the file system
 * supports permissions; nothing is encrypted. On a JVM that is the best a default can do; a real
 * `SecureStore` needs the platform's keystore (see `android-adapters`).
 *
 * @param dir the directory holding the entries; created on first write.
 */
public class FileKv(private val dir: Path) : KeyValueBackend {

    /**
     * The value stored under [key], or `null`.
     *
     * @throws StorageError.Corrupt if the entry file of [key] does not decode.
     * @throws StorageError if it cannot be read.
     */
    override suspend fun get(key: String): ByteArray? = io {
        val bytes = try {
            Files.readAllBytes(fileFor(key))
        } catch (e: NoSuchFileException) {
            return@io null
        }
        val stored = try {
            readEntry(bytes)
        } catch (e: WireException) {
            throw StorageError.Corrupt("the entry of '$key' does not decode: ${e.message}")
        }
        if (stored.first == key) stored.second else null // a hash collision is not a hit
    }

    /**
     * Stores [value] under [key], replacing what was there.
     *
     * @throws StorageError.Full if the disk or the quota is exhausted (the old value, if any, stays).
     * @throws StorageError if it cannot be written.
     */
    override suspend fun set(key: String, value: ByteArray): Unit = io {
        Files.createDirectories(dir)
        val w = UndraWriter(8 + key.length + value.size)
        w.writeStr(key)
        w.writeRaw(value)
        atomicWrite(fileFor(key), w.toByteArray(), ownerOnly = true)
    }

    /**
     * Removes [key]; removing a missing key is not an error.
     *
     * @throws StorageError if it cannot be removed.
     */
    override suspend fun delete(key: String) {
        io { Files.deleteIfExists(fileFor(key)) }
    }

    /**
     * Every stored key that starts with [prefix], sorted. A file of the directory that is not an entry, or that is
     * removed while the directory is read, is not listed.
     *
     * @throws StorageError if the directory cannot be read.
     */
    override suspend fun list(prefix: String): List<String> = io {
        if (!Files.isDirectory(dir)) return@io emptyList()
        val keys = ArrayList<String>()
        Files.newDirectoryStream(dir).use { entries ->
            for (file in entries) {
                if (file.fileName.toString().endsWith(TEMP_SUFFIX)) continue
                val key = try {
                    readKey(file)
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

    /**
     * This store as an async [PortImpl] for the port whose trait is named [trait] (`"Kv"` or `"SecureStore"`); see
     * [StoragePort.portImpl].
     *
     * @throws IllegalArgumentException for any other trait name.
     */
    public fun portImpl(trait: String): PortImpl = StoragePort.of(trait).portImpl(this)

    /** Runs [block] on `Dispatchers.IO`, turning what the file system throws into a [StorageError]. */
    private suspend fun <T> io(block: () -> T): T =
        withContext(Dispatchers.IO) {
            try {
                block()
            } catch (e: StorageError) {
                throw e
            } catch (e: IOException) {
                throw StorageError.of(e)
            } catch (e: DirectoryIteratorException) {
                throw StorageError.of(e.cause ?: IOException(e))
            } catch (e: UncheckedIOException) {
                throw StorageError.of(e.cause ?: IOException(e))
            } catch (e: SecurityException) {
                throw StorageError.Io(StorageFailures.describe(e))
            }
        }

    private fun fileFor(key: String): Path = dir.resolve(sha256Hex(key))

    private fun readEntry(bytes: ByteArray): Pair<String, ByteArray> {
        val r = UndraReader(bytes)
        val key = r.readStr()
        return key to r.readRemaining()
    }

    /**
     * The key of the entry file [file], reading only its first `4 + key` bytes (the Swift adapter reads the same
     * header): the value, which may be megabytes, is never read.
     *
     * @throws WireException if the file is not an entry (a length that does not fit in it, a key that is not UTF-8).
     * @throws IOException if it cannot be read, or was removed meanwhile.
     */
    private fun readKey(file: Path): String {
        Files.newInputStream(file).use { input ->
            val head = ByteArray(Int.SIZE_BYTES)
            if (!readFully(input, head)) throw WireException.UnexpectedEof(head.size, 0)
            val declared = (head[0].toLong() and 0xFF) or ((head[1].toLong() and 0xFF) shl 8) or
                ((head[2].toLong() and 0xFF) shl 16) or ((head[3].toLong() and 0xFF) shl 24)
            // A length that does not fit in the file is not an entry; the check also bounds the allocation by the file's size.
            val size = Files.size(file)
            if (declared > size - head.size || declared > Int.MAX_VALUE - head.size) throw WireException.LengthTooLarge(declared.toUInt(), 0)
            val entry = head.copyOf(head.size + declared.toInt())
            val body = ByteArray(declared.toInt())
            if (!readFully(input, body)) throw WireException.UnexpectedEof(entry.size, head.size) // the file shrank meanwhile
            body.copyInto(entry, head.size)
            return UndraReader(entry).readStr()
        }
    }

    /** Fills [into] from [input]; `false` if the stream ends first. */
    private fun readFully(input: java.io.InputStream, into: ByteArray): Boolean {
        var read = 0
        while (read < into.size) {
            val n = input.read(into, read, into.size - read)
            if (n < 0) return false
            read += n
        }
        return true
    }
}

/**
 * The `Fs` port over a directory: paths are relative to [root] (a leading `/` is ignored), and nothing outside
 * it can be reached: a path that resolves outside, whether through `..` or a symbolic link, is
 * [FsError.Denied]. Writes create missing parent directories and are atomic. A missing file is
 * [FsError.NotFound], a refused permission [FsError.Denied], a full disk or quota (`ENOSPC`, `EDQUOT`)
 * [FsError.Full] (ADR-049), and any other failure [FsError.Io] with the platform's description.
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
     * Deletes the file at [path], or the directory with everything in it, as Swift's `removeItem` and the web adapter's
     * recursive `removeEntry` do. A symbolic link is removed and never followed: what it points at stays (as with `rm`).
     * The root itself cannot be deleted.
     *
     * @throws FsError.NotFound if it does not exist.
     * @throws FsError.Denied for the root, or a path that leads out of it.
     */
    public suspend fun delete(path: String): Unit = io {
        val target = resolve(path)
        if (target == root) throw FsError.Denied
        // Iterative (a tree of any depth), and `walkFileTree` does not follow links without being told to: a link to a
        // directory is visited as a file and unlinked, so nothing outside the root can be reached from inside the walk.
        Files.walkFileTree(
            target,
            object : SimpleFileVisitor<Path>() {
                override fun visitFile(file: Path, attrs: BasicFileAttributes): FileVisitResult {
                    Files.delete(file)
                    return FileVisitResult.CONTINUE
                }

                override fun postVisitDirectory(dir: Path, exc: IOException?): FileVisitResult {
                    if (exc != null) throw exc
                    Files.delete(dir)
                    return FileVisitResult.CONTINUE
                }
            },
        )
    }

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
                val r = UndraReader(args)
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
                throw if (StorageFailures.isOutOfSpace(e)) FsError.Full else FsError.Io(StorageFailures.describe(e))
            }
        }

    /** Runs [block], turning an [FsError] into the port's typed error reply. */
    private inline fun fsResult(block: () -> ByteArray): ByteArray =
        try {
            block()
        } catch (e: FsError) {
            throw UndraPortException(FsError.encodeToByteArray(e))
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

private const val TEMP_SUFFIX = ".tmp"

private fun sha256Hex(text: String): String {
    val digest = MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8))
    val sb = StringBuilder(digest.size * 2)
    for (b in digest) {
        sb.append("0123456789abcdef"[(b.toInt() shr 4) and 0xF]).append("0123456789abcdef"[b.toInt() and 0xF])
    }
    return sb.toString()
}

/**
 * Writes [bytes] to a temporary sibling and moves it over [target], so readers never see a partial file. A write that
 * fails (a full disk) removes what it wrote of the temporary file and throws the write's own failure; a failure to
 * remove it is attached as suppressed.
 */
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
    } catch (e: Throwable) {
        try {
            Files.deleteIfExists(temp)
        } catch (cleanup: IOException) {
            e.addSuppressed(cleanup)
        }
        throw e
    }
}
