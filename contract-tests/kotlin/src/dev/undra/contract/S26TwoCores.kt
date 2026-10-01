package dev.undra.contract

import dev.undra.runtime.LoadOptions
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraTransportException
import dev.undra.twocores.a.UndraPlaygroundA
import dev.undra.twocores.b.UndraPlaygroundB
import dev.undra.twocores.a.Counter as CounterA
import dev.undra.twocores.a.UndraCoreNative as NativeA
import dev.undra.twocores.a.UndraIds as IdsA
import dev.undra.twocores.a.add as addA
import dev.undra.twocores.b.Counter as CounterB
import dev.undra.twocores.b.UndraCoreNative as NativeB
import dev.undra.twocores.b.UndraIds as IdsB
import dev.undra.twocores.b.add as addB

/**
 * S26 (ADR-044): the playground core under two more namespaces, `playground_a` and `playground_b`
 * (`libplayground_a`, `libplayground_b`, each registering its natives on its own `UndraCoreNative`), in this JVM next
 * to the playground core the other scenarios share. Each is its own image with its own runtime and threads; their
 * generated packages default to their own core.
 */
fun s26TwoCores(w: World) {
    val options = { LoadOptions(adapters = emptyMap(), defaultAdapters = true) }

    // 1. Both load through their generated entries; each entry's core is its own.
    check(NativeA.isAvailable) { "libplayground_a could not be loaded: ${NativeA.unavailableReason}" }
    check(NativeB.isAvailable) { "libplayground_b could not be loaded: ${NativeB.unavailableReason}" }
    expectEq("C ABI version of playground_a", 2, NativeA.abiVersion())
    expectEq("C ABI version of playground_b", 2, NativeB.abiVersion())
    val a = UndraPlaygroundA.load(options())
    val b = UndraPlaygroundB.load(options())
    var reloaded: UndraCore? = null
    try {
        check(a !== b && a !== w.core && b !== w.core) { "three cores" }
        check(UndraPlaygroundA.core === a && UndraPlaygroundB.core === b) { "each entry's core is its own" }
        expectEq("UndraIds.NAMESPACE of package A", "playground_a", IdsA.NAMESPACE)
        expectEq("UndraIds.NAMESPACE of package B", "playground_b", IdsB.NAMESPACE)
        expectEq("A's schema hash", IdsA.SCHEMA_HASH, a.readStats().schemaHash)
        expectEq("B's schema hash", IdsB.SCHEMA_HASH, b.readStats().schemaHash)

        // 2. A call on each, through each package's own default core.
        val callsA = a.readStats().calls
        val callsB = b.readStats().calls
        expectEq("add(2, 3) through package A", 5, addA(2, 3))
        expectEq("calls A counted", 1L, a.readStats().calls - callsA)
        expectEq("calls B counted for a call of package A", 0L, b.readStats().calls - callsB)
        expectEq("add(2, 3) through package B", 5, addB(2, 3))
        expectEq("calls A counted for a call of package B", 1L, a.readStats().calls - callsA)
        expectEq("calls B counted", 1L, b.readStats().calls - callsB)

        // 3. An observed change on each, independent (on the main thread, where a synchronous call drains the mirror).
        val handlesA = a.readStats().liveHandles
        val handlesB = b.readStats().liveHandles
        val counterA = onMain { CounterA() }
        val counterB = onMain { CounterB() }
        val changeSetsA = a.mirror.stats().changeSetsReceived
        val changeSetsB = b.mirror.stats().changeSetsReceived
        expectEq("A's count right after add(2)", 2, onMain { counterA.add(2); counterA.count.value })
        expectEq("change-sets A's mirror received", 1L, a.mirror.stats().changeSetsReceived - changeSetsA)
        expectEq("change-sets B's mirror received for A's write", 0L, b.mirror.stats().changeSetsReceived - changeSetsB)
        expectEq("B's count right after add(5)", 5, onMain { counterB.add(5); counterB.count.value })
        expectEq("A's count after B's write", 2, counterA.count.value)

        // 4. Independent statistics; each object keeps its own core.
        expectEq("handles A made", 1L, a.readStats().liveHandles - handlesA)
        expectEq("handles B made", 1L, b.readStats().liveHandles - handlesB)
        counterA.close()
        expectEq("A's handles after its counter closed", 0L, a.readStats().liveHandles - handlesA)
        expectEq("B's handles after A's counter closed", 1L, b.readStats().liveHandles - handlesB)

        // 5. One shut down while the other keeps working.
        a.close()
        check(UndraPlaygroundA.core !== a) { "A's entry still holds A after it was closed" }
        awaitUntil("A's image to run no core and no thread") {
            val doc = Json.parseObject(NativeA.statsJson())
            doc["initialized"] == false && doc["runtime_threads"] == 0L
        }
        check(Json.parseObject(NativeB.statsJson())["initialized"] != false) { "B's image stopped with A's" }
        expectEq("B's count after A's shutdown", 6, onMain { counterB.add(1); counterB.count.value })
        expectEq("add(2, 3) through B after A's shutdown", 5, addB(2, 3))
        val viaClosed = expectFails<UndraCallError.Unavailable>("add(2, 3) on A after its shutdown") { addA(2, 3, a) }
        expectEq("the transport reason on the closed core", UndraTransportException.Reason.CLOSED, viaClosed.transport.reason)
        expectFails<UndraCallError.Unavailable>("add(2, 3) through package A's entry after A's shutdown") { addA(2, 3) }

        // 6. A namespace loads once; a closed one loads again.
        expectFails<UndraException>("a second load of B while it is loaded") { UndraPlaygroundB.load(options()) }
        check(UndraPlaygroundB.core === b) { "B's entry no longer holds B" }
        reloaded = UndraPlaygroundA.load(options())
        check(UndraPlaygroundA.core === reloaded) { "A's entry does not hold the reloaded A" }
        expectEq("add(2, 3) through the reloaded A", 5, addA(2, 3))
        counterB.close()
    } finally {
        reloaded?.close()
        a.close()
        b.close()
    }
}
