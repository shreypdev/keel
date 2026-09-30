import Foundation
@testable import KeelRuntime

/// The `Clock` port of the harness: a clock the test sets and advances, starting at
/// 1,700,000,000,000 ms. The query cache reads it for staleness; timers are not the clock's
/// (the `Timer` adapter stays real), so advancing it never fires a sleep early.
final class ManualClock: KeelAdapter, @unchecked Sendable {
    /// Where every run starts, in milliseconds since the Unix epoch.
    static let start: Int64 = 1_700_000_000_000

    private let milliseconds = Locked<Int64>(ManualClock.start)

    var portId: UInt32 {
        return StandardPorts.Clock.portId
    }

    /// The current reading in milliseconds since the epoch.
    var nowMs: Int64 {
        return milliseconds.snapshot
    }

    /// The current reading as a `Date` (what a query's `updatedAt` is compared with).
    var now: Date {
        return Date(timeIntervalSince1970: Double(nowMs) / 1000.0)
    }

    /// Moves the clock forward.
    func advance(seconds: Int64) {
        milliseconds.withLock { (current: inout Int64) -> Void in
            current += seconds * 1000
        }
    }

    func makePortImpl(core: KeelCore) -> PortImpl? {
        let milliseconds = self.milliseconds
        return .sync([
            StandardPorts.Clock.nowMs: { _ in
                return milliseconds.snapshot.keelEncoded()
            },
            StandardPorts.Clock.monotonicNs: { _ in
                // Only differences mean anything; deriving it from the manual reading keeps
                // the two clocks consistent.
                return UInt64(milliseconds.snapshot - ManualClock.start).keelEncoded()
            },
        ])
    }
}
