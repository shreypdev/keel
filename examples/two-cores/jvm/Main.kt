// The two-core test app on the JVM (ADR-044): libplayground_a and libplayground_b in one JVM, each registering
// its natives on its own generated `UndraCoreNative`, loaded side by side through their generated entries. Every
// check prints one `two-cores jvm:` line; any failure exits 1.
//
//   examples/two-cores/jvm/run.sh
package dev.undra.twocores.jvm

import dev.undra.runtime.LoadOptions
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraDispatchers
import dev.undra.runtime.adapters.JvmAdapters
import kotlinx.coroutines.runBlocking
import dev.undra.twocores.a.UndraPlaygroundA
import dev.undra.twocores.b.UndraPlaygroundB
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.system.exitProcess
import dev.undra.twocores.a.Counter as CounterA
import dev.undra.twocores.a.add as addA
import dev.undra.twocores.a.kvGet as kvGetA
import dev.undra.twocores.a.kvKeys as kvKeysA
import dev.undra.twocores.a.kvPut as kvPutA
import dev.undra.twocores.a.kvRemove as kvRemoveA
import dev.undra.twocores.b.Counter as CounterB
import dev.undra.twocores.b.add as addB
import dev.undra.twocores.b.kvGet as kvGetB
import dev.undra.twocores.b.kvKeys as kvKeysB
import dev.undra.twocores.b.kvPut as kvPutB
import dev.undra.twocores.b.kvRemove as kvRemoveB

private var failed = false

private fun check(what: String, ok: Boolean) {
    println("two-cores jvm: ${if (ok) "ok  " else "FAIL"} $what")
    if (!ok) failed = true
}

/** Runs [block] on the runtime's main thread, where a synchronous call applies its change-sets before it returns. */
private fun <T> onMain(block: () -> T): T {
    val result = CompletableFuture<T>()
    UndraDispatchers.main.dispatch(kotlin.coroutines.EmptyCoroutineContext, Runnable {
        try {
            result.complete(block())
        } catch (e: Throwable) {
            result.completeExceptionally(e)
        }
    })
    return result.get(10, TimeUnit.SECONDS)
}

/**
 * The default stores of two cores are two stores (ADR-044 amendment A): the same `Kv` key written through each core reads
 * back that core's own value, a key one core wrote is not the other's, and the files are in `<data dir>/<namespace>/kv`.
 */
private fun checkStorage() = runBlocking {
    val nonce = System.nanoTime().toString(16)
    val key = "two-cores.key"
    val onlyInA = "two-cores.only-a"
    kvPutA(key, "value-of-a-$nonce".toByteArray())
    kvPutB(key, "value-of-b-$nonce".toByteArray())
    kvPutA(onlyInA, byteArrayOf(1))
    val readA = kvGetA(key)?.decodeToString()
    val readB = kvGetB(key)?.decodeToString()
    check("Kv: both wrote $key and read their own value back: A has $readA, B has $readB", readA == "value-of-a-$nonce" && readB == "value-of-b-$nonce")
    val keysA = kvKeysA("two-cores.")
    val keysB = kvKeysB("two-cores.")
    check("Kv: a key only A wrote is not B's: A lists $keysA, B lists $keysB", onlyInA in keysA && onlyInA !in keysB && key in keysB)
    val dir = JvmAdapters.defaultDataDir()
    val a = dir.resolve(UndraPlaygroundA.NAMESPACE).resolve("kv")
    val b = dir.resolve(UndraPlaygroundB.NAMESPACE).resolve("kv")
    check(
        "Kv files: $a and $b, and none in the shared $dir/kv",
        a.toFile().isDirectory && b.toFile().isDirectory && !dir.resolve("kv").toFile().exists(),
    )
    kvRemoveA(key)
    kvRemoveA(onlyInA)
    kvRemoveB(key)
}

fun main() {
    val a = UndraPlaygroundA.load(LoadOptions())
    val b = UndraPlaygroundB.load(LoadOptions())
    check("both loaded: ${UndraPlaygroundA.NAMESPACE} and ${UndraPlaygroundB.NAMESPACE}, two cores", a !== b && UndraPlaygroundA.core === a && UndraPlaygroundB.core === b)
    check("a call on each: add(2, 3) is 5 through A and through B", addA(2, 3) == 5 && addB(2, 3) == 5)
    val handlesA = a.stats().liveHandles
    val handlesB = b.stats().liveHandles
    val counterA = onMain { CounterA() }
    val counterB = onMain { CounterB() }
    val countA = onMain { counterA.add(2); counterA.count.value }
    val countB = onMain { counterB.add(5); counterB.count.value }
    check("an observed change on each: A's count is $countA, B's is $countB", countA == 2 && countB == 5)
    val newA = a.stats().liveHandles - handlesA
    val newB = b.stats().liveHandles - handlesB
    check("independent statistics: A has $newA new handle, B has $newB", newA == 1 && newB == 1)
    checkStorage()
    a.close()
    val countAfter = onMain { counterB.add(1); counterB.count.value }
    val unavailable = try {
        addA(2, 3)
        false
    } catch (e: UndraCallError.Unavailable) {
        true
    }
    check(
        "one shut down while the other keeps working: B's count is $countAfter, a call on A is ${if (unavailable) "unavailable" else "answered"}",
        countAfter == 6 && unavailable && addB(2, 3) == 5,
    )
    b.close()
    println("two-cores jvm: ${if (failed) "FAILED" else "passed"}")
    exitProcess(if (failed) 1 else 0)
}
