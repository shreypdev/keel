package dev.undra.android

import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.decodeAll
import java.io.File
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * The SecureStore adapter's storage and sealing with a software key in place of the Keystore's (the instrumented tests
 * cover the real Keystore): what lands on disk is sealed, names list, and a damaged file is an error, not a missing value.
 */
class AndroidSecureStoreAdapterTest {
    private lateinit var dir: File
    private val key: SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    private lateinit var secure: AndroidSecureStoreAdapter

    @Before
    fun setUp() {
        dir = scratchDir("secure")
        secure = AndroidSecureStoreAdapter(dir) { key }
    }

    @After
    fun tearDown() {
        dir.deleteRecursively()
    }

    private val secret = "correct horse battery staple".toByteArray()

    private fun diskContainsPlaintext(): Boolean {
        val needle = String(secret, Charsets.ISO_8859_1)
        return dir.walk().filter { it.isFile }.any { String(it.readBytes(), Charsets.ISO_8859_1).contains(needle) }
    }

    @Test
    fun a_secret_round_trips_and_is_not_on_disk_in_clear() = runBlocking {
        secure.set("session.token", secret)
        assertArrayEquals(secret, secure.get("session.token"))
        assertTrue(dir.walk().any { it.isFile })
        assertFalse(diskContainsPlaintext())
    }

    @Test
    fun a_missing_key_is_null_and_deleting_is_idempotent() = runBlocking {
        assertNull(secure.get("nothing"))
        secure.set("k", secret)
        secure.delete("k")
        secure.delete("k")
        assertNull(secure.get("k"))
    }

    @Test
    fun set_replaces_with_a_fresh_iv() = runBlocking {
        secure.set("k", secret)
        val first = dir.walk().first { it.isFile }.readBytes()
        secure.set("k", secret)
        val second = dir.walk().first { it.isFile }.readBytes()
        assertFalse(first.contentEquals(second))
        assertArrayEquals(secret, secure.get("k"))
    }

    @Test
    fun list_gives_the_names_with_the_prefix_sorted() = runBlocking {
        for (name in listOf("auth.refresh", "auth.access", "other")) secure.set(name, secret)
        assertEquals(listOf("auth.access", "auth.refresh"), secure.list("auth."))
        assertEquals(listOf("auth.access", "auth.refresh", "other"), secure.list(""))
    }

    @Test
    fun a_second_adapter_with_the_same_key_reads_what_the_first_wrote() = runBlocking {
        secure.set("k", secret)
        assertArrayEquals(secret, AndroidSecureStoreAdapter(dir) { key }.get("k"))
    }

    @Test
    fun an_adapter_with_another_key_cannot_read_it_and_says_so_instead_of_returning_null() = runBlocking {
        secure.set("k", secret)
        val other = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
        val e = try {
            AndroidSecureStoreAdapter(dir) { other }.get("k")
            null
        } catch (e: SecureStoreException) {
            e
        }
        assertTrue(e!!.message, e.message!!.contains("failed authentication"))
    }

    @Test
    fun a_file_that_was_altered_is_an_error() = runBlocking {
        secure.set("k", secret)
        val file = dir.walk().first { it.isFile }
        val bytes = file.readBytes()
        bytes[bytes.size - 1] = (bytes[bytes.size - 1].toInt() xor 1).toByte()
        file.writeBytes(bytes)
        assertThrows(SecureStoreException::class.java) { runBlocking { secure.get("k") } }
        Unit
    }

    @Test
    fun a_key_that_cannot_be_obtained_is_an_error_on_set_and_get() = runBlocking {
        secure.set("k", secret)
        val broken = AndroidSecureStoreAdapter(dir) { throw SecureStoreException("the Keystore is locked") }
        assertThrows(SecureStoreException::class.java) { runBlocking { broken.set("x", secret) } }
        assertThrows(SecureStoreException::class.java) { runBlocking { broken.get("k") } }
        Unit
    }

    @Test
    fun the_port_methods_round_trip_through_the_wire_encoding() {
        val impl = secure.portImpl()
        assertFalse(impl.sync)
        call(impl, StandardPorts.SecureStore.SET, argsOf("a.token", secret))
        call(impl, StandardPorts.SecureStore.SET, argsOf("b.token", byteArrayOf(7)))
        val option = Codecs.option(Codecs.bytes)
        assertArrayEquals(secret, option.decodeAll(call(impl, StandardPorts.SecureStore.GET, argsOf("a.token"))))
        assertNull(option.decodeAll(call(impl, StandardPorts.SecureStore.GET, argsOf("absent"))))
        assertEquals(listOf("a.token", "b.token"), Codecs.vec(Codecs.string).decodeAll(call(impl, StandardPorts.SecureStore.LIST, argsOf(""))))
        call(impl, StandardPorts.SecureStore.DELETE, argsOf("a.token"))
        assertNull(option.decodeAll(call(impl, StandardPorts.SecureStore.GET, argsOf("a.token"))))
        assertFalse(diskContainsPlaintext())
    }

    @Test
    fun a_failing_key_makes_the_port_method_throw_for_the_runtime_to_answer_unavailable() {
        val impl = AndroidSecureStoreAdapter(dir) { throw SecureStoreException("the Keystore is locked") }.portImpl()
        assertThrows(SecureStoreException::class.java) { call(impl, StandardPorts.SecureStore.SET, argsOf("k", secret)) }
    }
}
