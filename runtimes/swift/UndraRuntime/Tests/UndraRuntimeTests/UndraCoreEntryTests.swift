import Foundation
import XCTest
@testable import UndraRuntime

/// `UndraCoreEntry`, what the generated `Undra<Namespace>` of a core delegates to (ADR-044): it
/// fills in the core's table and the bindings' schema hash, and its `core` is the loaded core while
/// it is open and the closed placeholder otherwise.
final class UndraCoreEntryTests: XCTestCase {
    private let coreHash: UInt64 = 0xE7E7_0000_0000_0001

    /// Counts the calls of an entry's `api` closure.
    private final class Lookups: @unchecked Sendable {
        private let count = Guarded<Int>(0)
        func note() {
            count.withLock { $0 += 1 }
        }
        var value: Int {
            return count.withLock { $0 }
        }
    }

    /// An entry over `table`, as generated code makes one.
    private func entry(_ table: FakeCoreTable, namespace: String = "fake_core", schemaHash: UInt64? = nil, lookups: Lookups = Lookups()) -> UndraCoreEntry {
        let address = UInt(bitPattern: table.pointer)
        let entry = UndraCoreEntry(namespace: namespace, schemaHash: schemaHash ?? coreHash, api: {
            lookups.note()
            return UnsafeRawPointer(bitPattern: address)
        })
        addTeardownBlock { entry.core.shutdown() }
        return entry
    }

    override func setUp() {
        super.setUp()
        FakeCore.reset()
    }

    func testTheEntryFillsInTheTableAndTheSchemaHash() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        let lookups = Lookups()
        let entry = entry(table, lookups: lookups)
        XCTAssertEqual(entry.namespace, "fake_core")
        XCTAssertEqual(entry.schemaHash, coreHash)

        let core = try entry.load(.inproc(adapters: Adapters.none))
        XCTAssertEqual(lookups.value, 1, "the table is looked up by an in-process load")
        XCTAssertEqual(core.schemaHash, coreHash)
        XCTAssertEqual(FakeCore.calls, ["init"])
        XCTAssertTrue(entry.core === core)
        XCTAssertTrue(UndraCore.current === core)
    }

    func testWhatTheOptionsSayWins() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        let lookups = Lookups()
        let entry = entry(table, lookups: lookups)
        XCTAssertThrowsError(try entry.load(.inproc(adapters: Adapters.none, expectedSchemaHash: coreHash + 1))) { error in
            XCTAssertEqual(error as? UndraSchemaMismatchError, UndraSchemaMismatchError(expected: coreHash + 1, got: coreHash))
        }
        XCTAssertFalse(FakeCore.initialised)

        let other = FakeCoreTable(namespace: "fake_other", schemaHash: coreHash)
        let core = try entry.load(.inproc(api: other.pointer, adapters: Adapters.none))
        XCTAssertEqual(lookups.value, 1, "an explicit table is used instead of looking one up")
        XCTAssertTrue(InprocTransport.isClaimed("fake_other"))
        XCTAssertFalse(InprocTransport.isClaimed("fake_core"))
        XCTAssertTrue(entry.core === core)
    }

    func testCoreIsTheClosedPlaceholderUntilLoadedAndAfterShutdown() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        let entry = entry(table)
        XCTAssertTrue(entry.core === UndraCore.unloaded)
        XCTAssertTrue(entry.core.isShutDown)
        XCTAssertThrowsError(try entry.core.callSync(.freeFunction(methodId: 1), method: 1, args: [])) { error in
            XCTAssertEqual(UndraCallError.mapped(error) as? UndraCallError, .unavailable(.closed))
        }

        let core = try entry.load(.inproc(adapters: Adapters.none))
        XCTAssertTrue(entry.core === core)
        core.shutdown()
        XCTAssertTrue(entry.core === UndraCore.unloaded, "a shut-down core is not handed out")
        XCTAssertTrue(UndraCore.shared === entry.core, "the same placeholder as UndraCore.shared")

        let again = try entry.load(.inproc(adapters: Adapters.none))
        XCTAssertFalse(again === core)
        XCTAssertTrue(entry.core === again, "the entry loads again after shutdown")
        XCTAssertEqual(FakeCore.calls, ["init", "shutdown", "init"])
    }

    func testASecondLoadWhileTheCoreIsOpenIsRefused() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        let entry = entry(table)
        let core = try entry.load(.inproc(adapters: Adapters.none))
        XCTAssertThrowsError(try entry.load(.inproc(adapters: Adapters.none))) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.alreadyLoaded)
        }
        XCTAssertThrowsError(try entry.load(.remote(url: "ws://127.0.0.1:1", adapters: Adapters.none)), "nor a remote one in its place") { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.alreadyLoaded)
        }
        XCTAssertTrue(entry.core === core, "the open core stays the entry's")
        XCTAssertEqual(FakeCore.calls, ["init"])
    }

    func testAFailedLoadLeavesThePlaceholderAndNeverAsksARemoteLoadForTheTable() throws {
        let table = FakeCoreTable(schemaHash: coreHash)
        let lookups = Lookups()
        let entry = entry(table, lookups: lookups)
        XCTAssertThrowsError(try entry.load(.remote(url: "not a url", adapters: Adapters.none))) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.invalidURL("not a url"))
        }
        XCTAssertEqual(lookups.value, 0, "a remote core needs no table")
        XCTAssertTrue(entry.core === UndraCore.unloaded)

        FakeCore.reset(initCode: 3)
        XCTAssertThrowsError(try entry.load(.inproc(adapters: Adapters.none))) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.coreInitFailed(code: 3))
        }
        XCTAssertTrue(entry.core === UndraCore.unloaded)
        FakeCore.script.withLock { $0.initCode = 0 }
        _ = try entry.load(.inproc(adapters: Adapters.none))
        XCTAssertFalse(entry.core.isShutDown, "a failed load does not block the next one")
    }

    func testAnEntryWhoseCoreIsNotLinkedFailsTyped() {
        let entry = UndraCoreEntry(namespace: "not_linked", schemaHash: coreHash, api: { nil })
        XCTAssertThrowsError(try entry.load(.inproc(adapters: Adapters.none))) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.missingCoreTable)
        }
        XCTAssertTrue(entry.core === UndraCore.unloaded)
    }

    func testTwoEntriesHoldTwoCores() throws {
        let tableA = FakeCoreTable(namespace: "fake_a", schemaHash: coreHash)
        let tableB = FakeCoreTable(namespace: "fake_b", schemaHash: coreHash + 1)
        let entryA = entry(tableA, namespace: "fake_a")
        let entryB = entry(tableB, namespace: "fake_b", schemaHash: coreHash + 1)
        let coreA = try entryA.load(.inproc(adapters: Adapters.none))
        let coreB = try entryB.load(.inproc(adapters: Adapters.none))
        XCTAssertTrue(entryA.core === coreA && entryB.core === coreB)
        XCTAssertEqual(entryB.core.schemaHash, coreHash + 1)
        XCTAssertTrue(UndraCore.shared === coreA)

        coreA.shutdown()
        XCTAssertTrue(entryA.core === UndraCore.unloaded)
        XCTAssertTrue(entryB.core === coreB, "closing one core leaves the other's entry alone")
        XCTAssertEqual(try entryB.core.callSync(.freeFunction(methodId: 2), method: 2, args: [7]), [7])
    }
}
