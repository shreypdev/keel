import KeelFFI
@testable import KeelRuntime
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
    /// The generated `Probe.ticks(count:)` copies the runtime's pull-based stream into an
    /// unbounded `AsyncThrowingStream` from a task of its own (`keelDecodeStream` in
    /// `Objects.swift`), so nothing the consumer does limits how far the core runs ahead: after
    /// reading 5 of 1,000 items and waiting 200 ms, the core has produced all 1,000.
    func testFinding_generatedTicksIgnoreBackpressure() async throws {
        let core = try Fixture.shared.core()
        let probe = try Probe(ctx: core)
        defer { probe.close() }
        probe.reset()
        var iterator = probe.ticks(count: 1000).makeAsyncIterator()
        for _ in 0 ..< 5 {
            _ = try await iterator.next()
        }
        try await Task.sleep(for: .milliseconds(200))
        let produced = probe.counters().produced
        XCTExpectFailure("keel-bindgen: generated stream methods buffer without bound")
        XCTAssertLessThanOrEqual(produced, 5 + 64, "the core ran \(produced) items ahead of a consumer that read 5")
    }

    /// `InprocTransport.start` calls `keel_init` and only then reads `keel_schema_hash()`, so a load
    /// whose bindings were generated for another schema initialises the core first and tears it
    /// down again. scenarios.md S16.1 (and SPEC 11) want the check before the core is initialised.
    /// The observable difference: with the core already initialised by another embedder, `keel_init`
    /// is refused, so the load fails with `coreInitFailed` and never gets to report the mismatch.
    func testFinding_loadInitialisesTheCoreBeforeComparingSchemaHashes() throws {
        Fixture.shared.shutDown()
        let config = RuntimeConfigRecord(platform: "macos", mode: "inproc", coreThreads: 1, blockingThreads: 0, logLevel: 2).keelEncoded()
        let other = config.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return keel_init(bytes.baseAddress, UInt32(bytes.count), { _, _, _, _ in }, { _, _, _ in }, { _, _, _, _ in }, nil)
        }
        XCTAssertEqual(other, 0, "another embedder initialises the core")
        defer { keel_shutdown() }
        var thrown: (any Error)?
        do {
            _ = try KeelCore.load(.inproc(adapters: .none, expectedSchemaHash: KeelIds.schemaHash ^ 1))
        } catch {
            thrown = error
        }
        XCTExpectFailure("KeelRuntime: the in-process transport initialises the core before it compares schema hashes")
        XCTAssertTrue(thrown is KeelSchemaMismatchError, "the load failed with \(String(describing: thrown)), not with the schema mismatch")
    }
}
