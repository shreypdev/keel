package dev.undra.android

import dev.undra.runtime.UndraPortException
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.StorageError
import dev.undra.runtime.wire.decodeAll
import java.io.File
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * The real SecureStore adapter, with its real Keystore key source, where there is no Android Keystore (a desktop JVM):
 * every operation that needs the key is [StorageError.Unavailable] (ADR-049, "Keystore missing"), through the port too,
 * and the operations that need no key still work. On a device the Keystore exists; `SecureStoreOnDeviceTest` covers it.
 */
class NoKeystoreTest {
    private lateinit var dir: File

    @Before
    fun setUp() {
        dir = scratchDir("no-keystore")
    }

    @After
    fun tearDown() {
        dir.deleteRecursively()
    }

    @Test
    fun without_an_android_keystore_get_and_set_are_unavailable_and_say_why() = runBlocking {
        val secure = AndroidSecureStoreAdapter(dir, "dev.undra.test.no-keystore")
        val set = assertThrows(StorageError.Unavailable::class.java) { runBlocking { secure.set("k", byteArrayOf(1)) } }
        assertTrue(set.reason, set.reason.contains("Android Keystore is not available"))
        // A value that is there (written by another build, say) cannot be opened without the key either.
        AndroidKvAdapter(dir).set("k", byteArrayOf(1, 2, 3))
        assertThrows(StorageError.Unavailable::class.java) { runBlocking { secure.get("k") } }

        val port = assertThrows(UndraPortException::class.java) { call(secure.portImpl(), StandardPorts.SecureStore.SET, argsOf("k", byteArrayOf(1))) }
        assertTrue(StorageError.decodeAll(port.body) is StorageError.Unavailable)
        assertTrue("Unavailable is transient: the core may ask again", StorageError.decodeAll(port.body).isTransient)

        // Names need no key.
        assertEquals(listOf("k"), secure.list(""))
        secure.delete("k")
        assertEquals(emptyList<String>(), secure.list(""))
    }
}
