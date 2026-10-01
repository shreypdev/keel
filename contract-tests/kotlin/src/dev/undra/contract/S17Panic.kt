package dev.undra.contract

import dev.undra.playground.core.Counter
import dev.undra.playground.core.Stress
import dev.undra.playground.core.StressMode
import dev.undra.playground.core.add
import dev.undra.playground.core.explode
import dev.undra.playground.core.explodeLater
import dev.undra.playground.core.failLater
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraNative
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.wire.Payloads.ReplyStatus
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async

/**
 * S17: a panic in the core unwinds to the boundary and arrives as a reply with status panic; nothing else stops.
 * The last two steps shut the core down (a call in flight fails as closed, the core's tasks stop calling ports) and
 * load a fresh one in the same process (ADR-034).
 */
fun s17Panic(w: World) {
    val counter = Counter.create()
    val panicsBefore = w.stats().panics

    // 1. A sync panic: status panic, the message and a backtrace, and the process is still here to see it.
    val sync = expectFails<UndraReplyException>("explode(\"kaboom\")") { explode("kaboom") }
    expectEq("the status of the panic", ReplyStatus.PANIC, sync.status)
    val info = sync.panicInfo ?: fail("the panic reply has no readable message")
    check("kaboom" in info.message) { "the panic message is \"${info.message}\"" }

    // 2. An async panic, inside a call that was already running.
    val later = expectFailsAsync<UndraReplyException>("explode_later(10, \"later\")") { explodeLater(10u, "later") }
    expectEq("the status of the async panic", ReplyStatus.PANIC, later.status)
    check("later" in (later.panicInfo?.message ?: "")) { "the async panic message is \"${later.panicInfo?.message}\"" }

    // 3. The core keeps working, a store made before still updates, and both panics were counted.
    expectEq("add(1, 2) after the panics", 3, add(1, 2))
    counter.increment()
    awaitEq("Counter.count after the panics", 1) { counter.count.value }
    expectEq("panics grown by the two panics", 2L, w.stats().panics - panicsBefore)

    // 4. Each panic reached the Log port at error level or above, under the target undra::panic.
    for (reason in listOf("kaboom", "later")) {
        awaitUntil("an undra::panic log record about \"$reason\"") {
            w.log.records.any { it.level >= 4 && it.target == "undra::panic" && reason in it.message }
        }
    }

    // 5. Re-entry is refused, not deadlocked or aborted. The core logs the panic of `explode("reenter")` through
    // the runner's Log adapter, a synchronous port called on the thread that holds the core lock; the adapter calls
    // the generated `add(1, 1)` once from there.
    var reentered: Throwable? = null
    var reenteredReturned = false
    w.log.onNextRecord({ it.level >= 4 && it.target == "undra::panic" && "reenter" in it.message }) {
        try {
            add(1, 1)
            reenteredReturned = true
        } catch (e: Throwable) {
            reentered = e
        }
    }
    val reenterPanic = expectFails<UndraReplyException>("explode(\"reenter\")") { explode("reenter") }
    expectEq("the status of explode(\"reenter\")", ReplyStatus.PANIC, reenterPanic.status)
    check(!reenteredReturned) { "add(1, 1) from inside the Log port returned" }
    // The Kotlin runtime refuses the call itself, before the core can (its `InprocTransport` knows it is inside a
    // callback), so the failure is an `UndraException`, not a bad-request reply carrying E_REENTRANT.
    val refused = reentered as? UndraException ?: fail("the Log adapter did not run during the panic, or add(1, 1) failed with $reentered")
    check("inside a core callback" in (refused.message ?: "")) { "the refusal does not say it was made inside a core callback: ${refused.message}" }
    expectEq("add(1, 2) after the re-entrant call", 3, add(1, 2))

    // 7, first half: a timer-paced core task is running before the shutdown. The generator of `Stress.start` sleeps
    // 10 ms between ticks and reads the Clock port at each, so the Clock adapter sees it working.
    val stress = Stress.create()
    stress.start(StressMode.FIREHOSE, 1_000u)
    awaitEq("Stress.running after start", true) { stress.running.value }
    val clockCallsAtStart = w.portCalls.count("Clock")
    awaitUntil("the Stress generator reading the Clock port at its ticks") { w.portCalls.count("Clock") >= clockCallsAtStart + 3 }

    // 6. Shutdown with a typed call in flight. This step and the next end the core, so they are the last of the run
    // (S17 is the last entry of SCENARIOS).
    val inFlight = CompletableFuture<Throwable?>()
    CoroutineScope(Dispatchers.Default).async {
        try {
            failLater(5_000u, 1)
            inFlight.complete(null)
        } catch (e: Throwable) {
            inFlight.complete(e)
            throw e
        }
    }
    Thread.sleep(100)
    val shutdownAt = System.nanoTime()
    w.core.close()
    val portCallsAtShutdown = w.portCalls.all()
    val ended = inFlight.get(WAIT_MS, TimeUnit.MILLISECONDS)
    check(ended is UndraException && "closed" in (ended.message ?: "")) { "fail_later across a shutdown ended with $ended, not a closed UndraException" }
    val tookMs = (System.nanoTime() - shutdownAt) / 1_000_000L
    check(tookMs < 1_000L) { "the call in flight took $tookMs ms to fail" }
    val closed = expectFails<UndraException>("add(1, 2) on the shut-down core") { add(1, 2, w.core) }
    check("closed" in (closed.message ?: "")) { "add on the shut-down core failed with $closed, not as closed" }
    val closedCommand = expectFails<UndraException>("Counter.increment() on a store of the shut-down core") { counter.increment() }
    check("closed" in (closedCommand.message ?: "")) { "increment on a store of the shut-down core failed with $closedCommand, not as closed" }
    counter.close()

    // 7, second half: closing ended the core's work (ADR-034), not only detached this host from it. The native
    // core says so itself: with no runtime loaded, `undra_stats_json` counts the threads `undra-runtime` started that
    // are still running, and a shutdown joins them (the `undra-core` thread that ran the generator above, the timer
    // thread that paced it). The port-call windows below cannot show a surviving task on Kotlin: the transport
    // detaches before the native shutdown, so a task of the old core that kept running would call into the old,
    // detached callbacks and never reach the adapters, and its sleeps run on the core's own timer thread. (Review of
    // runtime-lifecycle: a shutdown that only released the global slot passed both windows and the reload.)
    awaitUntil("the old core's threads to exit after close") { runtimeThreadsLeft() == 0L }
    var portCallsSeen = portCallsAtShutdown
    try {
        holdsFor("the port calls the adapters received after the shutdown", 200) {
            portCallsSeen = w.portCalls.all()
            portCallsSeen == portCallsAtShutdown
        }
    } catch (e: Mismatch) {
        fail("a port call reached the adapters after the shutdown: $portCallsAtShutdown at the shutdown, then $portCallsSeen")
    }
    stress.close()
    // ... and a new load in this process, with the same options, starts a fresh core: no live handles, and the
    // generated bindings work on it. It is closed again, which ends the run.
    val fresh = UndraCore.load(w.options)
    try {
        check(fresh !== w.core) { "UndraCore.load after the shutdown returned the closed core" }
        expectEq("live_handles of the fresh core", 0L, fresh.readStats().liveHandles)
        expectEq("add(1, 2) on the fresh core", 3, add(1, 2, fresh))
        // The fresh core runs no timer-paced task, so its Clock adapter stays quiet.
        val clockOnFresh = w.portCalls.count("Clock")
        holdsFor("the Clock calls the adapters received on the fresh core", 200) { w.portCalls.count("Clock") == clockOnFresh }
    } finally {
        fresh.close()
    }
    awaitUntil("the fresh core's threads to exit after close") { runtimeThreadsLeft() == 0L }
}

/**
 * The threads `undra-runtime` started that still run in this process, as the native library reports them while no
 * core is loaded (`runtime_threads` of `UndraNative.statsJson()`); fails if a core is loaded.
 */
private fun runtimeThreadsLeft(): Long {
    val raw = UndraNative.statsJson()
    val doc = Json.parseObject(raw)
    check(doc["initialized"] == false) { "a native core is still loaded after close: $raw" }
    return doc["runtime_threads"] as? Long ?: fail("the statistics of no core carry no runtime_threads: $raw")
}
