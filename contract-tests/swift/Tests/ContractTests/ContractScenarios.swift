import UndraRuntime
import PlaygroundCore
import XCTest

/// The eighteen scenarios of contracts-tests/scenarios.md, one test each, for Swift: `UndraRuntime`
/// over the C ABI against the real playground core, through the generated bindings wherever a UI
/// would use them and through `UndraCore`'s own API for the raw checks.
///
/// XCTest runs a class's tests in alphabetical order, which is the order of the ids
/// (`testS01_` .. `testS18_`). They share one core. S16 shuts it down and loads it again, S17 shuts
/// it down at its end (and loads and closes a core of its own), and S18 then loads it once more
/// through `Fixture.core()`.
@MainActor
final class ContractScenarios: ScenarioCase {
    /// The shared core.
    var core: UndraCore {
        get throws {
            return try Fixture.shared.core()
        }
    }
}
