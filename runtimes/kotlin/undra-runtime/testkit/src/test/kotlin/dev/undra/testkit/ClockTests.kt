package dev.undra.testkit

import dev.undra.testkit.testing.Suite
import dev.undra.testkit.testing.assertEq
import dev.undra.testkit.testing.assertTrue
import org.junit.jupiter.api.Test

class ClockTests : Suite() {
    init {
        case("a manual clock fires due timers in deadline order, ties in arming order, and a timer armed inside the window fires in the same call") {
            val clock = FakeClock(0)
            val order = ArrayList<UInt>()
            clock.onTimerFired = { id ->
                order += id
                if (id == 1u) clock.set(4u, 5uL) // due at 15, inside the window
            }
            clock.set(3u, 20uL)
            clock.set(1u, 10uL)
            clock.set(2u, 10uL)
            assertEq(listOf(1u, 2u, 4u, 3u), clock.advance(30), "fired")
            assertEq(listOf(1u, 2u, 4u, 3u), order, "order")
            assertEq(30_000_000uL, clock.monotonicNs, "the clock moved the whole window")
        }

        case("a timer that re-arms itself without time passing stops at the cap with a typed error, and the clock stays where it was") {
            val clock = FakeClock(0)
            clock.onTimerFired = { id -> clock.set(id, 0uL) }
            clock.set(7u, 0uL)
            val error = try {
                clock.advance(1_000, maxTimers = 50)
                null
            } catch (e: TimerStormException) {
                e
            }
            assertTrue(error != null, "advance threw TimerStormException")
            assertEq(50, error!!.fired, "fired")
            assertEq(7u, error.timerId, "timerId")
            assertEq(0L, error.atMs, "atMs")
            assertEq(0uL, clock.monotonicNs, "the clock did not move")
            assertEq(1, clock.pendingTimers, "the timer is still armed")
        }

        case("a periodic timer is fine until its window holds more firings than the cap") {
            val clock = FakeClock(0)
            var count = 0
            clock.onTimerFired = { id ->
                count++
                clock.set(id, 1uL)
            }
            clock.set(1u, 1uL)
            assertEq(100, clock.advance(100).size, "a hundred ticks")
            val error = try {
                clock.advance(10_000, maxTimers = 500)
                null
            } catch (e: TimerStormException) {
                e
            }
            assertTrue(error != null, "the cap stopped it")
            assertEq(600, count, "fired")
        }
    }

    @Test
    fun allCases() = assertPassed()
}
