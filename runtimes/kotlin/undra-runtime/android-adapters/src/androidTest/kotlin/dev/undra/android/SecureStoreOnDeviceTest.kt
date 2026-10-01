package dev.undra.android

import android.app.ActivityManager
import android.content.Context
import android.security.keystore.KeyInfo
import android.os.Process
import androidx.test.core.app.ApplicationProvider
import java.io.File
import java.security.KeyStore
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** The SecureStore and Kv adapters on the device: the real Keystore, where the bytes land, and what survives the process. */
class SecureStoreOnDeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val secret = "plain-text-secret-4f9a1c7e-do-not-leak".toByteArray()
    private val secureDir = File(context.noBackupFilesDir, "undra/secure")
    private val kvDir = File(context.filesDir, "undra/kv")

    @Before
    fun clean() {
        secureDir.deleteRecursively()
        kvDir.deleteRecursively()
        File(context.filesDir, "undra/fs").deleteRecursively()
        context.deleteSharedPreferences("plain")
    }

    @After
    fun tearDown() {
        clean()
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        for (alias in listOf("dev.undra.test.alias-a", "dev.undra.test.alias-b")) if (keyStore.containsAlias(alias)) keyStore.deleteEntry(alias)
    }

    /** Whether any file under the app's data directory, whatever its name or place, contains [needle]. */
    private fun filesContaining(needle: ByteArray): List<String> {
        val text = String(needle, Charsets.ISO_8859_1)
        return context.dataDir.walk().filter { it.isFile }
            .filter { file -> String(file.readBytes(), Charsets.ISO_8859_1).contains(text) }
            .map { it.relativeTo(context.dataDir).path }.toList()
    }

    @Test
    fun a_secret_round_trips_through_the_real_keystore() = runBlocking {
        val secure = AndroidSecureStoreAdapter(context)
        secure.set("session.token", secret)
        assertArrayEquals(secret, secure.get("session.token"))
        assertNull(secure.get("absent"))
        assertEquals(listOf("session.token"), secure.list(""))
        secure.delete("session.token")
        assertNull(secure.get("session.token"))
    }

    @Test
    fun the_aes_key_lives_in_the_android_keystore_and_cannot_be_read_out() = runBlocking {
        AndroidSecureStoreAdapter(context).set("k", secret)
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        assertTrue(keyStore.containsAlias(AndroidSecureStoreAdapter.DEFAULT_KEY_ALIAS))
        val key = keyStore.getKey(AndroidSecureStoreAdapter.DEFAULT_KEY_ALIAS, null) as SecretKey
        assertNull("the key material is not extractable", key.encoded)
        assertEquals("AES", key.algorithm)
        val info = SecretKeyFactory.getInstance(key.algorithm, "AndroidKeyStore").getKeySpec(key, KeyInfo::class.java) as KeyInfo
        assertEquals(256, info.keySize)
        @Suppress("DEPRECATION") // KeyInfo.getSecurityLevel needs API 31; minSdk is 26
        val hardware = info.isInsideSecureHardware
        println("SecureStore key: inside secure hardware = $hardware")
    }

    @Test
    fun the_secret_is_in_no_file_of_the_app_in_clear_and_not_in_shared_preferences() = runBlocking {
        AndroidSecureStoreAdapter(context).set("session.token", secret)
        assertEquals("the sealed value must not contain the secret", emptyList<String>(), filesContaining(secret))
        // The scanner would see it if it were there: a plain SharedPreferences and the plain Kv both hold it in clear.
        context.getSharedPreferences("plain", Context.MODE_PRIVATE).edit().putString("token", String(secret)).commit()
        AndroidKvAdapter(context).set("plain.kv", secret)
        val found = filesContaining(secret)
        assertTrue(found.toString(), found.any { it.startsWith("shared_prefs/") })
        assertTrue(found.toString(), found.any { it.startsWith("files/undra/kv/") })
        assertFalse(found.toString(), found.any { it.contains("undra/secure") })
    }

    @Test
    fun the_sealed_file_is_in_the_no_backup_directory_and_not_in_the_backed_up_one() = runBlocking {
        AndroidSecureStoreAdapter(context).set("session.token", secret)
        assertTrue(secureDir.listFiles { f -> f.isFile && !f.name.startsWith(".") }!!.isNotEmpty())
        assertTrue(secureDir.canonicalPath.startsWith(context.noBackupFilesDir.canonicalPath))
        assertFalse(File(context.filesDir, "undra/secure").exists())
    }

    @Test
    fun a_new_adapter_instance_reads_what_another_wrote() = runBlocking {
        AndroidSecureStoreAdapter(context).set("k", secret)
        assertArrayEquals(secret, AndroidSecureStoreAdapter(context).get("k"))
    }

    @Test
    fun a_value_sealed_under_the_key_cannot_be_opened_with_another_alias() = runBlocking {
        AndroidSecureStoreAdapter(secureDir, "dev.undra.test.alias-a").set("k", secret)
        val other = AndroidSecureStoreAdapter(secureDir, "dev.undra.test.alias-b")
        val failed = try {
            other.get("k")
            false
        } catch (e: SecureStoreException) {
            true
        }
        assertTrue("another key must not open it (and must not read it as missing)", failed)
    }

    @Test
    fun a_secret_survives_the_death_of_the_process_that_wrote_it() {
        ProcessDeathService.start(context, ProcessDeathService.MODE_WRITE)
        assertTrue("the writer process wrote", waitFor(20_000) { File(context.cacheDir, ProcessDeathService.DONE_FILE).exists() })
        val pid = File(context.cacheDir, ProcessDeathService.PID_FILE).readText().trim().toInt()
        assertTrue("the writer process is another process", pid != Process.myPid())
        assertTrue("the writer process died", waitFor(20_000) { !isAlive(pid) })

        runBlocking {
            assertArrayEquals(ProcessDeathService.SECRET_VALUE, AndroidSecureStoreAdapter(context).get(ProcessDeathService.SECRET_KEY))
            assertArrayEquals(ProcessDeathService.KV_VALUE, AndroidKvAdapter(context).get(ProcessDeathService.KV_KEY))
            assertArrayEquals(ProcessDeathService.FS_VALUE, AndroidFsAdapter(context).read(ProcessDeathService.FS_PATH))
        }
        assertEquals(emptyList<String>(), filesContaining(ProcessDeathService.SECRET_VALUE))
    }

    @Test
    fun a_kv_write_interrupted_by_killing_the_process_leaves_a_whole_value() {
        ProcessDeathService.start(context, ProcessDeathService.MODE_LOOP)
        assertTrue("the writer made progress", waitFor(20_000) { progress() >= 30 })
        val pid = File(context.cacheDir, ProcessDeathService.PID_FILE).readText().trim().toInt()
        Process.killProcess(pid) // SIGKILL, in the middle of a write loop
        assertTrue("the writer process died", waitFor(20_000) { !isAlive(pid) })

        runBlocking {
            val kv = AndroidKvAdapter(context)
            val value = kv.get(ProcessDeathService.LOOP_KEY)
            assertNotNull(value)
            assertEquals(ProcessDeathService.LOOP_SIZE, value!!.size)
            assertTrue("the value is one write's, not a mixture of two", value.all { it == value[0] })
            assertEquals(listOf(ProcessDeathService.LOOP_KEY), kv.list(""))
        }
        // a kill can leave a temporary file of the write it interrupted; it is not an entry and `list` ignores it
        val leftovers = kvDir.listFiles()!!.filter { it.name.endsWith(".tmp") }
        println("temporary files left by the kill: ${leftovers.size}")
    }

    private fun progress(): Int = File(context.cacheDir, ProcessDeathService.PROGRESS_FILE).takeIf { it.exists() }?.readText()?.trim()?.toIntOrNull() ?: 0

    private fun isAlive(pid: Int): Boolean {
        val manager = context.getSystemService(ActivityManager::class.java)
        return manager.runningAppProcesses.orEmpty().any { it.pid == pid }
    }

    private fun waitFor(timeoutMs: Long, condition: () -> Boolean): Boolean {
        val deadline = System.nanoTime() + timeoutMs * 1_000_000
        while (System.nanoTime() < deadline) {
            if (condition()) return true
            Thread.sleep(50)
        }
        return condition()
    }
}
