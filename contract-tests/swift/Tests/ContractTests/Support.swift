import Foundation
import KeelRuntime

/// Why a scenario failed: the first check that did not hold.
struct ScenarioFailure: Error, CustomStringConvertible {
    let description: String
}

/// Fails the scenario with `message` unless `condition` holds.
func check(
    _ condition: @autoclosure () -> Bool,
    _ message: @autoclosure () -> String,
    file: StaticString = #fileID,
    line: UInt = #line
) throws {
    if !condition() {
        throw ScenarioFailure(description: "\(message()) (\(file):\(line))")
    }
}

/// Fails the scenario unless `actual == expected`.
func checkEqual<Value: Equatable>(
    _ actual: @autoclosure () throws -> Value,
    _ expected: @autoclosure () throws -> Value,
    _ what: @autoclosure () -> String,
    file: StaticString = #fileID,
    line: UInt = #line
) throws {
    let actualValue = try actual()
    let expectedValue = try expected()
    if actualValue != expectedValue {
        throw ScenarioFailure(description: "\(what()): expected \(expectedValue), got \(actualValue) (\(file):\(line))")
    }
}

/// Fails the scenario unless `body` throws an error equal to `expected`.
func checkThrows<Failure: Error & Equatable>(
    _ expected: Failure,
    _ what: @autoclosure () -> String,
    file: StaticString = #fileID,
    line: UInt = #line,
    _ body: () async throws -> Void
) async throws {
    do {
        try await body()
    } catch let error as Failure {
        try checkEqual(error, expected, what(), file: file, line: line)
        return
    } catch {
        throw ScenarioFailure(description: "\(what()): expected \(expected), got another error: \(error) (\(file):\(line))")
    }
    throw ScenarioFailure(description: "\(what()): expected \(expected), but nothing was thrown (\(file):\(line))")
}

/// How long anything asynchronous may take (scenarios.md, "Waiting").
let waitLimit: Duration = .seconds(5)

/// Polls `condition` every 10 ms (yielding the main actor, so the mirror can apply change-sets)
/// until it holds, and fails the scenario after five seconds. Never a bare sleep.
@MainActor
func waitUntil(
    _ what: String,
    timeout: Duration = waitLimit,
    file: StaticString = #fileID,
    line: UInt = #line,
    _ condition: @MainActor () throws -> Bool
) async throws {
    let deadline = ContinuousClock.now + timeout
    while true {
        if try condition() {
            return
        }
        if ContinuousClock.now >= deadline {
            throw ScenarioFailure(description: "timed out after \(timeout) waiting for \(what) (\(file):\(line))")
        }
        try await Task.sleep(for: .milliseconds(10))
    }
}

/// Lets `milliseconds` pass while the main actor keeps running (the scenarios' "for 200 ms nothing
/// happens" windows).
@MainActor
func quietFor(milliseconds: Int) async throws {
    try await Task.sleep(for: .milliseconds(milliseconds))
}

// MARK: - Encoding helpers for raw calls

/// The encoded arguments of a raw call, written field by field.
func encoded(_ write: (inout KeelWriter) -> Void) -> [UInt8] {
    var writer = KeelWriter()
    write(&writer)
    return writer.finish()
}

/// Decodes a whole reply body as `Value`.
func decoded<Value: KeelCodec>(_ type: Value.Type, _ body: [UInt8]) throws -> Value {
    return try Value.keelDecoded(from: body)
}

extension KeelCore {
    /// A reading of one integer of the core's statistics document (`live_handles`,
    /// `crossings.calls`, ...): scenarios compare readings, since other scenarios share the core.
    func stat(_ key: String) -> Int {
        return stats().values[key] ?? 0
    }
}

// MARK: - Typed results

/// Runs a typed-throwing call and keeps its outcome as a `Result`, so a scenario can compare the
/// typed error the binding threw (`LabError`, `ListError`, ...) with what scenarios.md expects.
func outcome<Value, Failure: Error>(_ body: () throws(Failure) -> Value) -> Result<Value, Failure> {
    do {
        return .success(try body())
    } catch {
        return .failure(error)
    }
}

/// The asynchronous form of `outcome`.
func outcome<Value, Failure: Error>(_ body: () async throws(Failure) -> Value) async -> Result<Value, Failure> {
    do {
        return .success(try await body())
    } catch {
        return .failure(error)
    }
}

/// Fails the scenario unless `result` is the failure `expected`.
func checkFailure<Value, Failure: Error & Equatable>(
    _ result: Result<Value, Failure>,
    _ expected: Failure,
    _ what: @autoclosure () -> String,
    file: StaticString = #fileID,
    line: UInt = #line
) throws {
    switch result {
    case .failure(let error):
        try checkEqual(error, expected, what(), file: file, line: line)
    case .success(let value):
        throw ScenarioFailure(description: "\(what()): expected the typed error \(expected), got the value \(value) (\(file):\(line))")
    }
}

/// The value of a successful `result`, or a scenario failure naming the error.
func success<Value, Failure: Error>(
    _ result: Result<Value, Failure>,
    _ what: @autoclosure () -> String,
    file: StaticString = #fileID,
    line: UInt = #line
) throws -> Value {
    switch result {
    case .success(let value):
        return value
    case .failure(let error):
        throw ScenarioFailure(description: "\(what()): failed with \(error) (\(file):\(line))")
    }
}
