package dev.undra.android

import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.decodeAll
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** The Kv adapter over a real directory: round trips, listing, persistence across instances and the port methods. */
class AndroidKvAdapterTest {
    private lateinit var dir: File
    private lateinit var kv: AndroidKvAdapter

    @Before
    fun setUp() {
        dir = scratchDir("kv")
        kv = AndroidKvAdapter(dir)
    }

    @After
    fun tearDown() {
        dir.deleteRecursively()
    }

    @Test
    fun a_value_round_trips() = runBlocking {
        kv.set("greeting", "hello".toByteArray())
        assertEquals("hello", String(kv.get("greeting")!!))
    }

    @Test
    fun a_missing_key_is_null() = runBlocking {
        assertNull(kv.get("nothing"))
        kv.set("a", byteArrayOf(1))
        assertNull(kv.get("b"))
    }

    @Test
    fun set_replaces_and_an_empty_value_is_a_value() = runBlocking {
        kv.set("k", byteArrayOf(1, 2, 3))
        kv.set("k", byteArrayOf(9))
        assertArrayEquals(byteArrayOf(9), kv.get("k"))
        kv.set("k", ByteArray(0))
        assertArrayEquals(ByteArray(0), kv.get("k"))
        assertEquals(listOf("k"), kv.list(""))
    }

    @Test
    fun delete_removes_and_deleting_a_missing_key_is_fine() = runBlocking {
        kv.set("k", byteArrayOf(1))
        kv.delete("k")
        assertNull(kv.get("k"))
        kv.delete("k")
        kv.delete("never-existed")
        assertEquals(emptyList<String>(), kv.list(""))
    }

    @Test
    fun list_returns_the_keys_with_the_prefix_sorted() = runBlocking {
        for (key in listOf("undra.query.cache.b", "undra.query.queue", "undra.query.cache.a", "other", "undra.query.cache.c")) kv.set(key, byteArrayOf(0))
        assertEquals(listOf("undra.query.cache.a", "undra.query.cache.b", "undra.query.cache.c"), kv.list("undra.query.cache."))
        assertEquals(listOf("other", "undra.query.cache.a", "undra.query.cache.b", "undra.query.cache.c", "undra.query.queue"), kv.list(""))
        assertEquals(emptyList<String>(), kv.list("nope"))
        assertEquals(listOf("undra.query.queue"), kv.list("undra.query.queue"))
    }

    @Test
    fun list_on_a_store_nothing_was_written_to_is_empty() = runBlocking {
        assertEquals(emptyList<String>(), AndroidKvAdapter(File(dir, "never-created")).list(""))
    }

    @Test
    fun any_string_is_a_key() = runBlocking {
        val keys = listOf("", "a/b/c", "../escape", "with space", "ünïcödé ☕ 日本語", "\u0000nul", "x".repeat(10_000), "CASE", "case")
        for ((i, key) in keys.withIndex()) kv.set(key, byteArrayOf(i.toByte()))
        for ((i, key) in keys.withIndex()) assertArrayEquals("key #$i", byteArrayOf(i.toByte()), kv.get(key))
        assertEquals(keys.sorted(), kv.list(""))
        assertTrue(dir.walk().none { it.name.contains("..") })
    }

    @Test
    fun every_byte_value_survives_and_so_does_a_large_value() = runBlocking {
        val everyByte = ByteArray(256) { it.toByte() }
        kv.set("bytes", everyByte)
        assertArrayEquals(everyByte, kv.get("bytes"))
        val big = ByteArray(3 * 1024 * 1024) { (it * 13).toByte() }
        kv.set("big", big)
        assertArrayEquals(big, kv.get("big"))
    }

    @Test
    fun what_was_written_is_there_for_a_new_instance_of_the_adapter() = runBlocking {
        kv.set("durable", "yes".toByteArray())
        val again = AndroidKvAdapter(dir)
        assertEquals("yes", String(again.get("durable")!!))
        assertEquals(listOf("durable"), again.list(""))
    }

    @Test
    fun a_write_leaves_no_temporary_file_behind() = runBlocking {
        repeat(20) { kv.set("k", ByteArray(100) { b -> (b + it).toByte() }) }
        assertEquals(1, dir.listFiles()!!.size)
        assertFalse(dir.listFiles()!!.any { it.name.endsWith(".tmp") })
    }

    @Test
    fun concurrent_writers_do_not_corrupt_each_other() = runBlocking {
        (1..50).map { i -> async(Dispatchers.Default) { repeat(5) { n -> kv.set("key-$i", "value-$i-$n".toByteArray()) } } }.awaitAll()
        for (i in 1..50) assertEquals("value-$i-4", String(kv.get("key-$i")!!))
        assertEquals(50, kv.list("key-").size)
    }

    @Test
    fun concurrent_writes_to_one_key_leave_one_whole_value() = runBlocking {
        (1..30).map { i -> async(Dispatchers.Default) { kv.set("same", ByteArray(5_000) { i.toByte() }) } }.awaitAll()
        val value = kv.get("same")!!
        assertEquals(5_000, value.size)
        assertTrue("the value is one writer's, not a mixture", value.all { it == value[0] })
    }

    @Test
    fun the_port_methods_round_trip_through_the_wire_encoding() {
        val impl = kv.portImpl()
        assertFalse(impl.sync)
        call(impl, StandardPorts.Kv.SET, argsOf("q.one", byteArrayOf(1, 2)))
        call(impl, StandardPorts.Kv.SET, argsOf("q.two", byteArrayOf(3)))
        call(impl, StandardPorts.Kv.SET, argsOf("other", byteArrayOf(4)))
        val option = Codecs.option(Codecs.bytes)
        assertArrayEquals(byteArrayOf(1, 2), option.decodeAll(call(impl, StandardPorts.Kv.GET, argsOf("q.one"))))
        assertNull(option.decodeAll(call(impl, StandardPorts.Kv.GET, argsOf("absent"))))
        assertEquals(listOf("q.one", "q.two"), Codecs.vec(Codecs.string).decodeAll(call(impl, StandardPorts.Kv.LIST, argsOf("q."))))
        assertEquals(0, call(impl, StandardPorts.Kv.DELETE, argsOf("q.one")).size)
        assertNull(option.decodeAll(call(impl, StandardPorts.Kv.GET, argsOf("q.one"))))
    }
}
