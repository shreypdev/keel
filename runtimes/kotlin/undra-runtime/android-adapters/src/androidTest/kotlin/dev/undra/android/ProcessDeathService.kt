package dev.undra.android

import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.IBinder
import android.os.Process
import java.io.File
import kotlinx.coroutines.runBlocking

/**
 * Runs in its own process (`:writer`) so that a test can make a process die: it writes through the real adapters and
 * then either kills itself ([MODE_WRITE]) or keeps rewriting one Kv entry until the test kills it ([MODE_LOOP]).
 * It reports through marker files in the cache directory, which both processes share.
 */
class ProcessDeathService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val mode = intent?.getStringExtra(EXTRA_MODE)
        Thread({
            File(cacheDir, PID_FILE).writeText(Process.myPid().toString())
            when (mode) {
                MODE_WRITE -> {
                    runBlocking {
                        AndroidSecureStoreAdapter(applicationContext, TEST_NAMESPACE).set(SECRET_KEY, SECRET_VALUE)
                        AndroidKvAdapter(applicationContext, TEST_NAMESPACE).set(KV_KEY, KV_VALUE)
                        AndroidFsAdapter(applicationContext, TEST_NAMESPACE).write(FS_PATH, FS_VALUE)
                    }
                    File(cacheDir, DONE_FILE).writeText("written")
                    Process.killProcess(Process.myPid())
                }
                MODE_LOOP -> {
                    val kv = AndroidKvAdapter(applicationContext, TEST_NAMESPACE)
                    var n = 0
                    while (true) {
                        runBlocking { kv.set(LOOP_KEY, loopValue(n)) }
                        n++
                        if (n % 5 == 0) File(cacheDir, PROGRESS_FILE).writeText(n.toString())
                    }
                }
            }
        }, "process-death-writer").start()
        return START_NOT_STICKY
    }

    /** What the tests and the service agree on. */
    companion object {
        const val EXTRA_MODE = "mode"
        const val MODE_WRITE = "write"
        const val MODE_LOOP = "loop"

        const val PID_FILE = "writer.pid"
        const val DONE_FILE = "writer.done"
        const val PROGRESS_FILE = "writer.progress"

        const val SECRET_KEY = "survivor.token"
        val SECRET_VALUE: ByteArray = "survives-a-process-death-0xC0FFEE".toByteArray()
        const val KV_KEY = "survivor.kv"
        val KV_VALUE: ByteArray = "kv-survives-too".toByteArray()
        const val FS_PATH = "survivor/file.txt"
        val FS_VALUE: ByteArray = "fs-survives-too".toByteArray()

        const val LOOP_KEY = "torn.candidate"
        const val LOOP_SIZE = 300_000

        /** The value of the n-th write of [MODE_LOOP]: [LOOP_SIZE] copies of one byte, so a torn write is visible as a mixture. */
        fun loopValue(n: Int): ByteArray = ByteArray(LOOP_SIZE) { (n % 251 + 1).toByte() }

        /** Starts the service in [mode] and returns the context to look for its marker files in. */
        fun start(context: Context, mode: String) {
            File(context.cacheDir, DONE_FILE).delete()
            File(context.cacheDir, PID_FILE).delete()
            File(context.cacheDir, PROGRESS_FILE).delete()
            context.startService(Intent(context, ProcessDeathService::class.java).putExtra(EXTRA_MODE, mode))
        }
    }
}
