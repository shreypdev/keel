package dev.undra.runtime

import dev.undra.runtime.adapters.FileKv
import dev.undra.runtime.adapters.FsAdapter
import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.TempDir
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.nio.file.Files
import java.nio.file.attribute.PosixFilePermissions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private fun call(impl: PortImpl, method: UInt, args: ByteArray = NO_BYTES): ByteArray = runBlocking { impl.methods.getValue(method)(args) }

private fun str(s: String): ByteArray = UndraWriter().also { it.writeStr(s) }.toByteArray()

private fun bytes(vararg b: Int): ByteArray = ByteArray(b.size) { b[it].toByte() }

private fun listOfStrings(bytes: ByteArray): List<String> = Codecs.vec(Codecs.string).decodeAll(bytes)

class FileAdapterTests : Suite() {
    init {
        case("Kv stores, reads, overwrites, deletes and lists") {
            TempDir().use { dir ->
                val kv = FileKv(dir.path.resolve("kv"))
                runBlocking {
                    assertEq(null, kv.get("missing"))
                    assertEq(emptyList<String>(), kv.list(""))
                    kv.set("a", bytes(1, 2))
                    kv.set("b", bytes(3))
                    assertEq(listOf<Byte>(1, 2), kv.get("a")!!.toList())
                    kv.set("a", bytes(9))
                    assertEq(listOf<Byte>(9), kv.get("a")!!.toList())
                    assertEq(listOf("a", "b"), kv.list(""))
                    kv.delete("a")
                    assertEq(null, kv.get("a"))
                    kv.delete("a") // deleting a missing key is fine
                    assertEq(listOf("b"), kv.list(""))
                }
            }
        }

        case("Kv keys can be anything: empty, unicode, slashes, dots, very long, differing only by case") {
            TempDir().use { dir ->
                val kv = FileKv(dir.path)
                val keys = listOf("", "..", ".", "a/b/../c", "héllo wörld 🌊", "todos:{page}", "A", "a", "k".repeat(5000), "line\nbreak", "\u0000nul")
                runBlocking {
                    for ((i, k) in keys.withIndex()) kv.set(k, bytes(i))
                    for ((i, k) in keys.withIndex()) assertEq(listOf(i.toByte()), kv.get(k)!!.toList(), "key #$i")
                    assertEq(keys.sorted(), kv.list(""))
                }
                // Every file lives directly in the directory: no key can escape it or nest.
                assertEq(keys.size, Files.list(dir.path).use { it.count() }.toInt())
            }
        }

        case("Kv values can be empty or large") {
            TempDir().use { dir ->
                val kv = FileKv(dir.path)
                val big = ByteArray(5_000_000) { (it * 31).toByte() }
                runBlocking {
                    kv.set("empty", NO_BYTES)
                    kv.set("big", big)
                    assertEq(0, kv.get("empty")!!.size)
                    assertTrue(kv.get("big")!!.contentEquals(big))
                }
            }
        }

        case("Kv list returns only keys with the prefix, sorted") {
            TempDir().use { dir ->
                val kv = FileKv(dir.path)
                runBlocking {
                    for (k in listOf("todo:2", "todo:1", "user:1", "todo", "tod", "Todo:1")) kv.set(k, NO_BYTES)
                    assertEq(listOf("todo", "todo:1", "todo:2"), kv.list("todo"))
                    assertEq(listOf("todo:1", "todo:2"), kv.list("todo:"))
                    assertEq(emptyList<String>(), kv.list("zzz"))
                    assertEq(6, kv.list("").size)
                }
            }
        }

        case("Kv list reads the key at the start of each file, never the value, and skips what is not an entry") {
            TempDir().use { dir ->
                val kv = FileKv(dir.path)
                runBlocking {
                    kv.set("small", bytes(1))
                    kv.set("héllo wörld 🌊", bytes(2))
                    kv.set("", bytes(3))
                    kv.set("k".repeat(5000), bytes(4))
                }
                // Not entries: empty, too short for a length, a length that does not fit the file, a key that is not UTF-8.
                Files.write(dir.path.resolve("empty"), NO_BYTES)
                Files.write(dir.path.resolve("short"), bytes(1, 0))
                Files.write(dir.path.resolve("overlong"), bytes(255, 255, 255, 255, 1, 2, 3))
                Files.write(dir.path.resolve("fits-but-not-utf8"), bytes(2, 0, 0, 0, 0xC3, 0x28))
                Files.write(dir.path.resolve("pending.0000.tmp"), UndraWriter().also { it.writeStr("half-written") }.toByteArray())
                val expected = listOf("", "héllo wörld 🌊", "k".repeat(5000), "small").sorted()
                assertEq(expected, runBlocking { kv.list("") })
                assertEq(listOf("small"), runBlocking { kv.list("sm") })

                // A value no read could hold: with the whole file read, this would be an OutOfMemoryError. A sparse file
                // costs no disk where the file system has them (APFS, ext4, tmpfs, overlayfs); elsewhere the case is skipped.
                val huge = dir.path.resolve("huge-value")
                val sparse = try {
                    java.io.RandomAccessFile(huge.toFile(), "rw").use { file ->
                        file.write(UndraWriter().also { it.writeStr("huge") }.toByteArray())
                        file.setLength(3L shl 30)
                    }
                    Files.getFileStore(huge).type() in setOf("apfs", "ext4", "tmpfs", "overlay", "xfs", "btrfs", "zfs")
                } catch (e: java.io.IOException) {
                    false
                }
                if (sparse) assertEq((expected + "huge").sorted(), runBlocking { kv.list("") })
                Files.deleteIfExists(huge)
            }
        }

        case("Kv persists across instances, leaves no temporary files, and is owner-only where POSIX") {
            TempDir().use { dir ->
                runBlocking { FileKv(dir.path).set("k", bytes(5)) }
                assertEq(listOf<Byte>(5), runBlocking { FileKv(dir.path).get("k") }!!.toList())
                val names: List<String> = Files.list(dir.path).use { s -> s.map { p -> p.fileName.toString() }.collect(java.util.stream.Collectors.toList()) }
                assertEq(1, names.size)
                assertTrue(names.none { it.endsWith(".tmp") }, "$names")
                try {
                    val perms = Files.getPosixFilePermissions(dir.path.resolve(names.single()))
                    assertEq(PosixFilePermissions.fromString("rw-------"), perms)
                } catch (e: UnsupportedOperationException) {
                    // not a POSIX file system
                }
            }
        }

        case("Kv survives concurrent writers and readers of the same key") {
            TempDir().use { dir ->
                val kv = FileKv(dir.path)
                runBlocking {
                    val writers = List(50) { i -> async(Dispatchers.Default) { kv.set("k", ByteArray(1000) { i.toByte() }) } }
                    val readers = List(50) { async(Dispatchers.Default) { kv.get("k") } }
                    writers.awaitAll()
                    for (value in readers.awaitAll()) {
                        // Either not written yet, or one writer's complete value: never a torn mix.
                        assertTrue(value == null || (value.size == 1000 && value.all { it == value[0] }), "torn read")
                    }
                }
            }
        }

        case("Kv as a port: methods decode arguments and encode replies exactly") {
            TempDir().use { dir ->
                val impl = FileKv(dir.path).portImpl("Kv")
                assertTrue(!impl.sync)
                val set = UndraWriter().also { it.writeStr("k"); it.writeBytes(bytes(1, 2)) }.toByteArray()
                assertEq(0, call(impl, StandardPorts.Kv.SET, set).size)
                assertEq(Codecs.option(Codecs.bytes).encodeToByteArray(bytes(1, 2)).toList(), call(impl, StandardPorts.Kv.GET, str("k")).toList())
                assertEq(listOf<Byte>(0), call(impl, StandardPorts.Kv.GET, str("none")).toList())
                assertEq(listOf("k"), listOfStrings(call(impl, StandardPorts.Kv.LIST, str(""))))
                assertEq(0, call(impl, StandardPorts.Kv.DELETE, str("k")).size)
                assertEq(emptyList<String>(), listOfStrings(call(impl, StandardPorts.Kv.LIST, str(""))))
                assertThrows<WireException> { call(impl, StandardPorts.Kv.GET, NO_BYTES) }
                assertThrows<WireException> { call(impl, StandardPorts.Kv.GET, str("k") + byteArrayOf(0)) }
                assertThrows<IllegalArgumentException> { FileKv(dir.path).portImpl("Other") }
            }
        }

        case("SecureStore answers under its own ids and is separate from Kv") {
            TempDir().use { dir ->
                val secure = FileKv(dir.path.resolve("secure")).portImpl("SecureStore")
                val kv = FileKv(dir.path.resolve("kv")).portImpl("Kv")
                val set = UndraWriter().also { it.writeStr("token"); it.writeBytes(bytes(7)) }.toByteArray()
                call(secure, StandardPorts.SecureStore.SET, set)
                assertEq(Codecs.option(Codecs.bytes).encodeToByteArray(bytes(7)).toList(), call(secure, StandardPorts.SecureStore.GET, str("token")).toList())
                assertEq(listOf<Byte>(0), call(kv, StandardPorts.Kv.GET, str("token")).toList())
                assertTrue(StandardPorts.Kv.GET !in secure.methods && StandardPorts.SecureStore.GET !in kv.methods)
            }
        }

        case("Fs writes, reads, lists and deletes under its root, creating parent directories") {
            TempDir().use { dir ->
                val fs = FsAdapter(dir.path.resolve("root"))
                runBlocking {
                    fs.write("a/b/c.txt", bytes(1, 2, 3))
                    fs.write("a/z.txt", bytes(4))
                    fs.write("top.txt", NO_BYTES)
                    assertEq(listOf<Byte>(1, 2, 3), fs.read("a/b/c.txt").toList())
                    assertEq(0, fs.read("top.txt").size)
                    assertEq(listOf("a", "top.txt"), fs.list(""))
                    assertEq(listOf("b", "z.txt"), fs.list("a"))
                    fs.write("a/z.txt", bytes(5)) // overwrite
                    assertEq(listOf<Byte>(5), fs.read("a/z.txt").toList())
                    fs.delete("a/z.txt")
                    assertEq(listOf("b"), fs.list("a"))
                    // A leading slash means "inside the root", not the real root of the disk.
                    fs.write("/abs.txt", bytes(8))
                    assertEq(listOf<Byte>(8), fs.read("abs.txt").toList())
                }
            }
        }

        case("Fs reports NotFound, Denied and Io as typed errors") {
            TempDir().use { dir ->
                val fs = FsAdapter(dir.path.resolve("root"))
                runBlocking {
                    fs.write("dir/file", bytes(1))
                    assertEq(FsError.NotFound, assertThrows<FsError.NotFound>("read missing") { fs.read("nope") })
                    assertEq(FsError.NotFound, assertThrows<FsError.NotFound>("delete missing") { fs.delete("nope") })
                    assertEq(FsError.NotFound, assertThrows<FsError.NotFound>("list missing") { fs.list("nope") })
                    assertThrows<FsError.Io>("read a directory") { fs.read("dir") }
                    assertThrows<FsError.Io>("list a file") { fs.list("dir/file") }
                    assertThrows<FsError.Io>("write over a directory") { fs.write("dir", bytes(1)) }
                }
            }
        }

        case("Fs delete removes a file, or a directory with everything in it, and never the root") {
            TempDir().use { dir ->
                val root = dir.path.resolve("root")
                val fs = FsAdapter(root)
                runBlocking {
                    fs.write("tree/a.txt", bytes(1))
                    fs.write("tree/sub/b.txt", bytes(2))
                    fs.write("tree/sub/deeper/c.txt", bytes(3))
                    fs.write("keep.txt", bytes(4))
                    fs.delete("tree") // populated: everything below goes with it
                    assertEq(listOf("keep.txt"), fs.list(""))
                    assertEq(FsError.NotFound, assertThrows<FsError.NotFound>("read what was deleted") { fs.read("tree/a.txt") })
                    assertEq(FsError.NotFound, assertThrows<FsError.NotFound>("delete it again") { fs.delete("tree") })
                    fs.write("empty/placeholder", bytes(0))
                    fs.delete("empty/placeholder")
                    fs.delete("empty") // an empty directory still goes
                    assertEq(listOf("keep.txt"), fs.list(""))

                    // The root cannot be deleted, in any spelling that resolves to it; its content stays.
                    for (path in listOf("", ".", "/", "\\", "\\.", "/\\", "a/..")) {
                        assertThrows<FsError.Denied>("delete '$path'") { fs.delete(path) }
                    }
                    assertTrue(Files.isDirectory(root), "the root is still there")
                    assertEq(listOf<Byte>(4), fs.read("keep.txt").toList())
                }
            }
        }

        case("Fs delete removes a symbolic link and never follows it") {
            TempDir().use { dir ->
                val root = dir.path.resolve("root")
                Files.createDirectories(root.resolve("inside"))
                Files.write(root.resolve("inside/file"), bytes(7))
                val outside = dir.path.resolve("outside")
                Files.createDirectories(outside)
                Files.write(outside.resolve("secret"), bytes(42))
                val linked = try {
                    Files.createSymbolicLink(root.resolve("to-inside"), root.resolve("inside"))
                    Files.createSymbolicLink(root.resolve("to-outside"), outside)
                    true
                } catch (e: UnsupportedOperationException) {
                    false
                } catch (e: java.io.IOException) {
                    false
                }
                if (linked) {
                    val fs = FsAdapter(root)
                    runBlocking {
                        fs.delete("to-inside") // a link to a directory inside the root: the link goes, the directory stays
                        assertTrue(!Files.exists(root.resolve("to-inside"), java.nio.file.LinkOption.NOFOLLOW_LINKS), "the link is gone")
                        assertEq(listOf<Byte>(7), Files.readAllBytes(root.resolve("inside/file")).toList())
                        // A link out of the root is refused like any other path through it, and nothing outside is touched.
                        assertThrows<FsError.Denied>("delete a link that leads out") { fs.delete("to-outside") }
                        assertThrows<FsError.Denied>("delete through a link that leads out") { fs.delete("to-outside/secret") }
                    }
                    assertEq(listOf<Byte>(42), Files.readAllBytes(outside.resolve("secret")).toList())
                    // A directory that contains a link takes the link with it and leaves the target alone.
                    Files.createDirectories(root.resolve("holder"))
                    Files.createSymbolicLink(root.resolve("holder/link"), root.resolve("inside"))
                    runBlocking { fs.delete("holder") }
                    assertTrue(!Files.exists(root.resolve("holder")), "the directory is gone")
                    assertEq(listOf<Byte>(7), Files.readAllBytes(root.resolve("inside/file")).toList())
                }
            }
        }

        case("Fs never leaves its root: dot-dot, absolute-looking and symlink escapes are Denied") {
            TempDir().use { dir ->
                val root = dir.path.resolve("root")
                Files.createDirectories(root)
                val outside = dir.path.resolve("outside")
                Files.createDirectories(outside)
                Files.write(outside.resolve("secret"), bytes(42))
                val fs = FsAdapter(root)
                runBlocking {
                    for (path in listOf("../outside/secret", "a/../../outside/secret", "..", "../", "x/../../..")) {
                        assertThrows<FsError.Denied>("read $path") { fs.read(path) }
                        assertThrows<FsError.Denied>("write $path") { fs.write(path, bytes(1)) }
                        assertThrows<FsError.Denied>("delete $path") { fs.delete(path) }
                        assertThrows<FsError.Denied>("list $path") { fs.list(path) }
                    }
                    // An absolute path is taken relative to the root, so it reaches a file inside, never /etc/passwd.
                    assertThrows<FsError.NotFound> { fs.read("/etc/passwd") }
                    val linked = try {
                        Files.createSymbolicLink(root.resolve("link"), outside)
                        true
                    } catch (e: UnsupportedOperationException) {
                        false
                    } catch (e: java.io.IOException) {
                        false
                    }
                    val dangling = linked && try {
                        Files.createSymbolicLink(root.resolve("dangling"), dir.path.resolve("does-not-exist"))
                        true
                    } catch (e: java.io.IOException) {
                        false
                    }
                    if (dangling) {
                        assertThrows<FsError.Denied>("write through a dangling link") { fs.write("dangling/x", bytes(1)) }
                        assertThrows<FsError.Denied>("read a dangling link") { fs.read("dangling") }
                    }
                    if (linked) {
                        assertThrows<FsError.Denied>("read through a symlink") { fs.read("link/secret") }
                        assertThrows<FsError.Denied>("write through a symlink") { fs.write("link/new", bytes(1)) }
                        assertTrue(!Files.exists(outside.resolve("new")), "nothing was written outside")
                        assertThrows<FsError.Denied>("list through a symlink") { fs.list("link") }
                    }
                }
                assertEq(listOf<Byte>(42), Files.readAllBytes(outside.resolve("secret")).toList())
            }
        }

        case("Fs as a port: results collapse to ok bodies and typed error replies") {
            TempDir().use { dir ->
                val impl = FsAdapter(dir.path.resolve("root")).portImpl()
                assertTrue(!impl.sync)
                val write = UndraWriter().also { it.writeStr("f"); it.writeBytes(bytes(1, 2)) }.toByteArray()
                assertEq(0, call(impl, StandardPorts.Fs.WRITE, write).size)
                assertEq(Codecs.bytes.encodeToByteArray(bytes(1, 2)).toList(), call(impl, StandardPorts.Fs.READ, str("f")).toList())
                assertEq(listOf("f"), listOfStrings(call(impl, StandardPorts.Fs.LIST, str(""))))
                val missing = assertThrows<UndraPortException> { call(impl, StandardPorts.Fs.READ, str("missing")) }
                assertEq(FsError.NotFound, FsError.decodeAll(missing.body))
                val denied = assertThrows<UndraPortException> { call(impl, StandardPorts.Fs.DELETE, str("../x")) }
                assertEq(FsError.Denied, FsError.decodeAll(denied.body))
                assertEq(0, call(impl, StandardPorts.Fs.DELETE, str("f")).size)
                assertThrows<WireException> { call(impl, StandardPorts.Fs.READ, NO_BYTES) }
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
