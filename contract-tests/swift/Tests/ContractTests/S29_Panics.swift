import Foundation
import UndraRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    // MARK: S29

    /// ADR-046: the playground core's panics, seen by the app's crash reporter. `LoadOptions.onPanic` records each
    /// `UndraPanicReport` with a note of the thread it was delivered on (Fixture.panics).
    func testS29_panicReport() async {
        await scenario("S29", "panic report") {
            let core = try self.core
            let delivered = Fixture.shared.panics
            let before = delivered.snapshot.count
            let panicsBefore = core.stat("panics")
            let reportsBefore = core.stats().panicReports

            // 1. `explode("kaboom")` fails as in S17.1, and onPanic received exactly one report.
            let exploded = try callError("explode(\"kaboom\")") { try explode(reason: "kaboom", ctx: core) }
            guard case .panicked(let kaboom, _) = exploded else {
                throw ScenarioFailure(description: "explode(\"kaboom\") failed with \(exploded), not .panicked")
            }
            try check(kaboom.contains("kaboom"), "the panic message names the reason: \(kaboom)")
            try await waitUntil("onPanic to receive the report of explode") { delivered.snapshot.count >= before + 1 }
            try await quietFor(milliseconds: 200)
            try checkEqual(delivered.snapshot.count - before, 1, "reports after explode(\"kaboom\")")
            let report = delivered.snapshot[before].report
            try check(report.message.contains("kaboom"), "the report's message names the reason: \(report.message)")
            try check(report.location.contains("lab.rs:"), "the location names lab.rs: \(report.location)")
            let parts = report.location.split(separator: ":")
            try check(
                parts.count >= 3 && UInt32(parts[parts.count - 1]) != nil && UInt32(parts[parts.count - 2]) != nil,
                "the location ends in :<line>:<column>: \(report.location)"
            )
            try checkEqual(report.operation, "explode", "operation")
            try check(!report.thread.isEmpty, "the thread of the panic is empty")
            try checkEqual(report.namespace, "playground_core", "namespace")
            try check(!report.coreVersion.isEmpty, "the core version is empty")
            try checkEqual(report.schemaHash, UndraIds.schemaHash, "the report's schema hash")
            try check(
                report.imageId.allSatisfy { "0123456789abcdef".contains($0) },
                "the image id is not lowercase hex: \(report.imageId)"
            )
            try check(!report.frames.isEmpty, "the report has no frames")
            try check(report.frames.contains { $0.symbol != nil }, "no frame is named (the runner loads a debug build): \(report.frames.prefix(3))")
            // Every frame's address is a `UInt64`: the type says so; a frame the core could not place carries 0.

            // 2. A panic in an async call, and in a detached task, report once each.
            let later = try await callError("explode_later(10, \"later\")") {
                try await explodeLater(delayMs: 10, reason: "later", ctx: core)
            }
            guard case .panicked(let laterMessage, _) = later, laterMessage.contains("later") else {
                throw ScenarioFailure(description: "explode_later failed with \(later), not .panicked(\"later\")")
            }
            explodeDetached(reason: "task", ctx: core)
            try await waitUntil("onPanic to receive the reports of explode_later and the detached task") {
                delivered.snapshot.count >= before + 3
            }
            try await quietFor(milliseconds: 200)
            try checkEqual(delivered.snapshot.count - before, 3, "reports after the three panics")

            // 3. On the main thread, once each, in the order the panics happened.
            let mine = Array(delivered.snapshot.dropFirst(before))
            try checkEqual(mine.map { $0.report.operation }, ["explode", "explode_later", "task"], "operations, in order")
            try check(mine[1].report.message.contains("later"), "explode_later's message: \(mine[1].report.message)")
            try check(mine[2].report.message.contains("task"), "the detached task's message: \(mine[2].report.message)")
            try check(mine.allSatisfy { $0.onMainThread }, "a report was delivered off the main thread: \(mine.map { $0.onMainThread })")

            // 4. The statistics count them, and the core keeps working.
            try checkEqual(core.stat("panics") - panicsBefore, 3, "panics delta")
            try checkEqual(core.stats().panicReports - reportsBefore, 3, "panicReports delta")
            try checkEqual(try PlaygroundCore.add(a: 1, b: 2, ctx: core), 3, "add(1, 2) after the panics")
            // The handler cannot throw in Swift (`@Sendable (UndraPanicReport) -> Void`), so the "reporter that throws" step is
            // Kotlin and TypeScript's, as scenarios.md says.
        }
    }
}
