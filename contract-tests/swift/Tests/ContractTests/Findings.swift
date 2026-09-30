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
}
