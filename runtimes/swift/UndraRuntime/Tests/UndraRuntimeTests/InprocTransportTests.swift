import Foundation
import UndraFFI
import XCTest
@testable import UndraRuntime

/// Loading an in-process core through its table (`UndraApi`, ADR-044), and the transport over it.
///
/// These run `UndraCore.load` over a `FakeCoreTable`: a real table whose entries are Swift
/// `@convention(c)` closures that record what the runtime calls. The table is checked before
/// anything in it runs (its ABI version first, then its size, namespace and entries, then the
/// schema hash, docs/SPEC.md section 11), one core of a namespace runs at a time, and cores of
/// different namespaces run side by side. The real core is exercised by
/// `crates/undra-ffi/tests/swift` and the contract runner (`contract-tests/swift`).
final class InprocTransportTests: XCTestCase {
    private let bindingsHash: UInt64 = 0xA11C_E5ED_0000_0001
    private let coreHash: UInt64 = 0xA11C_E5ED_0000_0002

    override func setUp() {
        super.setUp()
        FakeCore.reset()
    }

    private func load(_ table: FakeCoreTable, expected: UInt64) throws -> UndraCore {
        let core = try UndraCore.load(.inproc(api: table.pointer, adapters: Adapters.none, expectedSchemaHash: expected))
        addTeardownBlock { core.shutdown() }
        return core
    }

    // MARK: The checks before init

