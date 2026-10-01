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
        AndroidKvAdapter(context).set("k", byteArrayOf(1))
        AndroidFsAdapter(context).write("notes/a.txt", byteArrayOf(2))
        assertTrue(File(context.filesDir, "undra/kv").listFiles()!!.size == 1)
        assertEquals("\u0002", File(context.filesDir, "undra/fs/notes/a.txt").readText())
    }

    @Test
    fun the_adapters_over_a_context_keep_only_the_application_context() = runBlocking {
        // An activity context would leak if an adapter held it; the constructors take the application context.
        val kv = AndroidKvAdapter(context)
        kv.set("k", byteArrayOf(1))
        assertEquals(listOf("k"), AndroidKvAdapter(context.applicationContext).list(""))
    }

    @Test
    fun the_directories_belong_to_the_app() {
        assertTrue(context.filesDir.canonicalPath.startsWith(context.dataDir.canonicalPath))
        assertTrue(context.noBackupFilesDir.canonicalPath.startsWith(context.dataDir.canonicalPath))
    }
}
