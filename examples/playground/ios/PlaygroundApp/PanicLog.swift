import Observation
import PlaygroundCore
import UndraRuntime

/// The panics of the core this process has seen (ADR-046), for the Debug section of the Remote tab: the last report and
/// how many there were. `UndraBootstrap` feeds it from `LoadOptions.onPanic`; an app would also hand each report to its
/// crash reporter there.
@MainActor @Observable
final class PanicLog {
    static let shared = PanicLog()

    /// The report of the most recent panic, or `nil` while the core has not panicked.
    private(set) var last: UndraPanicReport?

    /// How many panics have been reported.
    private(set) var count = 0

    func record(_ report: UndraPanicReport) {
        last = report
        count += 1
    }

    /// The report as one line: what was running, what it said and where.
    static func describe(_ report: UndraPanicReport) -> String {
        return "\(report.operation): \(report.message) (\(report.location))"
    }

    /// Makes the core panic inside a call: the call fails with `UndraCallError.panicked` and the core keeps working, and
    /// `onPanic` receives the report. The playground's `explode` exists for this.
    func trigger() {
        _ = try? explode(reason: "from the Debug section")
    }
}
