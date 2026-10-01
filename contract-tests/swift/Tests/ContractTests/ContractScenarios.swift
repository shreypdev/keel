import UndraRuntime
import PlaygroundCore
import XCTest

/// The scenarios of contracts-tests/scenarios.md (S01..S19, S23..S26), one test each, for Swift: `UndraRuntime`
/// over the C ABI against the real playground core, through the generated bindings wherever a UI
/// would use them and through `UndraCore`'s own API for the raw checks.
///
/// XCTest runs a class's tests in alphabetical order, which is the order of the ids
/// (`testS01_` .. `testS26_`). They share one core. S16 shuts it down and loads it again, S17 shuts
/// it down at its end (and loads and closes a core of its own), and S18 then loads it once more
/// through `Fixture.core()`; S19 and S23 to S25 (the opt-in ports) use that core too.
@MainActor
final class ContractScenarios: ScenarioCase {
    /// The shared core.
    var core: UndraCore {
        get throws {
            return try Fixture.shared.core()
        }
    }
}
