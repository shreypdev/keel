package dev.undra.android

import dev.undra.runtime.UndraPortException
import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.decodeAll
import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** The Fs adapter over a real directory: what it reads and writes, how it reports failures, and that it stays inside its root. */
class AndroidFsAdapterTest {
    private lateinit var outer: File
    private lateinit var root: File
    private lateinit var fs: AndroidFsAdapter

    @Before
    fun setUp() {
        outer = scratchDir("fs")
        root = File(outer, "root")
        fs = AndroidFsAdapter(root)
    }

    @After
    fun tearDown() {
        outer.deleteRecursively()
    }

    private fun failure(block: suspend () -> Any?): FsError {
        try {
            runBlocking { block() }
        } catch (e: FsError) {
            return e
        }
        throw AssertionError("expected an FsError")
    }

    @Test
    fun a_file_round_trips_and_write_creates_the_directories_it_needs() = runBlocking {
        fs.write("notes/2026/october/a.txt", "hello".toByteArray())
        assertEquals("hello", String(fs.read("notes/2026/october/a.txt")))
        assertTrue(File(root, "notes/2026/october/a.txt").isFile)
    }

    @Test
    fun write_replaces_and_a_leading_slash_means_the_root() = runBlocking {
        fs.write("/a.txt", byteArrayOf(1))
        fs.write("a.txt", byteArrayOf(2, 3))
        assertArrayEquals(byteArrayOf(2, 3), fs.read("/a.txt"))
        assertEquals(listOf("a.txt"), fs.list(""))
    }

    @Test
    fun binary_and_empty_and_large_content_survive() = runBlocking {
        val everyByte = ByteArray(256) { it.toByte() }
        fs.write("bin", everyByte)
        assertArrayEquals(everyByte, fs.read("bin"))
        fs.write("empty", ByteArray(0))
        assertEquals(0, fs.read("empty").size)
        val big = ByteArray(4 * 1024 * 1024) { (it * 5).toByte() }
        fs.write("big", big)
        assertArrayEquals(big, fs.read("big"))
    }

    @Test
    fun list_gives_the_sorted_names_of_files_and_directories_without_slashes() = runBlocking {
        fs.write("b.txt", byteArrayOf())
        fs.write("a.txt", byteArrayOf())
        fs.write("dir/inner.txt", byteArrayOf())
        assertEquals(listOf("a.txt", "b.txt", "dir"), fs.list(""))
        assertEquals(listOf("a.txt", "b.txt", "dir"), fs.list("/"))
        assertEquals(listOf("inner.txt"), fs.list("dir"))
        assertEquals(listOf("inner.txt"), fs.list("dir/"))
        assertEquals(listOf("inner.txt"), fs.list("./dir"))
    }

    @Test
    fun a_missing_file_or_directory_is_not_found() {
        assertEquals(FsError.NotFound, failure { fs.read("nope.txt") })
        assertEquals(FsError.NotFound, failure { fs.read("dir/nope.txt") })
        assertEquals(FsError.NotFound, failure { fs.list("nope") })
        assertEquals(FsError.NotFound, failure { fs.delete("nope") })
    }

    @Test
    fun reading_a_directory_and_listing_a_file_are_io_errors() = runBlocking {
        fs.write("dir/f.txt", byteArrayOf(1))
        val read = failure { fs.read("dir") }
        assertTrue(read.toString(), read is FsError.Io)
        val list = failure { fs.list("dir/f.txt") }
        assertTrue(list.toString(), list is FsError.Io)
    }

    @Test
    fun writing_over_a_directory_or_below_a_file_is_an_io_error() = runBlocking {
        fs.write("dir/f.txt", byteArrayOf(1))
        assertTrue(failure { fs.write("dir", byteArrayOf(1)) } is FsError.Io)
        assertTrue(failure { fs.write("dir/f.txt/below", byteArrayOf(1)) } is FsError.Io)
        assertTrue(failure { fs.write("", byteArrayOf(1)) } is FsError.Io)
    }

    @Test
    fun delete_removes_a_file_and_a_directory_with_everything_in_it() = runBlocking {
        fs.write("keep.txt", byteArrayOf(1))
        fs.write("tree/a.txt", byteArrayOf(1))
        fs.write("tree/sub/b.txt", byteArrayOf(2))
        fs.write("tree/sub/deeper/c.txt", byteArrayOf(3))
        fs.delete("keep.txt")
        assertEquals(listOf("tree"), fs.list(""))
        fs.delete("tree")
        assertEquals(emptyList<String>(), fs.list(""))
        assertFalse(File(root, "tree").exists())
    }

    @Test
    fun the_root_cannot_be_deleted() = runBlocking {
        fs.write("a.txt", byteArrayOf(1))
        // the backslash forms resolve to the root too (FsAdapter ignores leading `/` and `\`), so they must be refused alike
        for (path in listOf("", "/", ".", "./", "//", "/./", "\\", "\\\\", "\\/", "/\\", "\\.")) {
            assertEquals("'$path'", FsError.Denied, failure { fs.delete(path) })
        }
        assertEquals(listOf("a.txt"), fs.list(""))
        assertTrue(root.isDirectory)
    }

