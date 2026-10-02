import UndraRuntime
import PlaygroundCore
import XCTest

/// The scenarios of contracts-tests/scenarios.md that Swift runs (S01 to S20, S23 to S28, S31), one test each:
/// `UndraRuntime` over the C ABI against the real playground core, through the generated bindings
/// wherever a UI would use them and through `UndraCore`'s own API for the raw checks.
///
/// XCTest runs a class's tests in alphabetical order, which is the order of the ids
/// (`testS01_` .. `testS26_`). They share one core. S16 shuts it down and loads it again, S17 shuts
/// it down at its end (and loads and closes a core of its own), and S18 then loads it once more
/// through `Fixture.core()`, which S19, S20 and S23 to S25 (the opt-in ports) use too. The build-B steps of
/// S14 and S15 run afterwards in a second process (`MigrationBuildB`).
@MainActor
final class ContractScenarios: ScenarioCase {
    /// The shared core.
    var core: UndraCore {
        get throws {
            return try Fixture.shared.core()
        }
    }
}
