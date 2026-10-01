import Foundation
import UndraRuntime
import XCTest

/// The base of the scenario class: reports every scenario in the format `check.sh` reads.
///
///     SCENARIO S07 PASS stream with backpressure
///     SCENARIO S07 FAIL <the first check that did not hold>
///
/// A scenario that fails is also an XCTest failure, so `swift test` exits non-zero.
@MainActor
class ScenarioCase: XCTestCase {
    /// Runs one scenario and prints its line.
    func scenario(_ id: String, _ title: String, _ body: @MainActor () async throws -> Void) async {
        do {
            try await body()
            ScenarioCase.report("SCENARIO \(id) PASS \(title)")
        } catch {
            let reason = ScenarioCase.oneLine("\(error)")
            ScenarioCase.report("SCENARIO \(id) FAIL \(title): \(reason)")
            XCTFail("\(id) \(title): \(reason)")
        }
    }

    /// A scenario that cannot run here, with the reason scenarios.md has to carry.
    func skipScenario(_ id: String, _ title: String, because reason: String) {
        ScenarioCase.report("SCENARIO \(id) SKIP \(title): \(reason)")
    }

    /// Prints a line and flushes it, so the log is complete even if a later scenario crashes the process.
    nonisolated static func report(_ line: String) {
        print(line)
        fflush(stdout)
    }

    private nonisolated static func oneLine(_ text: String) -> String {
        return text.replacingOccurrences(of: "\n", with: " ")
    }
}
