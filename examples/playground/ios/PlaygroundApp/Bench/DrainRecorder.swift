import Foundation
import UndraRuntime

/// What the mirror's drain listener reports, kept for the drain experiment. The listener runs on the main actor, the producer
/// thread waits on it, so the state is behind a lock.
final class DrainRecorder: @unchecked Sendable {
    /// One drain: what it consumed, what it applied, how long the main thread was in it.
    struct Drain {
        var changeSets: Int
        var entries: Int
        var applied: Int
        var durationNs: Double
    }

    private let lock = NSLock()
    private var drains: [Drain] = []
    private var changeSetsSeen = 0

    /// Called by the drain listener (any thread).
    func record(_ stats: DrainStats) {
        let c = stats.duration.components
        let ns = Double(c.seconds) * 1e9 + Double(c.attoseconds) / 1e9
        lock.lock()
        drains.append(Drain(changeSets: stats.changeSets, entries: stats.entries, applied: stats.appliedEntries, durationNs: ns))
        changeSetsSeen += stats.changeSets
        lock.unlock()
    }

    /// Forgets everything recorded so far.
    func reset() {
        lock.lock()
        drains = []
        changeSetsSeen = 0
        lock.unlock()
    }

    /// How many drains and how many change-sets have been recorded.
    var position: (drains: Int, changeSets: Int) {
        lock.lock()
        defer { lock.unlock() }
        return (drains.count, changeSetsSeen)
    }

    /// The drains recorded since the `index`th.
    func drains(since index: Int) -> [Drain] {
        lock.lock()
        defer { lock.unlock() }
        return Array(drains[index...])
    }

    /// Blocks the calling (non-main) thread until `changeSets` change-sets in total have been drained; false on a timeout.
    func wait(forChangeSets changeSets: Int, timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while position.changeSets < changeSets {
            if Date() > deadline { return false }
            Thread.sleep(forTimeInterval: 0.0002)
        }
        return true
    }
}
