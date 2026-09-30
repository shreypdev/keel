import KeelRuntime
import PlaygroundCore
import XCTest

/// Defects found in merged code while writing the scenarios, each as a minimal repro.
///
/// A repro asserts the behaviour the defect violates inside `XCTExpectFailure`: it passes while
/// the defect is there, and fails ("unexpected pass") the day it is fixed, which is the signal to
/// remove the matching `WORKAROUND(..)` from the scenarios. They run after the scenarios (XCTest
/// orders classes alphabetically) and print no `SCENARIO` line.
@MainActor
final class Findings: XCTestCase {
    /// `BigList::insert_at` checks the index against `len + 1` and reports that bound, not the
    /// length, in `ListError::OutOfRange`.
    func testFinding_bigListInsertAtReportsTheLengthOfTheList() throws {
        let core = try Fixture.shared.core()
        let list = try BigList(ctx: core)
        defer { list.close() }
        let refused = outcome { () throws(ListError) -> UInt32 in try list.insertAt(index: 10_001, label: "x") }
        guard case .failure(let error) = refused else {
            return XCTFail("insert_at past the end must be refused")
        }
        XCTExpectFailure("playground-core: OutOfRange.len is len + 1 for insert_at")
        XCTAssertEqual(error, .outOfRange(index: 10_001, len: 10_000))
    }

    /// The generated `Probe.ticks(count:)` copies the runtime's pull-based stream into an
    /// unbounded `AsyncThrowingStream` from a task of its own (`keelDecodeStream` in
    /// `Objects.swift`), so nothing the consumer does limits how far the core runs ahead: after
    /// reading 5 of 1,000 items and waiting 200 ms, the core has produced all 1,000.
    func testFinding_generatedTicksIgnoreBackpressure() async throws {
        let core = try Fixture.shared.core()
        let probe = try Probe(ctx: core)
        defer { probe.close() }
        probe.reset()
        var iterator = probe.ticks(count: 1000).makeAsyncIterator()
        for _ in 0 ..< 5 {
            _ = try await iterator.next()
        }
        try await Task.sleep(for: .milliseconds(200))
        let produced = probe.counters().produced
        XCTExpectFailure("keel-bindgen: generated stream methods buffer without bound")
        XCTAssertLessThanOrEqual(produced, 5 + 64, "the core ran \(produced) items ahead of a consumer that read 5")
    }
}
