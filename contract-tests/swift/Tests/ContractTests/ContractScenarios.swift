import KeelRuntime
import PlaygroundCore
import XCTest

/// The seventeen scenarios of contracts-tests/scenarios.md, one test each, for Swift: `KeelRuntime`
/// over the C ABI against the real playground core, through the generated bindings wherever a UI
/// would use them and through `KeelCore`'s own API for the raw checks.
///
/// XCTest runs a class's tests in alphabetical order, which is the order of the ids
/// (`testS01_` .. `testS17_`). They share one core; S16 is the one that shuts it down and loads it
/// again, so it is ordered before S17 on purpose.
@MainActor
final class ContractScenarios: ScenarioCase {
    /// The shared core.
    var core: KeelCore {
        get throws {
            return try Fixture.shared.core()
        }
    }
}
