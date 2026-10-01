package dev.undra.android

import android.system.ErrnoException
import android.system.OsConstants
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.StorageError
import dev.undra.testsupport.FaultyFileSystem
import java.io.File
import java.io.IOException
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * A full disk as Android itself reports it: the `IOException` that `ErrnoException.rethrowAsIOException` makes from a
 * real `ENOSPC` or `EDQUOT` errno (what `libcore`'s file writes throw), injected under the real adapters on the device.
 * `StorageFailureTests` (shared with the JVM) covers the rest of ADR-049's table here too.
 */
class StorageFailureOnDeviceTest {
    private lateinit var dir: File
    private lateinit var disk: FaultyFileSystem

    @Before
    fun setUp() {
        dir = scratchDir("errno")
        disk = FaultyFileSystem(dir.toPath())
    }

    @After
    fun tearDown() {
        disk.heal()
        dir.deleteRecursively()
    }

    /** The exception Android throws for a failed `write(2)` with [errno]. */
    private fun androidWriteFailure(errno: Int): IOException =
        try {
            ErrnoException("write", errno).rethrowAsIOException()
        } catch (e: IOException) {
            e
        }

    @Test
    fun the_platforms_own_enospc_and_edquot_are_Full_for_Kv_SecureStore_and_Fs() = runBlocking {
        for (errno in listOf(OsConstants.ENOSPC, OsConstants.EDQUOT)) {
            val failure = androidWriteFailure(errno)
            assertTrue(failure.cause is ErrnoException)
            disk.fail(FaultyFileSystem.Op.WRITE) { androidWriteFailure(errno) }
            val kv = try {
                call(AndroidKvAdapter(disk.root.resolve("kv")).portImpl(), StandardPorts.Kv.SET, argsOf("k", byteArrayOf(1)))
                null
            } catch (e: UndraPortException) {
                e.body
            }
            assertArrayEquals("Kv on ${failure.message}", byteArrayOf(1, 0), kv)
            val secure = try {
                call(AndroidSecureStoreAdapter(disk.root.resolve("secure")) { javax.crypto.KeyGenerator.getInstance("AES").apply { init(256) }.generateKey() }.portImpl(), StandardPorts.SecureStore.SET, argsOf("k", byteArrayOf(1)))
                null
            } catch (e: UndraPortException) {
                e.body
            }
            assertArrayEquals("SecureStore on ${failure.message}", byteArrayOf(1, 0), secure)
            val fs = try {
                call(AndroidFsAdapter(disk.root.resolve("fs")).portImpl(), StandardPorts.Fs.WRITE, argsOf("a.txt", byteArrayOf(1)))
                null
            } catch (e: UndraPortException) {
                e.body
            }
            assertArrayEquals("Fs on ${failure.message}", byteArrayOf(3, 0), fs)
            disk.heal()
        }
        assertEquals(StorageError.Full, StorageError.of(androidWriteFailure(OsConstants.ENOSPC)))
        // Nothing was created by the failed writes.
        assertEquals(0, dir.listFiles()!!.size)
    }
}
