package dev.undra.testsupport

import java.io.File
import java.io.IOException
import java.net.URI
import java.nio.channels.SeekableByteChannel
import java.nio.file.AccessMode
import java.nio.file.CopyOption
import java.nio.file.DirectoryStream
import java.nio.file.FileStore
import java.nio.file.FileSystem
import java.nio.file.LinkOption
import java.nio.file.OpenOption
import java.nio.file.Path
import java.nio.file.PathMatcher
import java.nio.file.ProviderMismatchException
import java.nio.file.StandardOpenOption
import java.nio.file.WatchEvent
import java.nio.file.WatchKey
import java.nio.file.WatchService
import java.nio.file.attribute.BasicFileAttributes
import java.nio.file.attribute.FileAttribute
import java.nio.file.attribute.FileAttributeView
import java.nio.file.attribute.UserPrincipalLookupService
import java.nio.file.spi.FileSystemProvider
import java.util.concurrent.atomic.AtomicInteger

/**
 * A view of a real directory through which file operations fail on demand, the way the platform fails them: a write
 * that hits a full disk, a read of an unreadable file, a directory that cannot be listed. It is a `java.nio.file`
 * file system whose paths wrap the default file system's, so an adapter that takes a [Path] (the runtime's `FileKv` and
 * `FsAdapter`, the Android adapters' internal constructors) runs its real code over it and meets real `IOException`s,
 * with nothing in the adapter knowing it is being tested. Shared by the runtime's tests and `android-adapters`' (it
 * needs only `java.nio.file`, which Android has from API 26).
 *
 * ```kotlin
 * val disk = FaultyFileSystem(tempDir)
 * val kv = FileKv(disk.root.resolve("kv"))
 * disk.fail(FaultyFileSystem.Op.WRITE) { FaultyFileSystem.noSpace() }
 * kv.set("k", bytes) // throws StorageError.Full
 * disk.heal()
 * ```
 *
 * Metadata (attributes, existence, permissions) is never faulted, only the operations of [Op].
 *
 * @param base the real directory the view starts at; [root] is the same directory seen through the view.
 */
class FaultyFileSystem(base: Path) {
    /** What can be made to fail. */
    enum class Op {
        /** Opening a file for reading. */
        READ,

        /** Opening a file for writing, and creating a directory that does not exist yet. */
        WRITE,

        /** Opening a directory to list it. */
        LIST,

        /** Deleting a file or directory. */
        DELETE,

        /** Moving (or copying) a file into place. */
        MOVE,
    }

    private class Fault(val remaining: AtomicInteger, val error: () -> IOException)

    private val faults = java.util.concurrent.ConcurrentHashMap<Op, Fault>()
    private val raised = AtomicInteger()
    private val provider = Provider(this)
    private val fileSystem = Fs(provider, base.fileSystem)

    /** [base], seen through this view. */
    val root: Path = wrap(base.toAbsolutePath())

    /** How many operations failed on purpose so far. */
    val failures: Int get() = raised.get()

    /** Makes the next [times] operations of kind [op] throw what [error] makes (every one, by default), until [heal]. */
    fun fail(op: Op, times: Int = Int.MAX_VALUE, error: () -> IOException) {
        faults[op] = Fault(AtomicInteger(times), error)
    }

    /** Stops every fault: operations behave as on the real file system again. */
    fun heal() {
        faults.clear()
    }

    internal fun check(op: Op) {
        val fault = faults[op] ?: return
        if (fault.remaining.getAndDecrement() <= 0) {
            faults.remove(op, fault)
            return
        }
        raised.incrementAndGet()
        throw fault.error()
    }

    internal fun wrap(path: Path): Path = FaultyPath(fileSystem, path)

    /** Exceptions shaped as the platforms throw them. */
    companion object {
        /** A write on a full disk as the JVM reports it on macOS and Linux (`FileChannel.write`), or Android's form of it. */
        fun noSpace(android: Boolean = false): IOException =
            if (android) IOException("write failed: ENOSPC (No space left on device)") else IOException("No space left on device")

        /** A quota that ran out (`EDQUOT`). */
        fun quotaExceeded(): IOException = IOException("Disk quota exceeded")

        /** A failing disk (`EIO`). */
        fun ioError(): IOException = IOException("Input/output error")
    }

    // ---- the java.nio.file plumbing ---------------------------------------------------------------------------------

    private class FaultyPath(private val fs: Fs, val delegate: Path) : Path {
        private fun wrap(p: Path?): Path? = p?.let { FaultyPath(fs, it) }

