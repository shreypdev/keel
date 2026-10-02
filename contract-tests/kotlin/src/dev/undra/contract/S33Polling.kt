package dev.undra.contract

import dev.undra.playground.core.TickError
import dev.undra.playground.core.TickerQueryHandle
import dev.undra.playground.core.setTickerFailing
import dev.undra.playground.core.tickerFetches
import dev.undra.runtime.adapters.AppState
import dev.undra.runtime.adapters.LifecycleEvents
import dev.undra.runtime.adapters.NetKind
import kotlin.time.Duration.Companion.seconds

/** How long a poll may take to show: the interval is one second (scenarios.md S33 steps 1 to 4). */
private const val WITHIN_MS: Long = 2_500L

/**
 * Waits until `ticker_fetches()` reaches [count] and returns when it was first seen (`System.nanoTime`). Polls the core's own
 * counter every few milliseconds, so the time is that of the fetch within the poll's resolution.
 */
private fun awaitFetches(count: UInt, timeoutMs: Long): Long {
    val deadline = System.nanoTime() + timeoutMs * 1_000_000L
    while (true) {
        if (tickerFetches() >= count) return System.nanoTime()
        if (System.nanoTime() > deadline) fail("the ticker did not reach $count fetches within $timeoutMs ms (at ${tickerFetches()})")
        Thread.sleep(3L)
    }
}

private fun millis(from: Long, to: Long): Long = (to - from) / 1_000_000L

/**
 * S33 (ADR-043): polling. `ticker` is a query with `interval = "1s"` that returns a counter bumped by every fetch. The core's
 * Timer is the runtime's real one (real time); the `Lifecycle` and `Connectivity` events are the harness's.
 */
fun s33Polling(w: World) {
    val lifecycle = LifecycleEvents(w.core)
    setTickerFailing(false)
    val ticker = TickerQueryHandle.create()
    try {
        // 1. A poll after each fetch, counted from the end of the one before.
        awaitEq("data of the observed ticker", 1u as UInt?) { ticker.data.value }
        val first = awaitFetches(1u, WITHIN_MS)
        val second = awaitFetches(2u, WITHIN_MS)
        awaitEq("data after the first poll", 2u as UInt?, WITHIN_MS) { ticker.data.value }
        check(tickerFetches() >= 2u) { "ticker_fetches() is ${tickerFetches()} after the first poll" }
        check(millis(first, second) >= 900L) { "two consecutive fetches were ${millis(first, second)} ms apart; the interval is a second" }

        // 2. Background pauses, Active resumes.
        lifecycle.changed(AppState.BACKGROUND)
        Thread.sleep(100L)
        val inBackground = tickerFetches()
        holdsFor("ticker_fetches() while the app is in the background", 1_500L) { tickerFetches() == inBackground }
        val before = ticker.data.value ?: 0u
        lifecycle.changed(AppState.ACTIVE)
        awaitUntil("data to advance after Active", WITHIN_MS) { (ticker.data.value ?: 0u) > before }

        // 3. Offline pauses, online resumes.
        w.connectivity.changed(online = false, kind = NetKind.NONE)
        Thread.sleep(100L)
        val offline = tickerFetches()
        holdsFor("ticker_fetches() while the network is offline", 1_500L) { tickerFetches() == offline }
        val beforeOnline = ticker.data.value ?: 0u
        w.connectivity.changed(online = true, kind = NetKind.WIFI)
        awaitUntil("data to advance after the network came back", WITHIN_MS) { (ticker.data.value ?: 0u) > beforeOnline }

        // 4. A failure keeps polling and clears on success.
        setTickerFailing(true)
        awaitEq("the error of a failing ticker", TickError.Failing as TickError?, WITHIN_MS) { ticker.error.value }
        val failing = tickerFetches()
        awaitFetches(failing + 1u, WITHIN_MS)
        awaitFetches(failing + 2u, WITHIN_MS)
        setTickerFailing(false)
        awaitEq("the error once the ticker works again", null as TickError?, WITHIN_MS) { ticker.error.value }
        val recovered = ticker.data.value ?: 0u
        awaitUntil("data to advance once the ticker works again", WITHIN_MS) { (ticker.data.value ?: 0u) > recovered }

        // 5. An observer's override: the gap after the next fetch is the override's, and clearing it returns to the query's second.
        ticker.setPollInterval(3.seconds)
        val next = tickerFetches() + 1u
        val ended = awaitFetches(next, 4_000L)
        val following = awaitFetches(next + 1u, 6_000L)
        check(millis(ended, following) >= 2_900L) { "with a 3 s override the following gap was ${millis(ended, following)} ms" }
        ticker.setPollInterval(null)
        val cleared = tickerFetches()
        awaitFetches(cleared + 1u, 6_000L)
        val a = awaitFetches(cleared + 2u, 4_000L)
        val b = awaitFetches(cleared + 3u, 4_000L)
        check(millis(a, b) in 900L..2_500L) { "after clearing the override the gap was ${millis(a, b)} ms, not about a second" }
    } finally {
        ticker.close()
    }

    // 6. The last observer stops it.
    Thread.sleep(100L)
    val stopped = tickerFetches()
    holdsFor("ticker_fetches() after the handle was closed", WITHIN_MS) { tickerFetches() == stopped }
}
