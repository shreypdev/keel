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
            // WORKAROUND(keel-bindgen): the generated `Probe.ticks(count:)` copies the runtime's
            // pull-based stream into an unbounded `AsyncThrowingStream` from a task of its own, so
            // the consumer never applies backpressure and the core produces all 1,000 items at
            // once (the repro is in Findings.swift). The steps that assert backpressure therefore
            // read `KeelCore.stream`, which is what the generated method wraps.
            var iterator = self.rawTicks(core, probe, count: 1000).makeAsyncIterator()
            for expected in 0 ..< 5 {
                let item = try await iterator.next()
                try checkEqual(item, UInt32(expected), "item \(expected) of the first five")
            }

            // 2. For 200 ms nothing more is read; the core has not run ahead by more than its credit window.
            try await quietFor(milliseconds: 200)
            let produced = probe.counters().produced
            try check(produced >= 5 && produced <= 5 + 64, "produced is \(produced) after reading 5 and waiting 200 ms, not within 5...69")

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
            for try await _ in self.rawTicks(core, probe, count: 1_000_000) {
                read += 1
                if read == 3 {
                    break
                }
            }
            try await waitUntil("open_streams to return to \(openBefore)", timeout: .seconds(1)) {
                core.stat("open_streams") == openBefore
            }
            let runAhead = probe.counters().produced - producedBefore
            try check(runAhead < 200, "a stream cut after 3 items produced \(runAhead) items (expected < 200)")

            // 5. Short streams end. These use the generated method: no backpressure is involved.
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

    /// `Probe.ticks(count:)` as the runtime offers it: a pull-based stream of 4-byte items.
    private func rawTicks(_ core: KeelCore, _ probe: Probe, count: UInt32) -> AsyncThrowingStream<UInt32, any Error> {
        let source = core.stream(
            .objectMethod(handle: probe.handle, methodId: KeelIds.Objects.Probe.ticks),
            method: KeelIds.Objects.Probe.ticks,
            args: encoded { (w: inout KeelWriter) in w.writeU32(count) }
        )
        var bytes = source.makeAsyncIterator()
        return AsyncThrowingStream<UInt32, any Error>(unfolding: {
            guard let item = try await bytes.next() else {
                return nil
            }
            return try UInt32.keelDecoded(from: item)
        })
    }
}
