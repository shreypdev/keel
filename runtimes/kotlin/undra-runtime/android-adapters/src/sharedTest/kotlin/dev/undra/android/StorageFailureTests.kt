package dev.undra.android

import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.UserNotAuthenticatedException
import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.StorageError
import dev.undra.runtime.adapters.StoragePort
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import dev.undra.testsupport.FaultyFileSystem
import java.io.File
import java.io.IOException
import java.security.NoSuchProviderException
import java.security.ProviderException
import javax.crypto.AEADBadTagException
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/**
 * ADR-049's failure-injection suite for the Android storage adapters (the runtime, Swift and TypeScript have one of the
 * same name), run as JVM unit tests and on the device. The real adapters run over a file system that fails on demand the
 * way the platform does ([FaultyFileSystem]) and over a key source that throws what the Keystore throws, and every port
 * method is checked for the exact typed reply the core receives: the runtime sends an [UndraPortException]'s body with
 * port status 1 (its `StorageFailureTests` checks those bytes on the wire, and status 2 for anything untyped).
 *
 * | Failure | Kv | SecureStore | Fs |
 * |---|---|---|---|
 * | full disk (`ENOSPC`, `EDQUOT`) | `Full` | `Full` | `FsError.Full` |
 * | other I/O (`EIO`) | `Io` | `Io` | `FsError.Io` |
 * | stored bytes that do not decode or open | `Corrupt` | `Corrupt` | — |
 * | key needs user authentication | — | `Locked` | — |
 * | key permanently invalidated | — | `Corrupt` | — |
 * | no Android Keystore | — | `Unavailable` | — |
 */
class StorageFailureTests {
    private lateinit var dir: File
    private lateinit var disk: FaultyFileSystem
    private val key: SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    private val secret = "a token that must survive".toByteArray()

    @Before
    fun setUp() {
        dir = scratchDir("storage-failures")
        disk = FaultyFileSystem(dir.toPath())
    }

    @After
    fun tearDown() {
        disk.heal()
        dir.deleteRecursively()
    }

    private fun kv() = AndroidKvAdapter(disk.root.resolve("kv"))

    private fun secure(keys: AndroidSecureStoreAdapter.SecretKeySource = AndroidSecureStoreAdapter.SecretKeySource { key }) =
        AndroidSecureStoreAdapter(disk.root.resolve("secure"), keys)

    private fun fs() = AndroidFsAdapter(disk.root.resolve("fs"))

    /** A full disk, as this platform words it. */
    private fun noSpace(): IOException = FaultyFileSystem.noSpace(android = isAndroidRuntime)

    /** The body of the typed reply [methodId] of [impl] answers [args] with; fails if it succeeds or throws anything else. */
    private fun typedReply(impl: PortImpl, methodId: UInt, args: ByteArray): ByteArray {
        try {
            call(impl, methodId, args)
        } catch (e: UndraPortException) {
            return e.body
        }
        fail("method 0x${methodId.toString(16)} succeeded; a typed failure was expected")
        throw AssertionError()
    }

    private fun storage(error: StorageError): ByteArray = StorageError.encodeToByteArray(error)

    /** Each method of [port] with arguments for the key `k`. */
    private fun methods(port: StoragePort): List<Pair<UInt, ByteArray>> =
        listOf(port.getId to argsOf("k"), port.setId to argsOf("k", byteArrayOf(9)), port.deleteId to argsOf("k"), port.listId to argsOf(""))

    // ---- the file system ---------------------------------------------------------------------------------------------

    @Test
    fun a_full_disk_or_quota_is_Full_on_every_write_of_Kv_and_SecureStore_and_the_old_value_stays() = runBlocking {
        for ((port, impl, read) in listOf(
            Triple(StoragePort.KV, kv().portImpl(), suspend { kv().get("k") }),
            Triple(StoragePort.SECURE_STORE, secure().portImpl(), suspend { secure().get("k") }),
        )) {
            call(impl, port.setId, argsOf("k", secret))
            for (error in listOf(noSpace(), FaultyFileSystem.quotaExceeded())) {
                disk.fail(FaultyFileSystem.Op.WRITE) { error }
                assertArrayEquals("${port.traitName}.set on $error", byteArrayOf(1, 0), typedReply(impl, port.setId, argsOf("k", byteArrayOf(9))))
                disk.heal()
            }
            assertArrayEquals("${port.traitName}: the old value", secret, read())
        }
        assertTrue("no temporary file is left behind", dir.walk().none { it.name.endsWith(".tmp") })
    }

