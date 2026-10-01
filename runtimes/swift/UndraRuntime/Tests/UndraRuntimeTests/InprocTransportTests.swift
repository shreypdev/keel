import Foundation
import XCTest
@testable import UndraRuntime

/// The order in which the in-process transport uses the core (docs/SPEC.md section 11: the schema
/// hash is checked at attach, before the core is initialised).
///
/// The link-time stub of the C ABI cannot take part (it reports ABI version 0 and refuses
/// `undra_init`), so these run `InprocTransport` over a scripted `CoreEntry` that records what
/// `start` calls and in which order. The real core is exercised by the contract runner
/// (`contract-tests/swift`, scenario S16).
final class InprocTransportTests: XCTestCase {
    private let bindingsHash: UInt64 = 0xA11C_E5ED_0000_0001
    private let coreHash: UInt64 = 0xA11C_E5ED_0000_0002

    /// What the transport asked of the scripted core, in order.
    private final class Script: @unchecked Sendable {
        private let lock = NSLock()
        private var calls: [String] = []
        private var configs: [[UInt8]] = []
        var abiVersion: UInt32 = UndraCore.abiVersion
        var schemaHash: UInt64 = 0
        var initCode: UInt32 = 0

        func record(_ name: String) {
            lock.lock()
            defer { lock.unlock() }
            calls.append(name)
        }

        func recordConfig(_ bytes: [UInt8]) {
            lock.lock()
            defer { lock.unlock() }
            configs.append(bytes)
        }

        var history: [String] {
            lock.lock()
            defer { lock.unlock() }
            return calls
        }

        var initConfigs: [[UInt8]] {
            lock.lock()
            defer { lock.unlock() }
            return configs
        }

        var entry: InprocTransport.CoreEntry {
            return InprocTransport.CoreEntry(
                abiVersion: {
                    self.record("undra_abi_version")
                    return self.abiVersion
                },
                schemaHash: {
                    self.record("undra_schema_hash")
                    return self.schemaHash
                },
                initialize: { config, _ in
                    self.record("undra_init")
                    self.recordConfig(config)
                    return self.initCode
                }
            )
        }
    }

    private func connect(_ script: Script, expected: UInt64) throws -> (UndraCore, InprocTransport) {
        let transport = InprocTransport(entry: script.entry)
        let options = LoadOptions.inproc(adapters: Adapters.none, expectedSchemaHash: expected)
        let core = try UndraCore.connect(transport: transport, options: options)
        return (core, transport)
    }

    func testAWrongSchemaHashIsRefusedBeforeTheCoreIsInitialised() throws {
        let script = Script()
        script.schemaHash = coreHash
        XCTAssertThrowsError(try connect(script, expected: bindingsHash)) { error in
            XCTAssertEqual(error as? UndraSchemaMismatchError, UndraSchemaMismatchError(expected: bindingsHash, got: coreHash))
        }
        XCTAssertEqual(script.history, ["undra_abi_version", "undra_schema_hash"], "undra_init must not run for bindings of another schema")
    }

    func testARefusedLoadLeavesTheProcessFreeToLoadAgain() throws {
        let script = Script()
        script.schemaHash = coreHash
        XCTAssertThrowsError(try connect(script, expected: bindingsHash))
        let (core, _) = try connect(script, expected: coreHash)
        XCTAssertEqual(core.schemaHash, coreHash)
        core.shutdown()
    }

    func testAMatchingSchemaHashInitialisesTheCoreOnceAfterTheChecks() throws {
        let script = Script()
        script.schemaHash = coreHash
        let (core, _) = try connect(script, expected: coreHash)
        defer { core.shutdown() }
        XCTAssertEqual(script.history, ["undra_abi_version", "undra_schema_hash", "undra_init"])
        XCTAssertEqual(core.schemaHash, coreHash)
        let config = try RuntimeConfigRecord.undraDecoded(from: try XCTUnwrap(script.initConfigs.first))
        XCTAssertEqual(config.mode, "inproc")
    }

    func testAnABIMismatchIsReportedBeforeTheSchemaHashIsRead() throws {
        let script = Script()
        script.abiVersion = UndraCore.abiVersion + 1
        script.schemaHash = coreHash
        XCTAssertThrowsError(try connect(script, expected: bindingsHash)) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.abiMismatch(expected: UndraCore.abiVersion, got: UndraCore.abiVersion + 1))
        }
        XCTAssertEqual(script.history, ["undra_abi_version"])
    }

    func testAMismatchIsReportedEvenWhileAnotherCoreIsLoaded() throws {
        let first = Script()
        first.schemaHash = coreHash
        let (loaded, _) = try connect(first, expected: coreHash)
        defer { loaded.shutdown() }

        let second = Script()
        second.schemaHash = coreHash
        XCTAssertThrowsError(try connect(second, expected: bindingsHash), "the wrong hash is the more useful answer") { error in
            XCTAssertEqual(error as? UndraSchemaMismatchError, UndraSchemaMismatchError(expected: bindingsHash, got: coreHash))
        }
        XCTAssertThrowsError(try connect(second, expected: coreHash)) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.alreadyLoaded)
        }
        XCTAssertFalse(second.history.contains("undra_init"))
    }

    func testARefusedUndraInitIsACoreInitFailedAndReleasesTheClaim() throws {
        let script = Script()
        script.schemaHash = coreHash
        script.initCode = 7
        XCTAssertThrowsError(try connect(script, expected: coreHash)) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.coreInitFailed(code: 7))
        }
        script.initCode = 0
        let (core, _) = try connect(script, expected: coreHash)
        core.shutdown()
    }
}
