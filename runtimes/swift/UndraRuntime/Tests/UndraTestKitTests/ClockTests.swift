import XCTest

@testable import UndraTestKit

final class ClockTests: XCTestCase {
    func testAManualClockFiresDueTimersInDeadlineOrderTiesInArmingOrderAndATimerArmedInsideTheWindowFiresInTheSameCall() throws {
        let clock = FakeClock(nowMs: 0)
        let order = Locked([UInt32]())
        clock.onTimerFired = { id in
            order.withLock { $0.append(id) }
            if id == 1 {
                clock.set(timerId: 4, delayMs: 5)  // due at 15, inside the window
            }
        }
        clock.set(timerId: 3, delayMs: 20)
        clock.set(timerId: 1, delayMs: 10)
        clock.set(timerId: 2, delayMs: 10)
        XCTAssertEqual(try clock.advance(ms: 30), [1, 2, 4, 3])
        XCTAssertEqual(order.withLock { $0 }, [1, 2, 4, 3])
        XCTAssertEqual(clock.monotonicNs, 30_000_000)
    }

    func testATimerThatReArmsItselfWithoutTimePassingStopsAtTheCapWithATypedErrorAndTheClockStaysWhereItWas() {
        let clock = FakeClock(nowMs: 0)
        clock.onTimerFired = { id in clock.set(timerId: id, delayMs: 0) }
        clock.set(timerId: 7, delayMs: 0)
        XCTAssertThrowsError(try clock.advance(ms: 1_000, maxTimers: 50)) { error in
            XCTAssertEqual(error as? TimerStormError, TimerStormError(fired: 50, timerId: 7, atMs: 0))
            XCTAssertTrue("\(error)".contains("timer 7"))
        }
        XCTAssertEqual(clock.monotonicNs, 0)
        XCTAssertEqual(clock.pendingTimerIds(), [7])
    }

    func testAPeriodicTimerIsFineUntilItsWindowHoldsMoreFiringsThanTheCap() throws {
        let clock = FakeClock(nowMs: 0)
        let count = Locked(0)
        clock.onTimerFired = { id in
            count.withLock { $0 += 1 }
            clock.set(timerId: id, delayMs: 1)
        }
        clock.set(timerId: 1, delayMs: 1)
        XCTAssertEqual(try clock.advance(ms: 100).count, 100)
        XCTAssertThrowsError(try clock.advance(ms: 10_000, maxTimers: 500)) { XCTAssertTrue($0 is TimerStormError) }
        XCTAssertEqual(count.withLock { $0 }, 600)
    }
}