        private fun unwrap(p: Path): Path = (p as? FaultyPath)?.delegate ?: throw ProviderMismatchException("not a FaultyFileSystem path: $p")

        override fun getFileSystem(): FileSystem = fs

        override fun isAbsolute(): Boolean = delegate.isAbsolute

        override fun getRoot(): Path? = wrap(delegate.root)

        override fun getFileName(): Path? = wrap(delegate.fileName)

        override fun getParent(): Path? = wrap(delegate.parent)

        override fun getNameCount(): Int = delegate.nameCount

        override fun getName(index: Int): Path = FaultyPath(fs, delegate.getName(index))

        override fun subpath(beginIndex: Int, endIndex: Int): Path = FaultyPath(fs, delegate.subpath(beginIndex, endIndex))

        override fun startsWith(other: Path): Boolean = other is FaultyPath && delegate.startsWith(other.delegate)

        override fun startsWith(other: String): Boolean = delegate.startsWith(other)

        override fun endsWith(other: Path): Boolean = other is FaultyPath && delegate.endsWith(other.delegate)

        override fun endsWith(other: String): Boolean = delegate.endsWith(other)

        override fun normalize(): Path = FaultyPath(fs, delegate.normalize())

        override fun resolve(other: Path): Path = FaultyPath(fs, delegate.resolve(unwrap(other)))

        override fun resolve(other: String): Path = FaultyPath(fs, delegate.resolve(other))

        override fun resolveSibling(other: Path): Path = FaultyPath(fs, delegate.resolveSibling(unwrap(other)))

        override fun resolveSibling(other: String): Path = FaultyPath(fs, delegate.resolveSibling(other))

        override fun relativize(other: Path): Path = FaultyPath(fs, delegate.relativize(unwrap(other)))

        override fun toUri(): URI = delegate.toUri()

        override fun toAbsolutePath(): Path = FaultyPath(fs, delegate.toAbsolutePath())

        override fun toRealPath(vararg options: LinkOption): Path = FaultyPath(fs, delegate.toRealPath(*options))

        override fun toFile(): File = delegate.toFile()

        override fun register(watcher: WatchService, events: Array<out WatchEvent.Kind<*>>, vararg modifiers: WatchEvent.Modifier): WatchKey =
            throw UnsupportedOperationException("FaultyFileSystem has no watch service")

        override fun register(watcher: WatchService, vararg events: WatchEvent.Kind<*>): WatchKey =
            throw UnsupportedOperationException("FaultyFileSystem has no watch service")

        override fun iterator(): MutableIterator<Path> {
            val names = delegate.iterator()
            return object : MutableIterator<Path> {
                override fun hasNext(): Boolean = names.hasNext()

                override fun next(): Path = FaultyPath(fs, names.next())

                override fun remove(): Unit = throw UnsupportedOperationException()
            }
        }

        override fun compareTo(other: Path): Int = delegate.compareTo(unwrap(other))

        override fun equals(other: Any?): Boolean = other is FaultyPath && other.fs === fs && other.delegate == delegate

        override fun hashCode(): Int = delegate.hashCode()

        override fun toString(): String = delegate.toString()
    }

    private class Fs(private val provider: Provider, private val base: FileSystem) : FileSystem() {
        override fun provider(): FileSystemProvider = provider

        override fun close() = Unit

        override fun isOpen(): Boolean = true

        override fun isReadOnly(): Boolean = false

        override fun getSeparator(): String = base.separator

        override fun getRootDirectories(): Iterable<Path> = base.rootDirectories.map { FaultyPath(this, it) }

        override fun getFileStores(): Iterable<FileStore> = base.fileStores

        override fun supportedFileAttributeViews(): Set<String> = base.supportedFileAttributeViews()

        override fun getPath(first: String, vararg more: String): Path = FaultyPath(this, base.getPath(first, *more))

        override fun getPathMatcher(syntaxAndPattern: String): PathMatcher {
            val matcher = base.getPathMatcher(syntaxAndPattern)
            return PathMatcher { path -> matcher.matches((path as? FaultyPath)?.delegate ?: path) }
        }

        override fun getUserPrincipalLookupService(): UserPrincipalLookupService = base.userPrincipalLookupService

        override fun newWatchService(): WatchService = throw UnsupportedOperationException("FaultyFileSystem has no watch service")
    }

