package dev.undra.twocores

import android.app.Activity
import android.graphics.Color
import android.os.Bundle
import android.util.Log
import android.widget.ScrollView
import android.widget.TextView
import dev.undra.android.AndroidPlatformDefaults
import dev.undra.android.ChoreographerFramePacer
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.MirrorOptions
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraDispatchers
import dev.undra.twocores.a.UndraPlaygroundA
import dev.undra.twocores.b.UndraPlaygroundB
import dev.undra.twocores.a.Counter as CounterA
import dev.undra.twocores.a.UndraIds as IdsA
import dev.undra.twocores.a.add as addA
import dev.undra.twocores.a.kvGet as kvGetA
import dev.undra.twocores.a.kvKeys as kvKeysA
import dev.undra.twocores.a.kvPut as kvPutA
import dev.undra.twocores.a.kvRemove as kvRemoveA
import dev.undra.twocores.b.Counter as CounterB
import dev.undra.twocores.b.UndraIds as IdsB
import dev.undra.twocores.b.add as addB
import dev.undra.twocores.b.kvGet as kvGetB
import dev.undra.twocores.b.kvKeys as kvKeysB
import dev.undra.twocores.b.kvPut as kvPutB
import dev.undra.twocores.b.kvRemove as kvRemoveB
import java.io.File
import kotlinx.coroutines.runBlocking

/**
 * The two-core test app on Android (ADR-044): two copies of the playground core, `libplayground_a.so` and
 * `libplayground_b.so`, in one APK and one process, each registering its natives on its own `UndraCoreNative`
 * and loaded through its own generated entry. It gives each a call and an observed change, compares their
 * statistics, closes one and checks the other keeps working. Both get the platform's default adapters
 * (`AndroidPlatformDefaults.install`), whose `Kv` is per core namespace (ADR-044 amendment A): each writes the same key
 * and reads its own value back, and the files are in two directories. Every check is a `two-cores android:` line in
 * logcat (tag `TwoCores`) and on screen.
 */
class MainActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val lines = run()
        val passed = lines.none { "FAIL" in it }
        val text = TextView(this).apply {
            setPadding(48, 96, 48, 48)
            textSize = 16f
            setTextColor(if (passed) Color.rgb(0, 128, 0) else Color.RED)
            this.text = (listOf(if (passed) "Two cores: passed" else "Two cores: FAILED", "") + lines).joinToString("\n\n")
        }
        setContentView(ScrollView(this).apply { addView(text) })
    }

    private fun run(): List<String> {
        val lines = mutableListOf<String>()
        fun check(what: String, ok: Boolean) {
            val line = "two-cores android: ${if (ok) "ok  " else "FAIL"} $what"
            Log.i(TAG, line)
            lines += line
        }
        check("the activity runs on the runtime's main thread", UndraDispatchers.isMainThread())
        try {
            // Each core has its own mirror and its own pacer on the display's frames.
            val a = UndraPlaygroundA.load(LoadOptions(mirror = MirrorOptions(framePacer = ChoreographerFramePacer())))
            val b = UndraPlaygroundB.load(LoadOptions(mirror = MirrorOptions(framePacer = ChoreographerFramePacer())))
            check(
                "both loaded: ${UndraPlaygroundA.NAMESPACE} and ${UndraPlaygroundB.NAMESPACE}, two cores",
                a !== b && UndraPlaygroundA.core === a && UndraPlaygroundB.core === b,
            )
            val hashA = "0x" + IdsA.SCHEMA_HASH.toString(16).padStart(16, '0')
            val hashB = "0x" + IdsB.SCHEMA_HASH.toString(16).padStart(16, '0')
            check(
                "schema hash of each: $hashA, $hashB",
                "\"schema_hash\":\"$hashA\"" in a.stats().raw && "\"schema_hash\":\"$hashB\"" in b.stats().raw,
            )
            val sumA = addA(2, 3)
            val sumB = addB(2, 3)
            check("a call on each: add(2, 3) is $sumA through A and $sumB through B", sumA == 5 && sumB == 5)
            val handlesA = a.stats().liveHandles
            val handlesB = b.stats().liveHandles
            val counterA = CounterA()
            val counterB = CounterB()
            counterA.add(2)
            counterB.add(5)
            check(
                "an observed change on each: A's count is ${counterA.count.value}, B's is ${counterB.count.value}",
                counterA.count.value == 2 && counterB.count.value == 5,
            )
            val newA = a.stats().liveHandles - handlesA
            val newB = b.stats().liveHandles - handlesB
            check("independent statistics: A has $newA new handle, B has $newB", newA == 1 && newB == 1)
            val platformA = AndroidPlatformDefaults.install(a, this)
            val platformB = AndroidPlatformDefaults.install(b, this)
            checkStorage(::check)
            platformA.close()
            platformB.close()
            a.close()
            counterB.add(1)
            val unavailable = try {
                addA(2, 3)
                false
            } catch (e: UndraCallError.Unavailable) {
                true
            }
            val sumAfter = addB(2, 3)
            check(
                "one shut down while the other keeps working: B's count is ${counterB.count.value}, " +
                    "a call on A is ${if (unavailable) "unavailable" else "answered"}",
                counterB.count.value == 6 && unavailable && sumAfter == 5,
            )
            counterB.close()
            b.close()
        } catch (e: Throwable) {
            check("the run failed: $e", false)
        }
        Log.i(TAG, "two-cores android: ${if (lines.none { "FAIL" in it }) "passed" else "FAILED"}")
        return lines
    }

    /**
     * The default stores of two cores are two stores (ADR-044 amendment A): the same `Kv` key written through each core
     * reads back that core's own value, a key one core wrote is not the other's, and the files are in
     * `<filesDir>/undra/<namespace>/kv`. Runs on a thread of its own, which waits for the cores' port replies.
     */
    private fun checkStorage(check: (String, Boolean) -> Unit) {
        var failure: Throwable? = null
        val worker = Thread {
            try {
                runBlocking {
                    val nonce = System.nanoTime().toString(16)
                    val key = "two-cores.key"
                    val onlyInA = "two-cores.only-a"
                    kvPutA(key, "value-of-a-$nonce".toByteArray())
                    kvPutB(key, "value-of-b-$nonce".toByteArray())
                    kvPutA(onlyInA, byteArrayOf(1))
                    val readA = kvGetA(key)?.decodeToString()
                    val readB = kvGetB(key)?.decodeToString()
                    check(
                        "Kv: both wrote $key and read their own value back: A has $readA, B has $readB",
                        readA == "value-of-a-$nonce" && readB == "value-of-b-$nonce",
                    )
                    val keysA = kvKeysA("two-cores.")
                    val keysB = kvKeysB("two-cores.")
                    check("Kv: a key only A wrote is not B's: A lists $keysA, B lists $keysB", onlyInA in keysA && onlyInA !in keysB && key in keysB)
                    val root = File(filesDir, "undra")
                    val a = File(root, "${UndraPlaygroundA.NAMESPACE}/kv")
                    val b = File(root, "${UndraPlaygroundB.NAMESPACE}/kv")
                    check("Kv files: ${a.path} and ${b.path}, and none in ${root.path}/kv", a.isDirectory && b.isDirectory && !File(root, "kv").exists())
                    kvRemoveA(key)
                    kvRemoveA(onlyInA)
                    kvRemoveB(key)
                }
            } catch (e: Throwable) {
                failure = e
            }
        }
        worker.start()
        worker.join()
        failure?.let { check("Kv: the check failed: $it", false) }
    }

    private companion object {
        const val TAG = "TwoCores"
    }
}