    func testAMatchingTableInitialisesTheCoreOnceAfterTheChecks() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        let core = try load(table, expected: coreHash)
        XCTAssertEqual(FakeCore.calls, ["init"], "the version, size, namespace and hash are data: only init is called")
        XCTAssertEqual(core.schemaHash, coreHash)
        XCTAssertEqual(core.mode, .inproc)
        let config = try RuntimeConfigRecord.undraDecoded(from: try XCTUnwrap(FakeCore.script.withLock { $0.configs.first }))
        XCTAssertEqual(config.mode, "inproc")
        XCTAssertTrue(InprocTransport.isClaimed("fake_core"))
        XCTAssertTrue(UndraCore.current === core, "the first core loaded is the shared one")
    }

    func testAWrongSchemaHashIsRefusedBeforeTheCoreIsInitialised() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        XCTAssertThrowsError(try load(table, expected: bindingsHash)) { error in
            XCTAssertEqual(error as? UndraSchemaMismatchError, UndraSchemaMismatchError(expected: bindingsHash, got: coreHash))
        }
        XCTAssertFalse(FakeCore.initialised, "init must not run for bindings of another schema")
        XCTAssertFalse(InprocTransport.isClaimed("fake_core"))
        XCTAssertNil(UndraCore.current)
    }

    func testATableOfAnotherABIVersionIsRefusedBeforeAnythingElseInItIsRead() throws {
        for version: UInt32 in [0, 1, 3] {
            // Everything after the version is zero: a runtime that read on would report a size,
            // namespace or null entry instead of the version.
            let table = FakeCoreTable(namespace: nil, schemaHash: 0, abiVersion: version, size: 0)
            for entry in FakeCoreTable.entryNames {
                table.nullEntry(entry)
            }
            XCTAssertThrowsError(try load(table, expected: coreHash), "version \(version)") { error in
                XCTAssertEqual(error as? UndraLoadError, UndraLoadError.abiMismatch(expected: 2, got: version))
            }
        }
        XCTAssertEqual(UndraCore.abiVersion, 2)
        XCTAssertEqual(FakeCore.calls, [])
    }

    func testATableSmallerThanAVersion2TableIsRefused() throws {
        let table = FakeCoreTable(schemaHash: coreHash, size: UInt32(MemoryLayout<UndraApi>.size - 8))
        XCTAssertThrowsError(try load(table, expected: coreHash)) { error in
            guard case UndraLoadError.invalidCoreTable(let reason)? = error as? UndraLoadError else {
                return XCTFail("\(error)")
            }
            XCTAssertTrue(reason.contains("bytes"), reason)
        }
        XCTAssertEqual(FakeCore.calls, [])
        // A larger table (a later version that appended fields) is fine.
        let larger = FakeCoreTable(schemaHash: coreHash, size: UInt32(MemoryLayout<UndraApi>.size + 16))
        _ = try load(larger, expected: coreHash)
    }

    func testATableWithANullEntryIsRefusedBeforeInit() throws {
        for entry in FakeCoreTable.entryNames {
            let table = FakeCoreTable(schemaHash: coreHash)
            table.nullEntry(entry)
            XCTAssertThrowsError(try load(table, expected: coreHash), entry) { error in
                guard case UndraLoadError.invalidCoreTable(let reason)? = error as? UndraLoadError else {
                    return XCTFail("\(entry): \(error)")
                }
                XCTAssertTrue(reason.contains("`\(entry)`") && reason.contains("fake_core"), reason)
            }
        }
        XCTAssertEqual(FakeCore.calls, [])
        XCTAssertFalse(InprocTransport.isClaimed("fake_core"))
    }

    func testATableWithoutANamespaceIsRefused() throws {
        for namespace: String? in [nil, ""] {
            let table = FakeCoreTable(namespace: namespace, schemaHash: coreHash)
            XCTAssertThrowsError(try load(table, expected: coreHash)) { error in
                guard case UndraLoadError.invalidCoreTable(let reason)? = error as? UndraLoadError else {
                    return XCTFail("\(error)")
                }
                XCTAssertTrue(reason.contains("namespace"), reason)
            }
        }
        XCTAssertEqual(FakeCore.calls, [])
    }

    func testALoadWithoutATableOrAHashIsATypedError() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        XCTAssertThrowsError(try UndraCore.load(.inproc(adapters: Adapters.none, expectedSchemaHash: coreHash))) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.missingCoreTable)
        }
        XCTAssertThrowsError(try UndraCore.load(.inproc(api: table.pointer, adapters: Adapters.none))) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.missingSchemaHash)
        }
        XCTAssertThrowsError(try UndraCore.load(.remote(url: "ws://127.0.0.1:1", adapters: Adapters.none))) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.missingSchemaHash)
        }
        XCTAssertTrue("\(UndraLoadError.missingCoreTable)".contains("Undra<Namespace>.load()"))
        XCTAssertTrue("\(UndraLoadError.missingSchemaHash)".contains("Undra<Namespace>.load()"))
        XCTAssertEqual(FakeCore.calls, [])
        XCTAssertNil(UndraCore.current)
    }

    func testTheLoadErrorsNameNoStub() {
        let errors: [UndraLoadError] = [
            .alreadyLoaded, .abiMismatch(expected: 2, got: 0), .abiMismatch(expected: 2, got: 1),
            .invalidCoreTable("x"), .missingCoreTable, .missingSchemaHash, .coreInitFailed(code: 1),
        ]
        for error in errors {
            let text = "\(error)"
            XCTAssertFalse(text.contains("stub") || text.contains("UNDRA_LINK_CORE") || text.contains("undra_init"), text)
        }
    }

    // MARK: Claims per namespace

    func testASecondLoadOfALoadedNamespaceIsRefusedAfterTheHashCheck() throws {
        let first = FakeCoreTable(schemaHash: coreHash)
        _ = try load(first, expected: coreHash)

        let second = FakeCoreTable(schemaHash: coreHash)
        XCTAssertThrowsError(try load(second, expected: bindingsHash), "the wrong hash is the more useful answer") { error in
            XCTAssertEqual(error as? UndraSchemaMismatchError, UndraSchemaMismatchError(expected: bindingsHash, got: coreHash))
        }
        XCTAssertThrowsError(try load(second, expected: coreHash)) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.alreadyLoaded)
        }
        XCTAssertThrowsError(try load(first, expected: coreHash), "the same table twice") { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.alreadyLoaded)
        }
        XCTAssertEqual(FakeCore.calls, ["init"], "a refused load never reaches init")
    }

    func testCoresOfDifferentNamespacesRunSideBySide() throws {
        let tableA = FakeCoreTable(namespace: "fake_a", schemaHash: coreHash)
        let tableB = FakeCoreTable(namespace: "fake_b", schemaHash: bindingsHash)
        let coreA = try load(tableA, expected: coreHash)
        let coreB = try load(tableB, expected: bindingsHash)
        XCTAssertFalse(coreA === coreB)
        XCTAssertTrue(InprocTransport.isClaimed("fake_a") && InprocTransport.isClaimed("fake_b"))
        XCTAssertTrue(UndraCore.current === coreA, "shared stays the first core loaded")

        coreA.shutdown()
        XCTAssertFalse(InprocTransport.isClaimed("fake_a"))
        XCTAssertTrue(InprocTransport.isClaimed("fake_b"))
        XCTAssertEqual(try coreB.callSync(.freeFunction(methodId: 7), method: 7, args: [4, 2]), [4, 2], "the other core keeps working")
        XCTAssertNil(UndraCore.current, "shared is not handed to a later core")

        let again = try load(tableA, expected: coreHash)
        XCTAssertFalse(again.isShutDown)
        XCTAssertEqual(FakeCore.calls, ["init", "init", "shutdown", "call_sync", "init"])
    }

    func testARefusedInitIsACoreInitFailedAndReleasesTheClaim() throws {
        FakeCore.reset(initCode: 7)
        let table = FakeCoreTable(schemaHash: coreHash)
        XCTAssertThrowsError(try load(table, expected: coreHash)) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.coreInitFailed(code: 7))
        }
        XCTAssertFalse(InprocTransport.isClaimed("fake_core"))
        XCTAssertNil(UndraCore.current)
        FakeCore.script.withLock { $0.initCode = 0 }
        _ = try load(table, expected: coreHash)
        XCTAssertEqual(FakeCore.calls, ["init", "init"], "a failed init is not shut down")
    }

    func testShutdownCallsTheTableOnceAndFreesTheNamespace() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        let core = try load(table, expected: coreHash)
        core.shutdown()
        core.shutdown()
        XCTAssertEqual(FakeCore.calls, ["init", "shutdown"])
        XCTAssertFalse(InprocTransport.isClaimed("fake_core"))
        XCTAssertNil(UndraCore.current)
        XCTAssertThrowsError(try core.callSync(.freeFunction(methodId: 7), method: 7, args: []))
        XCTAssertEqual(FakeCore.calls, ["init", "shutdown"], "a closed core calls nothing")
        _ = try load(table, expected: coreHash)
    }

    // MARK: Calls through the table

    @MainActor
    func testEveryOperationGoesThroughTheTableAndItsBuffersAreFreedByIt() async throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        let core = try load(table, expected: coreHash)

        XCTAssertEqual(try core.callSync(.freeFunction(methodId: 7), method: 7, args: [1, 2, 3]), [1, 2, 3])
        let replied = try await core.call(.freeFunction(methodId: 8), method: 8, args: [9])
        XCTAssertEqual(replied, [9], "a reply delivered inside `call` reaches the caller through the reply callback")
        let handle = UndraHandle(index: 5, generation: 1)
        core.observe(handle, signal: 3, on: true)
        core.release(handle)
        core.event(port: 11, method: 12, payload: [1, 2])
        core.timerFired(13)
        XCTAssertEqual(try core.snapshot(), [1, 2, 3])
        try core.restore([4, 5])
        FakeCore.script.withLock { $0.restoreCode = 6 }
        XCTAssertThrowsError(try core.restore([4])) { error in
            XCTAssertEqual((error as? UndraRestoreError)?.code, 6)
        }
        XCTAssertEqual(core.stats().coreLiveHandles, 3)

        let calls = FakeCore.calls
        for expected in ["init", "call_sync", "call", "observe \(handle.rawValue) 3 1", "release \(handle.rawValue)", "event 11 12 2", "timer_fired 13", "snapshot", "restore 2", "restore 1", "stats_json"] {
            XCTAssertTrue(calls.contains(expected), "\(expected) in \(calls)")
        }
        let counts = FakeCore.script.withLock { ($0.allocated, $0.freed) }
        XCTAssertEqual(counts.0, 3, "call_sync, snapshot and stats_json return core buffers")
        XCTAssertEqual(counts.1, counts.0, "every core buffer goes back to the table's buf_free")
    }

    func testAPortCallbackRegisteredThroughTheTableAnswersWithMallocdMemory() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        let core = try load(table, expected: coreHash)
        core.registerPort(0x0B0B, .sync([0x77: { args in args + [42] }]))
        XCTAssertTrue(FakeCore.calls.contains("port_register \(0x0B0B)"))

        let answer = try XCTUnwrap(FakeCore.callPort(0x0B0B, method: 0x77, portCallId: 5, args: [1]))
        XCTAssertEqual(answer.code, 0, "a synchronous port answers inline")
        XCTAssertEqual(try Wire.PortReply.decode(answer.reply), Wire.PortReply(portCallId: 5, status: .ok, body: [1, 42]))

        core.shutdown()
        XCTAssertNil(FakeCore.callPort(0x0B0B, method: 0x77, portCallId: 6, args: []), "the core dropped the registration at shutdown")
    }
}
