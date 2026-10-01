// The two-core test app on the JVM (ADR-044): libplayground_a and libplayground_b in one JVM, each registering
// its natives on its own generated `UndraCoreNative`, loaded side by side through their generated entries. Every
// check prints one `two-cores jvm:` line; any failure exits 1.
//
//   examples/two-cores/jvm/run.sh
package dev.undra.twocores.jvm

import dev.undra.runtime.LoadOptions
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraDispatchers
import dev.undra.twocores.a.UndraPlaygroundA
import dev.undra.twocores.b.UndraPlaygroundB
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.system.exitProcess
import dev.undra.twocores.a.Counter as CounterA
import dev.undra.twocores.a.add as addA
import dev.undra.twocores.b.Counter as CounterB
import dev.undra.twocores.b.add as addB

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
