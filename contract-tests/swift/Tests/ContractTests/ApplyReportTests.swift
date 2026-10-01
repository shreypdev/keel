import Foundation
@testable import UndraRuntime
import PlaygroundCore
import XCTest

/// ADR-032, decision 6: a generated store that cannot decode a change from the core skips it and
/// reports it through `LoadOptions.onError`, and keeps working. Not a scenario of scenarios.md (no
/// platform shows the core sending an undecodable change); it runs before the scenarios (XCTest orders
/// classes by name) on the core they share.
@MainActor
final class ApplyReportTests: XCTestCase {
    func testAStoreSkipsAndReportsAChangeItCannotDecode() async throws {
        let core = try Fixture.shared.core()
        let counter = try Counter(ctx: core)
        defer { counter.close() }
        let reportsBefore = Fixture.shared.unhandled.snapshot.count

        // `Counter` signal 0 is `count: i32`; one byte is not a full value of it.
        let malformed = Wire.ChangeSet(
            txnId: 999,
            entries: [Wire.ChangeEntry(handle: counter.handle, signalId: 0, op: .fullValue, value: [0xFF])]
        )
        core.mirror.enqueue(malformed.encode())
        core.mirror.flush()

        let reports = Array(Fixture.shared.unhandled.snapshot.dropFirst(reportsBefore))
        XCTAssertEqual(reports.count, 1, "\(reports)")
        XCTAssertEqual(reports.first?.operation, "Counter.apply(signal: 0)")
        guard case .malformed? = reports.first?.error else {
            return XCTFail("expected .malformed, got \(String(describing: reports.first?.error))")
        }

        // The entry was skipped and the store still follows the core.
        XCTAssertEqual(counter.count, 0)
        counter.increment()
        try await waitUntil("the store to follow the core after the skipped change") { counter.count == 1 }
    }

    func testAChangeWithTrailingBytesIsSkippedWholeNotHalfApplied() async throws {
        let core = try Fixture.shared.core()
        let counter = try Counter(ctx: core)
        defer { counter.close() }
        let reportsBefore = Fixture.shared.unhandled.snapshot.count

        // A whole `i32` (7) followed by a stray byte: the value decodes, the payload does not.
        let malformed = Wire.ChangeSet(
            txnId: 998,
            entries: [Wire.ChangeEntry(handle: counter.handle, signalId: 0, op: .fullValue, value: [7, 0, 0, 0, 0xFF])]
        )
        core.mirror.enqueue(malformed.encode())
        core.mirror.flush()

        let reports = Array(Fixture.shared.unhandled.snapshot.dropFirst(reportsBefore))
        XCTAssertEqual(reports.map { $0.operation }, ["Counter.apply(signal: 0)"])
        XCTAssertEqual(counter.count, 0, "a change that does not decode whole is not stored")
    }
}
