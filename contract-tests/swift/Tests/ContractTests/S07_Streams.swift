import Foundation
import KeelRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    // MARK: S07

    func testS07_streamWithBackpressure() async {
        await scenario("S07", "stream with backpressure") {
            let core = try self.core
            let probe = try Probe(ctx: core)
            defer { probe.close() }

            // 1. Read exactly five items of a thousand, then stop reading without cancelling.
            probe.reset()
            var iterator = probe.ticks(count: 1000).makeAsyncIterator()
            for expected in 0 ..< 5 {
                let item = try await iterator.next()
                try checkEqual(item, UInt32(expected), "item \(expected) of the first five")
            }

            // 2. For 200 ms nothing more is read; the core has not run ahead by more than its credit window.
            try await quietFor(milliseconds: 200)
            let produced = probe.counters().produced
            try check(produced >= 5 && produced <= 5 + 64, "produced is \(produced) after reading 5 and waiting 200 ms, not within 5...69")
            // The Swift runtime grants 16 items of credit when a stream opens and tops the window up
            // when fewer than 8 are unread (SPEC 3.7), so with five read the core has sent 16, and
            // has made at most one more that waits for credit.
            try check(produced <= 16 + 1, "produced is \(produced) after reading 5 and waiting 200 ms, above the credit window of 16 plus the item waiting for credit")

            // 3. Resume: the other 995 arrive in order, the stream ends, and all 1,000 were produced.
            var next: UInt32 = 5
            while let item = try await iterator.next() {
                try checkEqual(item, next, "the item after \(next - 1)")
                next += 1
            }
            try checkEqual(next, 1000, "items read in total")
            try await waitUntil("produced to reach 1000") { probe.counters().produced == 1000 }

            // 4. Early termination: leaving the loop after three items cancels the stream in the core.
            let openBefore = core.stat("open_streams")
            let producedBefore = probe.counters().produced
            var read = 0
            for try await _ in probe.ticks(count: 1_000_000) {
                read += 1
                if read == 3 {
                    break
                }
            }
            try await waitUntil("open_streams to return to \(openBefore)") {
                core.stat("open_streams") == openBefore
            }
            let runAhead = probe.counters().produced - producedBefore
            try check(runAhead < 200, "a stream cut after 3 items produced \(runAhead) items (expected < 200)")

            // 5. Short streams end.
            var three: [UInt32] = []
            for try await item in probe.ticks(count: 3) {
                three.append(item)
            }
            try checkEqual(three, [0, 1, 2], "ticks(3)")
            var none: [UInt32] = []
            for try await item in probe.ticks(count: 0) {
                none.append(item)
            }
            try checkEqual(none, [], "ticks(0)")
        }
    }
}