    private class Provider(private val owner: FaultyFileSystem) : FileSystemProvider() {
        private fun real(path: Path): Path = (path as? FaultyPath)?.delegate ?: throw ProviderMismatchException("not a FaultyFileSystem path: $path")

        private fun Path.realProvider(): FileSystemProvider = fileSystem.provider()

        override fun getScheme(): String = "undra-faulty"

        override fun newFileSystem(uri: URI, env: Map<String, *>): FileSystem = throw UnsupportedOperationException()

        override fun getFileSystem(uri: URI): FileSystem = throw UnsupportedOperationException()

        override fun getPath(uri: URI): Path = throw UnsupportedOperationException()

        override fun newByteChannel(path: Path, options: Set<OpenOption>, vararg attrs: FileAttribute<*>): SeekableByteChannel {
            val writing = StandardOpenOption.WRITE in options || StandardOpenOption.APPEND in options
            owner.check(if (writing) Op.WRITE else Op.READ)
            val target = real(path)
            return target.realProvider().newByteChannel(target, options, *attrs)
        }

        override fun newDirectoryStream(dir: Path, filter: DirectoryStream.Filter<in Path>): DirectoryStream<Path> {
            owner.check(Op.LIST)
            val target = real(dir)
            val inner = target.realProvider().newDirectoryStream(target) { entry -> filter.accept(owner.wrap(entry)) }
            return object : DirectoryStream<Path> {
                override fun iterator(): MutableIterator<Path> {
                    val entries = inner.iterator()
                    return object : MutableIterator<Path> {
                        override fun hasNext(): Boolean = entries.hasNext()

                        override fun next(): Path = owner.wrap(entries.next())

                        override fun remove(): Unit = entries.remove()
                    }
                }

                override fun close() = inner.close()
            }
        }

        override fun createDirectory(dir: Path, vararg attrs: FileAttribute<*>) {
            val target = real(dir)
            // A directory that exists is refused as such before the disk is asked for space, as mkdir(2) does.
            if (!java.nio.file.Files.exists(target, LinkOption.NOFOLLOW_LINKS)) owner.check(Op.WRITE)
            target.realProvider().createDirectory(target, *attrs)
        }

        override fun createSymbolicLink(link: Path, target: Path, vararg attrs: FileAttribute<*>) {
            val real = real(link)
            real.realProvider().createSymbolicLink(real, (target as? FaultyPath)?.delegate ?: target, *attrs)
        }

        override fun readSymbolicLink(link: Path): Path {
            val real = real(link)
            return owner.wrap(real.realProvider().readSymbolicLink(real))
        }

        override fun delete(path: Path) {
            owner.check(Op.DELETE)
            val target = real(path)
            target.realProvider().delete(target)
        }

        override fun copy(source: Path, target: Path, vararg options: CopyOption) {
            owner.check(Op.MOVE)
            val from = real(source)
            from.realProvider().copy(from, real(target), *options)
        }

        override fun move(source: Path, target: Path, vararg options: CopyOption) {
            owner.check(Op.MOVE)
            val from = real(source)
            from.realProvider().move(from, real(target), *options)
        }

        override fun isSameFile(path: Path, path2: Path): Boolean {
            val a = real(path)
            return a.realProvider().isSameFile(a, real(path2))
        }

        override fun isHidden(path: Path): Boolean {
            val target = real(path)
            return target.realProvider().isHidden(target)
        }

        override fun getFileStore(path: Path): FileStore {
            val target = real(path)
            return target.realProvider().getFileStore(target)
        }

        override fun checkAccess(path: Path, vararg modes: AccessMode) {
            val target = real(path)
            target.realProvider().checkAccess(target, *modes)
        }

        override fun <V : FileAttributeView> getFileAttributeView(path: Path, type: Class<V>, vararg options: LinkOption): V? {
            val target = real(path)
            return target.realProvider().getFileAttributeView(target, type, *options)
        }

        override fun <A : BasicFileAttributes> readAttributes(path: Path, type: Class<A>, vararg options: LinkOption): A {
            val target = real(path)
            return target.realProvider().readAttributes(target, type, *options)
        }

        override fun readAttributes(path: Path, attributes: String, vararg options: LinkOption): Map<String, Any?> {
            val target = real(path)
            return target.realProvider().readAttributes(target, attributes, *options)
        }

        override fun setAttribute(path: Path, attribute: String, value: Any?, vararg options: LinkOption) {
            val target = real(path)
            target.realProvider().setAttribute(target, attribute, value, *options)
        }
    }
}
