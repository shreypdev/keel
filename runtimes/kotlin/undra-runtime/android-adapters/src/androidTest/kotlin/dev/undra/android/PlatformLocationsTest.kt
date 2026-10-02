package dev.undra.android

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import java.io.File
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** Where the adapters keep their files on a device, as the README documents. */
class PlatformLocationsTest {
    private val context: Context = ApplicationProvider.getApplicationContext()

    @Before
    fun clean() {
        File(context.filesDir, "undra").deleteRecursively()
        File(context.noBackupFilesDir, "undra").deleteRecursively()
    }

    @After
    fun tearDown() = clean()

    @Test
    fun kv_lives_under_files_dir_and_fs_under_its_own_directory_there() = runBlocking {
        AndroidKvAdapter(context, TEST_NAMESPACE).set("k", byteArrayOf(1))
        AndroidFsAdapter(context, TEST_NAMESPACE).write("notes/a.txt", byteArrayOf(2))
        assertTrue(File(context.filesDir, "undra/$TEST_NAMESPACE/kv").listFiles()!!.size == 1)
        assertEquals("\u0002", File(context.filesDir, "undra/$TEST_NAMESPACE/fs/notes/a.txt").readText())
    }

    @Test
    fun the_adapters_over_a_context_keep_only_the_application_context() = runBlocking {
        // An activity context would leak if an adapter held it; the constructors take the application context.
        val kv = AndroidKvAdapter(context, TEST_NAMESPACE)
        kv.set("k", byteArrayOf(1))
        assertEquals(listOf("k"), AndroidKvAdapter(context.applicationContext, TEST_NAMESPACE).list(""))
    }

    @Test
    fun two_cores_with_the_defaults_never_see_each_others_data() = runBlocking {
        // `AndroidPlatformDefaults.install` keeps each core's stores under that core's namespace (ADR-044 amendment A).
        val a = RecordingCore("ns_a")
        val b = RecordingCore("ns_b")
        val platformA = AndroidPlatformDefaults.install(a, context)
        val platformB = AndroidPlatformDefaults.install(b, context)
        try {
            platformA.kv.set("k", byteArrayOf(1))
            platformB.kv.set("k", byteArrayOf(2))
            platformA.kv.set("only-a", byteArrayOf(3))
            assertEquals(1, platformA.kv.get("k")!![0].toInt())
            assertEquals(2, platformB.kv.get("k")!![0].toInt())
            assertEquals(listOf("k", "only-a"), platformA.kv.list(""))
            assertEquals(listOf("k"), platformB.kv.list(""))
            assertTrue(File(context.filesDir, "undra/ns_a/kv").isDirectory)
            assertTrue(File(context.filesDir, "undra/ns_b/kv").isDirectory)

            platformA.fs.write("notes/a.txt", byteArrayOf(4))
            assertTrue(File(context.filesDir, "undra/ns_a/fs/notes/a.txt").isFile)
            assertTrue(!File(context.filesDir, "undra/ns_b/fs/notes/a.txt").exists())

            // The real Keystore: one key per namespace, and a secret of one core is not a secret of the other.
            platformA.secureStore.set("token", byteArrayOf(5))
            platformB.secureStore.set("token", byteArrayOf(6))
            assertEquals(5, platformA.secureStore.get("token")!![0].toInt())
            assertEquals(6, platformB.secureStore.get("token")!![0].toInt())
            assertTrue(File(context.noBackupFilesDir, "undra/ns_a/secure").isDirectory)
            assertTrue(File(context.noBackupFilesDir, "undra/ns_b/secure").isDirectory)
            val keyStore = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            assertTrue(keyStore.containsAlias("ns_a.dev.undra.securestore"))
            assertTrue(keyStore.containsAlias("ns_b.dev.undra.securestore"))
        } finally {
            platformA.close()
            platformB.close()
            val keyStore = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            for (alias in listOf("ns_a.dev.undra.securestore", "ns_b.dev.undra.securestore")) if (keyStore.containsAlias(alias)) keyStore.deleteEntry(alias)
        }
    }

    @Test
    fun an_adapter_given_a_directory_keeps_it_whatever_the_core() = runBlocking {
        val dir = File(context.cacheDir, "undra-ns-own-${System.nanoTime()}")
        try {
            AndroidKvAdapter(dir).set("k", byteArrayOf(1))
            assertEquals(1, AndroidKvAdapter(dir).get("k")!![0].toInt())
            assertTrue(!File(context.filesDir, "undra").exists())
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun the_directories_belong_to_the_app() {
        assertTrue(context.filesDir.canonicalPath.startsWith(context.dataDir.canonicalPath))
        assertTrue(context.noBackupFilesDir.canonicalPath.startsWith(context.dataDir.canonicalPath))
    }
}
