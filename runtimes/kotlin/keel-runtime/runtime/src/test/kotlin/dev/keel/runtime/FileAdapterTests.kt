package dev.keel.runtime

import dev.keel.runtime.adapters.FileKv
import dev.keel.runtime.adapters.FsAdapter
import dev.keel.runtime.adapters.FsError
import dev.keel.runtime.adapters.StandardPorts
import dev.keel.runtime.support.NO_BYTES
import dev.keel.runtime.support.TempDir
import dev.keel.runtime.testing.Suite
import dev.keel.runtime.testing.assertEq
import dev.keel.runtime.testing.assertThrows
import dev.keel.runtime.testing.assertTrue
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.KeelWriter
import dev.keel.runtime.wire.WireException
import dev.keel.runtime.wire.decodeAll
import dev.keel.runtime.wire.encodeToByteArray
import java.nio.file.Files
import java.nio.file.attribute.PosixFilePermissions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private fun call(impl: PortImpl, method: UInt, args: ByteArray = NO_BYTES): ByteArray = runBlocking { impl.methods.getValue(method)(args) }

private fun str(s: String): ByteArray = KeelWriter().also { it.writeStr(s) }.toByteArray()

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
                val set = KeelWriter().also { it.writeStr("k"); it.writeBytes(bytes(1, 2)) }.toByteArray()
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
                val set = KeelWriter().also { it.writeStr("token"); it.writeBytes(bytes(7)) }.toByteArray()
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
                    assertThrows<FsError.Io>("delete a non-empty directory") { fs.delete("dir") }
                    assertThrows<FsError.Io>("list a file") { fs.list("dir/file") }
                    assertThrows<FsError.Io>("write over a directory") { fs.write("dir", bytes(1)) }
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
                val write = KeelWriter().also { it.writeStr("f"); it.writeBytes(bytes(1, 2)) }.toByteArray()
                assertEq(0, call(impl, StandardPorts.Fs.WRITE, write).size)
                assertEq(Codecs.bytes.encodeToByteArray(bytes(1, 2)).toList(), call(impl, StandardPorts.Fs.READ, str("f")).toList())
                assertEq(listOf("f"), listOfStrings(call(impl, StandardPorts.Fs.LIST, str(""))))
                val missing = assertThrows<KeelPortException> { call(impl, StandardPorts.Fs.READ, str("missing")) }
                assertEq(FsError.NotFound, FsError.decodeAll(missing.body))
                val denied = assertThrows<KeelPortException> { call(impl, StandardPorts.Fs.DELETE, str("../x")) }
                assertEq(FsError.Denied, FsError.decodeAll(denied.body))
                assertEq(0, call(impl, StandardPorts.Fs.DELETE, str("f")).size)
                assertThrows<WireException> { call(impl, StandardPorts.Fs.READ, NO_BYTES) }
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
