import XCTest
@testable import UndraTestKit
import UndraRuntime

/// The `Diagnostics` fake (ADR-046): a recorder of the core's panic reports, in the kit's bundle of fakes.
@MainActor
final class DiagnosticsFakeTests: XCTestCase {
    private let panicked = undraMethodId("Diagnostics", "panicked")

    private func report(_ operation: String, message: String = "boom") -> UndraPanicReport {
        return UndraPanicReport(
            message: message, location: "lab.rs:1:1", operation: operation, thread: "undra-core",
            frames: [UndraPanicFrame(address: 4, symbol: "f", file: "lab.rs", line: 1)],
            namespace: "playground_core", coreVersion: "1.0.0", schemaHash: 7, imageId: "ab"
        )
    }

    private func scratchCore() throws -> RecordedCore {
        return try RecordedCore.load(Recording(schemaHash: 1, source: "test"), expectedSchemaHash: 1)
    }

    private func call(_ impl: PortImpl?, _ method: UInt32, _ args: [UInt8]) throws -> [UInt8] {
        guard case .sync(let table)? = impl, let function = table[method] else {
            throw XCTSkip("no synchronous method \(method)")
        }
        return try function(args)
    }

    func testItIsTheDiagnosticsPortAndRecordsEveryReportInOrder() throws {
        let fake = CaptureDiagnostics()
        XCTAssertEqual(fake.portId, 0xab68_cd7c)
        XCTAssertEqual(panicked, 0xbd14_7e2e)
        XCTAssertEqual(fake.reports, [])
        let core = try scratchCore()
        defer { core.close() }
        let impl = fake.makePortImpl(core: core.core)
        XCTAssertEqual(try call(impl, panicked, report("a").undraEncoded()), [], "a sync port answers with an empty body")
        XCTAssertEqual(try call(impl, panicked, report("b", message: "later one").undraEncoded()), [])
        XCTAssertEqual(fake.reports.map { $0.operation }, ["a", "b"])
        XCTAssertEqual(fake.reports.first, report("a"))
        XCTAssertTrue(fake.contains("later one"))
        XCTAssertFalse(fake.contains("nothing like it"))
        fake.clear()
        XCTAssertEqual(fake.reports, [])
    }

    func testAMalformedReportIsDroppedNotThrown() throws {
        let fake = CaptureDiagnostics()
        let core = try scratchCore()
        defer { core.close() }
        let impl = fake.makePortImpl(core: core.core)
        XCTAssertEqual(try call(impl, panicked, [1, 2, 3]), [])
        XCTAssertEqual(fake.reports, [])
    }

    func testTheBundleHasOneAndRegistersItsPort() {
        let fakes = Fakes()
        XCTAssertEqual(fakes.diagnostics.reports, [])
        let ports = fakes.adapters().all.map { $0.portId }
        XCTAssertTrue(ports.contains(undraPortId("Diagnostics")))
        XCTAssertEqual(Set(ports).count, ports.count, "one adapter per port")
        XCTAssertEqual(undraStandardName(port: undraPortId("Diagnostics"), method: panicked), "Diagnostics.panicked")
    }
}