    @Test
    fun a_nul_byte_in_a_path_is_a_typed_io_error_not_an_untyped_exception() = runBlocking {
        fs.write("a.txt", byteArrayOf(1))
        for (path in listOf("a\u0000.txt", "\u0000", "dir/\u0000/x")) {
            assertTrue("read $path", failure { fs.read(path) } is FsError.Io)
            assertTrue("write $path", failure { fs.write(path, byteArrayOf(1)) } is FsError.Io)
            assertTrue("delete $path", failure { fs.delete(path) } is FsError.Io)
            assertTrue("list $path", failure { fs.list(path) } is FsError.Io)
        }
        assertEquals(listOf("a.txt"), fs.list(""))
    }

    @Test
    fun deleting_a_symbolic_link_to_a_directory_removes_the_link_and_not_what_it_points_at() = runBlocking {
        fs.write("target/keep.txt", byteArrayOf(1))
        fs.write("target/sub/deep.txt", byteArrayOf(2))
        Files.createSymbolicLink(File(root, "link").toPath(), File(root, "target").toPath())
        assertEquals(listOf("keep.txt", "sub"), fs.list("link")) // a link inside the root may be read through
        fs.delete("link")
        assertFalse(Files.isSymbolicLink(File(root, "link").toPath()))
        assertEquals(listOf("keep.txt", "sub"), fs.list("target"))
        assertArrayEquals(byteArrayOf(2), fs.read("target/sub/deep.txt"))
        assertEquals(listOf("target"), fs.list(""))
    }

    @Test
    fun a_path_with_a_parent_component_is_denied_everywhere() = runBlocking {
        fs.write("inside/f.txt", byteArrayOf(1))
        File(outer, "secret.txt").writeText("outside the root")
        for (path in listOf("../secret.txt", "inside/../../secret.txt", "..", "inside/..", "/../secret.txt", "a/../b")) {
            assertEquals("read $path", FsError.Denied, failure { fs.read(path) })
            assertEquals("write $path", FsError.Denied, failure { fs.write(path, byteArrayOf(1)) })
            assertEquals("delete $path", FsError.Denied, failure { fs.delete(path) })
            assertEquals("list $path", FsError.Denied, failure { fs.list(path) })
        }
        assertEquals("outside the root", File(outer, "secret.txt").readText())
        assertFalse(File(outer, "b").exists())
    }

    @Test
    fun a_symbolic_link_out_of_the_root_is_not_followed() = runBlocking {
        fs.write("inside/f.txt", byteArrayOf(1))
        val secretDir = File(outer, "elsewhere").also { it.mkdirs() }
        File(secretDir, "secret.txt").writeText("secret")
        Files.createSymbolicLink(File(root, "inside/link").toPath(), secretDir.toPath())
        Files.createSymbolicLink(File(root, "filelink").toPath(), File(secretDir, "secret.txt").toPath())
        assertEquals(FsError.Denied, failure { fs.read("inside/link/secret.txt") })
        assertEquals(FsError.Denied, failure { fs.read("filelink") })
        assertEquals(FsError.Denied, failure { fs.write("inside/link/new.txt", byteArrayOf(1)) })
        assertEquals(FsError.Denied, failure { fs.list("inside/link") })
        assertFalse(File(secretDir, "new.txt").exists())
        // deleting the link itself would be harmless, but the way there goes through the same resolution and is refused too
        assertEquals("secret", File(secretDir, "secret.txt").readText())
    }

    @Test
    fun the_port_methods_round_trip_and_fail_with_the_encoded_fs_error() {
        val impl = fs.portImpl()
        assertFalse(impl.sync)
        assertEquals(0, call(impl, StandardPorts.Fs.WRITE, argsOf("docs/a.txt", "hi".toByteArray())).size)
        assertEquals("hi", String(Codecs.bytes.decodeAll(call(impl, StandardPorts.Fs.READ, argsOf("docs/a.txt")))))
        assertEquals(listOf("a.txt"), Codecs.vec(Codecs.string).decodeAll(call(impl, StandardPorts.Fs.LIST, argsOf("docs"))))
        assertEquals(0, call(impl, StandardPorts.Fs.DELETE, argsOf("docs")).size)
        for ((method, expected) in listOf(
            StandardPorts.Fs.READ to FsError.NotFound,
            StandardPorts.Fs.LIST to FsError.NotFound,
            StandardPorts.Fs.DELETE to FsError.NotFound,
        )) {
            val e = try {
                call(impl, method, argsOf("docs/a.txt"))
                null
            } catch (e: UndraPortException) {
                e
            }
            assertEquals(expected, FsError.decodeAll(e!!.body))
        }
        val denied = try {
            call(impl, StandardPorts.Fs.READ, argsOf("../x"))
            null
        } catch (e: UndraPortException) {
            e
        }
        assertEquals(FsError.Denied, FsError.decodeAll(denied!!.body))
    }
}
