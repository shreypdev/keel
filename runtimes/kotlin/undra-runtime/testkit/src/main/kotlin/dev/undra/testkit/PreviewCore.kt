package dev.undra.testkit

import dev.undra.runtime.LoadOptions
import dev.undra.runtime.Mode
import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraDispatchers
import dev.undra.runtime.UndraUnhandledError
import kotlinx.coroutines.runBlocking

/**
 * The app's own core, loaded in this process with the deterministic [Fakes] as its ports and a manual clock, for a unit test, an instrumented
 * test or a screenshot test: the real logic, scripted ports, and time that moves when you say so.
 *
 * ```kotlin
 * val preview = PreviewCore.load(
 *     UndraPlaygroundCore::load,                         // the entry of the app's bindings (ADR-044)
 *     seed = Seed.fromJson(seedJson),                   // or Seed(http = listOf(...)), or script preview.fakes by hand
 * )
 * val todos = Todos(preview.core)
 * preview.advance(31_000)                                // the cached list goes stale; the core refetches
 * ```
 *
 * The core is the one `undra build --platform host` (a desktop JVM) or the Android `.so` provides, loaded through the entry of its bindings, so it
 * is also the core the bindings' stores use by default. Android Studio's preview pane runs on a desktop JVM that cannot load an Android library,
 * so a Compose `@Preview` uses [RecordedCore] instead.
 *
 * A process holds one in-process core per namespace. [load] closes the shared one first (a refreshed preview does the same), unless
 * `replaceCurrent` is `false`.
 */
public class PreviewCore private constructor(
    /** The core. */
    public val core: UndraCore,
    /** The fakes it runs on: script `fakes.http`, seed `fakes.kv`, read `fakes.log`. */
    public val fakes: Fakes,
) : AutoCloseable {
    /** The manual clock: [FakeClock.nowMs] reads it, [FakeClock.setNowMs] jumps the wall clock, [advance] moves time. */
    public val clock: FakeClock get() = fakes.clock

    /**
     * Lets the core catch up and the stores see what it produced: waits until the core's counters have stood still for [quietMs] and it has answered
     * every port call, then applies what the mirror holds. The core runs on a thread of its own, so "idle" is observed, not known: raise [quietMs] on a
     * machine that is busy.
     */
    public fun settle(quietMs: Long = 20, timeoutMs: Long = 5_000) {
        val deadline = System.nanoTime() + timeoutMs * 1_000_000
        var quiet = 0
        var last = ""
        while (System.nanoTime() < deadline) {
            val stats = core.stats()
            val now = "${field(stats.raw, "polls")}:${stats.pendingPortCalls}:${stats.activeCalls}:${stats.hostPendingCalls}"
            if (now == last && stats.pendingPortCalls <= 0) quiet++ else quiet = 0
            last = now
            if (quiet >= (quietMs / 2).coerceAtLeast(1)) break
            Thread.sleep(2)
        }
        applied()
    }

    private fun field(json: String, name: String): Long = ((parseJson(json) as? Json.Obj)?.fields?.get(name) as? Json.Num)?.raw?.toLongOrNull() ?: -1L

    private fun applied() {
        if (UndraDispatchers.isMainThread()) core.mirror.flush() else runBlocking(UndraDispatchers.main) { core.mirror.flush() }
    }

    /**
     * Moves the manual clock forward by [ms], one deadline at a time: at each deadline the clock reads exactly that instant, the due timer fires
     * into the core (completing a `ctx.sleep`, or a stale-time refetch), and the core settles before time moves on, so a task that sleeps again
     * inside the window is served within the same call. Returns how many timers fired.
     */
    public fun advance(ms: Long): Int {
        var left = ms.coerceAtLeast(0)
        var fired = 0
        settle()
        while (true) {
            val due = fakes.clock.nextDueInMs() ?: break
            if (due > left) break
            if (fakes.clock.fireNext(left) == null) break
            left -= due
            fired++
            settle()
        }
        fakes.clock.moveBy(left)
        settle()
        return fired
    }

    /** Closes the core. */
    override fun close() {
        fakes.clock.onTimerFired = null
        core.close()
    }

    /** Loading. */
    public companion object {
        /**
         * Loads the core with the fakes installed.
         *
         * @param entry the load function of the bindings the app was generated with (`UndraPlaygroundCore::load`): it knows the core's natives and
         *   its schema hash, and remembers the core for the generated classes.
         * @param seed the starting state of the fakes, applied before the core starts.
         * @param fakes fakes to use instead of fresh ones (for example ones a test already holds).
         * @param adapters further ports on top of the fakes (an app's own port, or a [Replayer]'s ports).
         * @param replaceCurrent close the shared core first, if there is one (a process holds one in-process core per namespace).
         */
        public fun load(
            entry: (LoadOptions) -> UndraCore,
            seed: Seed? = null,
            fakes: Fakes = Fakes(),
            adapters: Map<UInt, PortImpl> = emptyMap(),
            replaceCurrent: Boolean = true,
            onError: ((UndraUnhandledError) -> Unit)? = null,
        ): PreviewCore {
            seed?.apply(fakes)
            if (replaceCurrent) UndraCore.current?.close()
            val options = LoadOptions(
                mode = Mode.INPROC,
                adapters = fakes.ports() + adapters,
                defaultAdapters = false,
                onError = onError,
            )
            val core = entry(options)
            fakes.clock.onTimerFired = { core.timerFired(it) }
            fakes.connectivity.attach(core)
            fakes.lifecycle.attach(core)
            val preview = PreviewCore(core, fakes)
            preview.settle()
            return preview
        }
    }
}
