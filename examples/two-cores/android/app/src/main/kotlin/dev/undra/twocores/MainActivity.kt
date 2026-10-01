package dev.undra.twocores

import android.app.Activity
import android.graphics.Color
import android.os.Bundle
import android.util.Log
import android.widget.ScrollView
import android.widget.TextView
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
import dev.undra.twocores.b.Counter as CounterB
import dev.undra.twocores.b.UndraIds as IdsB
import dev.undra.twocores.b.add as addB

/**
 * The two-core test app on Android (ADR-044): two copies of the playground core, `libplayground_a.so` and
 * `libplayground_b.so`, in one APK and one process, each registering its natives on its own `UndraCoreNative`
 * and loaded through its own generated entry. It gives each a call and an observed change, compares their
 * statistics, closes one and checks the other keeps working. Every check is a `two-cores android:` line in
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

    private companion object {
        const val TAG = "TwoCores"
    }
}
