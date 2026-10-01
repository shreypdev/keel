import XCTest

/// Drives the device benchmark (`PlaygroundApp/Bench`): launches the app in benchmark mode, waits for it to say it is done and
/// hands the JSON it shows to `scripts/bench-device.sh`. The tests skip unless `UNDRA_BENCH=1` is in the environment of
/// the test run (`TEST_RUNNER_UNDRA_BENCH=1 xcodebuild test ...`), so the smoke run's `xcodebuild test` never starts a
/// benchmark; the script sets it.
///
/// The result leaves the test two ways: as an attachment named `bench-<mode>-<n>.json` (which is what a physical
/// device has: `xcresulttool` exports it), and as one line on the test's output, `UNDRA_BENCH_RESULT <json>`,
/// which is what the script reads first.
///
/// | Variable | Meaning |
/// |---|---|
/// | `UNDRA_BENCH` | `1` to run |
/// | `UNDRA_BENCH_QUICK` | `1` for the short run (checks the plumbing in seconds) |
/// | `UNDRA_BENCH_COLD_RUNS` | cold launches after the full run (default 10) |
/// | `UNDRA_BENCH_OUT` | a directory to also write `bench-<mode>-<n>.json` in (a simulator shares the Mac's disk) |
@MainActor
final class PlaygroundBenchTests: XCTestCase {
    private var environment: [String: String] { ProcessInfo.processInfo.environment }

    override func setUpWithError() throws {
        continueAfterFailure = false
        try XCTSkipUnless(environment["UNDRA_BENCH"] == "1", "set TEST_RUNNER_UNDRA_BENCH=1 to run the device benchmark")
    }

    /// Launches the app in `mode`, waits until it is `done`, and returns the JSON it produced.
    private func run(mode: String, timeout: TimeInterval) throws -> String {
        let app = XCUIApplication()
        app.launchArguments += ["-bench", mode]
        if environment["UNDRA_BENCH_QUICK"] == "1" { app.launchArguments += ["-benchQuick", "YES"] }
        app.launch()
        let status = app.staticTexts["bench-status"]
        XCTAssertTrue(status.waitForExistence(timeout: 30), "the benchmark screen never appeared")
        let done = NSPredicate(format: "label == 'done' OR label BEGINSWITH 'failed'")
        XCTAssertEqual(XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: done, object: status)], timeout: timeout), .completed,
                       "the benchmark did not finish within \(Int(timeout)) s (status: \(status.label))")
        XCTAssertEqual(status.label, "done", "the benchmark failed: \(status.label)")
        var json = ""
        var index = 0
        while true {
            let piece = app.staticTexts["bench-result-\(index)"]
            if !piece.exists { break }
            json += piece.label
            index += 1
        }
        XCTAssertFalse(json.isEmpty, "the benchmark produced no result")
        app.terminate()
        return json
    }

    private func publish(_ json: String, mode: String, number: Int) {
        print("UNDRA_BENCH_RESULT \(json)")
        let attachment = XCTAttachment(string: json)
        attachment.name = "bench-\(mode)-\(number).json"
        attachment.lifetime = .keepAlways
        add(attachment)
        if let directory = environment["UNDRA_BENCH_OUT"] {
            try? FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
            try? json.write(toFile: "\(directory)/bench-\(mode)-\(number).json", atomically: true, encoding: .utf8)
        }
    }

    /// The full run, then the cold launches (each a fresh process: the first load of a process and the restore of the snapshot the full run left).
    func testDeviceBench() throws {
        let full = try run(mode: "full", timeout: 600)
        publish(full, mode: "full", number: 0)
        let colds = Int(environment["UNDRA_BENCH_COLD_RUNS"] ?? "") ?? (environment["UNDRA_BENCH_QUICK"] == "1" ? 2 : 10)
        for n in 0 ..< max(colds, 0) {
            publish(try run(mode: "cold", timeout: 120), mode: "cold", number: n + 1)
        }
    }
}