    @Test
    fun the_first_write_into_a_directory_a_full_disk_cannot_create_is_Full() = runBlocking {
        disk.fail(FaultyFileSystem.Op.WRITE) { noSpace() }
        assertThrows(StorageError.Full::class.java) { runBlocking { kv().set("k", secret) } }
        assertThrows(StorageError.Full::class.java) { runBlocking { secure().set("k", secret) } }
        assertFalse(File(dir, "kv").exists())
        disk.heal()
        kv().set("k", secret)
        assertArrayEquals(secret, kv().get("k"))
    }

    @Test
    fun every_other_io_failure_is_Io_with_the_platforms_message_for_every_method_of_Kv_and_SecureStore() = runBlocking {
        val io = storage(StorageError.Io("Input/output error"))
        for ((port, impl) in listOf(StoragePort.KV to kv().portImpl(), StoragePort.SECURE_STORE to secure().portImpl())) {
            call(impl, port.setId, argsOf("k", secret))
            val faults = listOf(
                FaultyFileSystem.Op.READ to (port.getId to argsOf("k")),
                FaultyFileSystem.Op.WRITE to (port.setId to argsOf("k", byteArrayOf(9))),
                FaultyFileSystem.Op.MOVE to (port.setId to argsOf("k", byteArrayOf(9))),
                FaultyFileSystem.Op.DELETE to (port.deleteId to argsOf("k")),
                FaultyFileSystem.Op.LIST to (port.listId to argsOf("")),
            )
            for ((op, method) in faults) {
                disk.fail(op) { FaultyFileSystem.ioError() }
                assertArrayEquals("${port.traitName} on a failing $op", io, typedReply(impl, method.first, method.second))
                disk.heal()
            }
            // Healed: the same calls succeed with the bodies they always had.
            val option = Codecs.option(Codecs.bytes)
            assertArrayEquals(secret, option.decodeAll(call(impl, port.getId, argsOf("k"))))
            assertEquals(listOf("k"), Codecs.vec(Codecs.string).decodeAll(call(impl, port.listId, argsOf(""))))
            assertEquals(0, call(impl, port.deleteId, argsOf("k")).size)
        }
    }

    @Test
    fun a_damaged_entry_is_Corrupt_for_Kv_and_SecureStore_and_stays_on_disk() = runBlocking {
        kv().set("k", secret)
        secure().set("k", secret)
        for (sub in listOf("kv", "secure")) {
            val file = File(dir, sub).listFiles { f -> f.isFile && !f.name.startsWith(".") }!!.single()
            file.writeBytes(byteArrayOf(0x40, 0, 0, 0, 0x6b)) // claims a 64-byte key, holds one byte
        }
        for ((port, impl) in listOf(StoragePort.KV to kv().portImpl(), StoragePort.SECURE_STORE to secure().portImpl())) {
            val error = StorageError.decodeAll(typedReply(impl, port.getId, argsOf("k")))
            assertTrue("${port.traitName}: $error", error is StorageError.Corrupt)
        }
        // A sealed value that decodes as an entry but was altered is Corrupt too, and so is one in another format.
        secure().set("sealed", secret)
        val sealedFile = File(dir, "secure").listFiles { f -> f.isFile && !f.name.startsWith(".") }!!.first { it.readBytes().size > 30 }
        val bytes = sealedFile.readBytes()
        bytes[bytes.size - 1] = (bytes[bytes.size - 1].toInt() xor 1).toByte()
        sealedFile.writeBytes(bytes)
        val altered = StorageError.decodeAll(typedReply(secure().portImpl(), StandardPorts.SecureStore.GET, argsOf("sealed")))
        assertTrue("$altered", altered is StorageError.Corrupt && altered.reason.contains("failed authentication"))
        assertFalse("the reason never carries the value", altered.message!!.contains(String(secret)))
        // Nothing was deleted: both damaged entries and the altered one are still on disk.
        assertEquals(1, File(dir, "kv").listFiles()!!.size)
        assertEquals(2, File(dir, "secure").listFiles { f -> f.isFile && !f.name.startsWith(".") }!!.size)
    }

    // ---- the Keystore ------------------------------------------------------------------------------------------------

