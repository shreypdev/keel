import Foundation
import UndraRuntime
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
            let produced = try probe.counters().produced
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
            try await waitUntil("produced to reach 1000") { try probe.counters().produced == 1000 }

            // 4. Early termination: leaving the loop after three items cancels the stream in the core.
            let openBefore = core.stat("open_streams")
            let producedBefore = try probe.counters().produced
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
            let runAhead = try probe.counters().produced - producedBefore
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

            // 6. A typed error part-way (ADR-036): the stream's own `LabError` ends it after the
            // items before it, through the generated `mapError` (never a wire error).
            var beforeError: [UInt32] = []
            try await checkThrows(
                {
                    for try await item in probe.ticksThenFail(count: 5, failAt: 3, code: 7) {
                        beforeError.append(item)
                    }
                },
                LabError.rejected(code: 7, reason: "stopped at 3"),
                "the end of ticksThenFail(5, 3, 7)"
            )
            try checkEqual(beforeError, [0, 1, 2], "the items of ticksThenFail(5, 3, 7) before its error")
            var complete: [UInt32] = []
            for try await item in probe.ticksThenFail(count: 3, failAt: 9, code: 7) {
                complete.append(item)
            }
            try checkEqual(complete, [0, 1, 2], "ticksThenFail(3, 9, 7)")

            // 7. A stream the core cancels (ADR-036: flag 3, status 3). The probe is not a store, so a
            // restore invalidates it and ends its stream, which has an error type: the loop fails as
            // cancelled by the core, not as a `LabError` and not as `.malformed`.
            let openBeforeRestore = core.stat("open_streams")
            let snapshot = try core.snapshot()
            var cancelled = probe.ticksThenFail(count: 1_000_000, failAt: 999_999, code: 1).makeAsyncIterator()
            for expected in 0 ..< 2 {
                let item = try await cancelled.next()
                try checkEqual(item, UInt32(expected), "item \(expected) of the stream the restore ends")
            }
            let restoredAt = ContinuousClock.now
            try core.restore(snapshot)
            // The items the core sent before the restore are still read, in order, then the failure.
            var afterRestore: [UInt32] = []
            let ending: Result<Void, any Error>
            do {
                while let item = try await cancelled.next() {
                    afterRestore.append(item)
                }
                ending = .success(())
            } catch {
                ending = .failure(error)
            }
            let tookToFail = ContinuousClock.now - restoredAt
            switch ending {
            case .success:
                throw ScenarioFailure(description: "the stream ended normally across a restore, after \(afterRestore.count) more items")
            case .failure(let error):
                try check(!(error is LabError), "a stream cancelled by the core failed with its own error: \(error)")
                try check(!(error is CancellationError), "a stream cancelled by the core failed as a CancellationError")
                try checkEqual(error as? UndraCallError, .cancelledByCore, "the error of ticksThenFail across a restore")
            }
            try check(tookToFail < .seconds(1), "the stream took \(tookToFail) to fail after the restore")
            try checkEqual(afterRestore, Array(2 ..< 2 + UInt32(afterRestore.count)), "the items read after the restore")
            try await waitUntil("open_streams to return to \(openBeforeRestore)") {
                core.stat("open_streams") == openBeforeRestore
            }
            // The probe is closed by the `defer` above.
        }
    }
}