    @Test
    fun what_the_keystore_throws_is_Locked_Corrupt_Unavailable_or_Io_on_get_and_set() = runBlocking {
        secure().set("k", secret)
        val cases = listOf<Pair<() -> Exception, StorageError>>(
            { UserNotAuthenticatedException() } to StorageError.Locked,
            { NoSuchProviderException("AndroidKeyStore") } to StorageError.Unavailable("the Android Keystore is not available: AndroidKeyStore"),
            { StorageError.Unavailable("the Android Keystore is not available: KeyStoreException") } to
                StorageError.Unavailable("the Android Keystore is not available: KeyStoreException"),
            { IOException("write failed: ENOSPC (No space left on device)") } to StorageError.Full,
        )
        for ((thrown, expected) in cases) {
            val impl = secure { throw thrown() }.portImpl()
            assertArrayEquals("get: $expected", storage(expected), typedReply(impl, StandardPorts.SecureStore.GET, argsOf("k")))
            assertArrayEquals("set: $expected", storage(expected), typedReply(impl, StandardPorts.SecureStore.SET, argsOf("k", byteArrayOf(9))))
        }
        // A key invalidated for good: Corrupt, on both.
        val invalidated = secure { throw KeyPermanentlyInvalidatedException() }.portImpl()
        for ((method, args) in listOf(StandardPorts.SecureStore.GET to argsOf("k"), StandardPorts.SecureStore.SET to argsOf("k", byteArrayOf(9)))) {
            val error = StorageError.decodeAll(typedReply(invalidated, method, args))
            assertTrue("$error", error is StorageError.Corrupt && error.reason.contains("permanently invalidated"))
        }
        // Anything else the Keystore reports is Io, with what it said.
        val provider = StorageError.decodeAll(typedReply(secure { throw ProviderException("Keystore operation failed") }.portImpl(), StandardPorts.SecureStore.GET, argsOf("k")))
        assertTrue("$provider", provider is StorageError.Io && provider.reason.contains("Keystore operation failed"))
        // Names are not secret: list and delete need no key, so a locked Keystore does not stop them.
        val locked = secure { throw UserNotAuthenticatedException() }.portImpl()
        assertEquals(listOf("k"), Codecs.vec(Codecs.string).decodeAll(call(locked, StandardPorts.SecureStore.LIST, argsOf(""))))
        // The value is still there for a key source that works.
        assertArrayEquals(secret, secure().get("k"))
    }

    @Test
    fun the_failure_table_of_SecureSeal() {
        assertEquals(StorageError.Locked, SecureSeal.failure(UserNotAuthenticatedException(), "x"))
        assertTrue(SecureSeal.failure(KeyPermanentlyInvalidatedException(), "x") is StorageError.Corrupt)
        assertTrue(SecureSeal.failure(AEADBadTagException("tag mismatch"), "x") is StorageError.Corrupt)
        assertTrue(SecureSeal.failure(NoSuchProviderException("AndroidKeyStore"), "x") is StorageError.Unavailable)
        assertEquals(StorageError.Full, SecureSeal.failure(IOException("No space left on device"), "x"))
        assertEquals(StorageError.Io("Input/output error"), SecureSeal.failure(IOException("Input/output error"), "x"))
        assertTrue(SecureSeal.failure(ProviderException("boom"), "x") is StorageError.Io)
        assertTrue(SecureSeal.failure(IllegalStateException("boom"), "x") is StorageError.Io)
        assertEquals(StorageError.Locked, SecureSeal.failure(StorageError.Locked, "x"))
    }

    // ---- Fs ----------------------------------------------------------------------------------------------------------

    @Test
    fun a_full_disk_or_quota_is_FsError_Full_and_other_failures_stay_Io() = runBlocking {
        val impl = fs().portImpl()
        for (error in listOf(noSpace(), FaultyFileSystem.quotaExceeded())) {
            disk.fail(FaultyFileSystem.Op.WRITE) { error }
            assertArrayEquals("write on $error", byteArrayOf(3, 0), typedReply(impl, StandardPorts.Fs.WRITE, argsOf("notes/a.txt", secret)))
            disk.heal()
        }
        call(impl, StandardPorts.Fs.WRITE, argsOf("notes/a.txt", secret))
        disk.fail(FaultyFileSystem.Op.READ) { FaultyFileSystem.ioError() }
        assertArrayEquals(FsError.encodeToByteArray(FsError.Io("Input/output error")), typedReply(impl, StandardPorts.Fs.READ, argsOf("notes/a.txt")))
        disk.heal()
        assertThrows(FsError.Full::class.java) {
            disk.fail(FaultyFileSystem.Op.WRITE) { noSpace() }
            runBlocking { fs().write("notes/b.txt", secret) }
        }
        disk.heal()
        assertEquals(listOf("a.txt"), fs().list("notes"))
    }

    // ---- not typed ---------------------------------------------------------------------------------------------------

    @Test
    fun arguments_that_do_not_decode_are_not_a_typed_failure_so_the_runtime_answers_unavailable_and_logs() {
        for ((port, impl) in listOf(StoragePort.KV to kv().portImpl(), StoragePort.SECURE_STORE to secure().portImpl())) {
            for ((method, _) in methods(port)) {
                assertThrows("${port.traitName} 0x${method.toString(16)}", WireException::class.java) { call(impl, method, byteArrayOf(1)) }
            }
        }
    }
}
