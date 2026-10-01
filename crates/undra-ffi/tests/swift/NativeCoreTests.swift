// The real C ABI (version 2: one `UndraApi` table per core, ADR-044) driven by the real Swift
// runtime (UndraCore over InprocTransport), against the fixture core
// (crates/undra-ffi/tests/fixture, namespace `undra_fixture`). It is not part of the Swift package:
// run.sh builds a scratch package that depends on the runtime, declares the fixture's
// `undra_fixture_undra_api` in the C module `UndraFixtureCoreFFI` (what bindgen generates for a
// core) and links the fixture, so the Swift tree is untouched.

import Foundation
import UndraFFI
import UndraFixtureCoreFFI
import XCTest

@testable import UndraRuntime

private func fnv(_ text: String) -> UInt32 {
    var hash: UInt32 = 0x811c_9dc5
    for byte in text.utf8 {
        hash ^= UInt32(byte)
        hash = hash &* 0x0100_0193
    }
    return hash
}

private func portId(_ trait: String) -> UInt32 { fnv("port.\(trait)") }
private func portMethod(_ trait: String, _ name: String) -> UInt32 { fnv("\(trait).\(name)") }
private func mid(_ type: String, _ name: String) -> UInt32 { fnv("\(type).\(name)") }

private func i64(_ value: Int64) -> [UInt8] {
    var w = UndraWriter()
    w.writeI64(value)
    return w.finish()
}

private func u32(_ value: UInt32) -> [UInt8] {
    var w = UndraWriter()
    w.writeU32(value)
    return w.finish()
}

private func readU32(_ bytes: [UInt8], at offset: Int = 0) -> UInt32 {
    var value: UInt32 = 0
    for i in 0 ..< 4 { value |= UInt32(bytes[offset + i]) << UInt32(8 * i) }
    return value
}

private func readI64(_ bytes: [UInt8]) throws -> Int64 {
    var r = UndraReader(bytes)
    return try r.readI64()
}

/// What `undra_fixture_undra_api()` returns: the fixture core's table.
private var fixtureTable: UnsafeRawPointer {
    return undra_fixture_undra_api()
}

/// The fixture core's table, copied.
private var fixtureApi: UndraApi {
    return fixtureTable.load(as: UndraApi.self)
}

/// The schema hash the fixture was built from, read from its table.
private var fixtureHash: UInt64 {
    return fixtureApi.schema_hash
}

/// The version call every check below makes: proves the core behind a load is running.
private func coreVersion(_ core: UndraCore) throws -> String {
    let bytes = try core.callSync(.freeFunction(methodId: fnv("fn.version")), method: fnv("fn.version"), args: [])
    var reader = UndraReader(bytes)
    return try reader.readString()
}

/// A copy of the fixture's table (same namespace, same entries) whose `init` counts its calls before
/// it forwards to the fixture's own, and whose header fields a test may change: what proves a
/// refused load never reached `init` of the real core.
private final class CountingTable {
    static let initCalls = Guarded<Int>(0)

    let api: UnsafeMutablePointer<UndraApi>

    var pointer: UnsafeRawPointer {
        return UnsafeRawPointer(api)
    }

    init(abiVersion: UInt32 = UndraCore.abiVersion) {
        var table = fixtureApi
        table.abi_version = abiVersion
        table.`init` = { cfg, len, reply, changes, stream, user in
            CountingTable.initCalls.withLock { $0 += 1 }
            let real = undra_fixture_undra_api().load(as: UndraApi.self).`init`!
            return real(cfg, len, reply, changes, stream, user)
        }
        api = UnsafeMutablePointer<UndraApi>.allocate(capacity: 1)
        api.initialize(to: table)
        CountingTable.initCalls.withLock { $0 = 0 }
    }

    var inits: Int {
        return CountingTable.initCalls.withLock { $0 }
    }

    deinit {
        api.deinitialize(count: 1)
        api.deallocate()
    }
}

private final class Records: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [(UInt8, String, String)] = []
    func add(_ level: UInt8, _ target: String, _ message: String) {
        lock.lock(); items.append((level, target, message)); lock.unlock()
    }
    var all: [(UInt8, String, String)] {
        lock.lock(); defer { lock.unlock() }
        return items
    }
    func clear() {
        lock.lock(); items.removeAll(); lock.unlock()
    }
}

final class NativeCoreTests: XCTestCase {
    private func adapters(_ records: Records) -> Adapters {
        let sum: PortImpl = .sync([
            portMethod("Sum", "add"): { args in u32(readU32(args, at: 0) + readU32(args, at: 4)) },
        ])
        let echo: PortImpl = .async([
            portMethod("Echo", "ping"): { args in
                try await Task.sleep(nanoseconds: 5_000_000)
                return u32(readU32(args) + 1000)
            },
        ])
        let log: PortImpl = .sync([
            portMethod("Log", "log"): { args in
                var r = UndraReader(args)
                records.add(try r.readU8(), try r.readString(), try r.readString())
                return []
            },
        ])
        return Adapters.none
            .replacing(portId: portId("Sum"), with: sum)
            .replacing(portId: portId("Echo"), with: echo)
            .replacing(portId: portId("Log"), with: log)
    }

    /// Loads the fixture through `table` and shuts it down when the test ends.
    private func load(_ table: UnsafeRawPointer, expected: UInt64? = nil) throws -> UndraCore {
        let core = try UndraCore.load(.inproc(api: table, adapters: Adapters.none, expectedSchemaHash: expected ?? fixtureHash))
        addTeardownBlock { core.shutdown() }
        return core
    }

    // MARK: The table

    func testTheFixtureExportsAVersion2TableNamedAfterItsNamespace() throws {
        let api = fixtureApi
        XCTAssertEqual(api.abi_version, 2)
        XCTAssertEqual(api.abi_version, UndraCore.abiVersion)
        XCTAssertEqual(Int(api.size), MemoryLayout<UndraApi>.size, "the header's UndraApi and the core's agree in size")
        XCTAssertNotEqual(api.schema_hash, 0)
        XCTAssertEqual(api.name_space.map { String(cString: $0) }, "undra_fixture")
        XCTAssertTrue(undra_fixture_undra_api() == fixtureTable, "one immutable table per core")
        let table = try CoreTable(reading: fixtureTable)
        XCTAssertEqual(table.namespace, "undra_fixture")
        XCTAssertEqual(table.schemaHash, api.schema_hash)
    }

    // MARK: Refusals before init

    func testATableOfAnotherVersionIsRefusedBeforeInit() throws {
        for version: UInt32 in [1, 3] {
            let table = CountingTable(abiVersion: version)
            XCTAssertThrowsError(try load(table.pointer), "version \(version)") { error in
                XCTAssertEqual(error as? UndraLoadError, UndraLoadError.abiMismatch(expected: 2, got: version))
            }
            XCTAssertEqual(table.inits, 0, "version \(version): init of the core must not run")
        }
        XCTAssertFalse(InprocTransport.isClaimed("undra_fixture"))
        XCTAssertEqual(try coreVersion(try load(fixtureTable)), "undra-ffi test core 1", "the core is still free to load")
    }

    func testASchemaMismatchIsRefusedBeforeInit() throws {
        let table = CountingTable()
        XCTAssertThrowsError(try load(table.pointer, expected: fixtureHash &+ 1)) { error in
            XCTAssertEqual(error as? UndraSchemaMismatchError, UndraSchemaMismatchError(expected: fixtureHash &+ 1, got: fixtureHash))
        }
        XCTAssertEqual(table.inits, 0, "init of the core must not run for bindings of another schema")
        XCTAssertFalse(InprocTransport.isClaimed("undra_fixture"))

        let core = try load(table.pointer)
        XCTAssertEqual(table.inits, 1, "the counting table does reach the core's init")
        XCTAssertEqual(try coreVersion(core), "undra-ffi test core 1")
    }

    // MARK: One core per namespace

    func testASecondLoadOfTheNamespaceIsRefusedWhileItIsLoadedAndAllowedAfterClose() throws {
        let core = try load(fixtureTable)
        XCTAssertTrue(InprocTransport.isClaimed("undra_fixture"))

        XCTAssertThrowsError(try load(fixtureTable), "the same table again") { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.alreadyLoaded)
        }
        let copy = CountingTable()
        XCTAssertThrowsError(try load(copy.pointer), "another table of the same namespace") { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.alreadyLoaded)
        }
        XCTAssertEqual(copy.inits, 0, "a refused load never reaches init")
        XCTAssertEqual(try coreVersion(core), "undra-ffi test core 1", "the loaded core is unaffected")

        core.shutdown()
        XCTAssertFalse(InprocTransport.isClaimed("undra_fixture"))
        let again = try load(fixtureTable)
        XCTAssertEqual(try coreVersion(again), "undra-ffi test core 1", "shutdown leaves the core ready for init")
    }

    // MARK: The generated entry's runtime half

    func testTheEntryFillsInTheTableAndTheHashAndHandsOutThePlaceholderAfterClose() throws {
        let entry = UndraCoreEntry(namespace: "undra_fixture", schemaHash: fixtureHash, api: { undra_fixture_undra_api() })
        XCTAssertTrue(entry.core === UndraCore.unloaded, "before load: the closed placeholder")

        let core = try entry.load(.inproc(adapters: Adapters.none))
        XCTAssertTrue(entry.core === core)
        XCTAssertEqual(core.schemaHash, fixtureHash)
        XCTAssertEqual(try coreVersion(entry.core), "undra-ffi test core 1")

        core.shutdown()
        XCTAssertTrue(entry.core === UndraCore.unloaded, "after close: the closed placeholder UndraCore.shared returns too")
        XCTAssertTrue(entry.core.isShutDown)
        XCTAssertThrowsError(try coreVersion(entry.core)) { error in
            XCTAssertEqual(UndraCallError.mapped(error) as? UndraCallError, .unavailable(.closed))
        }

        let again = try entry.load(.inproc(adapters: Adapters.none))
        addTeardownBlock { again.shutdown() }
        XCTAssertTrue(entry.core === again)
        XCTAssertEqual(try coreVersion(again), "undra-ffi test core 1")
    }

    // MARK: The whole ABI

    func testRealCoreOverTheCABI() async throws {
        let hash = fixtureHash
        XCTAssertNotEqual(hash, 0)
        let records = Records()

        // Load, use, shut down, load again: shutdown must leave the process ready for init.
        for round in 1 ... 2 {
            let core = try UndraCore.load(.inproc(api: fixtureTable, adapters: adapters(records), expectedSchemaHash: hash))
            XCTAssertEqual(core.schemaHash, hash, "round \(round)")

            // A free function and a constructor.
            XCTAssertEqual(try coreVersion(core), "undra-ffi test core 1")
            let calc = try core.construct(type: fnv("Calculator"), method: mid("Calculator", "new"), args: i64(100))
            func on(_ name: String) -> CallTarget { .objectMethod(handle: calc, methodId: mid("Calculator", name)) }

            // Sync and async calls, typed errors, bad requests, panics.
            let add = try core.callSync(on("add"), method: mid("Calculator", "add"), args: i64(2) + i64(3))
            XCTAssertEqual(try readI64(add), 105)
            let slow = try await core.call(on("slow_add"), method: mid("Calculator", "slow_add"), args: i64(2) + i64(3))
            XCTAssertEqual(try readI64(slow), 105)
            XCTAssertThrowsError(try core.callSync(on("fail"), method: mid("Calculator", "fail"), args: [])) { error in
                XCTAssertEqual((error as? UndraReplyError)?.status, .error)
            }
            XCTAssertThrowsError(try core.callSync(.freeFunction(methodId: 0xDEAD_BEEF), method: 0xDEAD_BEEF, args: [])) { error in
                XCTAssertEqual((error as? UndraReplyError)?.status, .badRequest)
            }
            do {
                _ = try await core.call(.freeFunction(methodId: 0xDEAD_BEEF), method: 0xDEAD_BEEF, args: [])
                XCTFail("an unknown method must fail")
            } catch {
                XCTAssertEqual((error as? UndraReplyError)?.status, .badRequest)
            }
            XCTAssertThrowsError(try core.callSync(on("boom"), method: mid("Calculator", "boom"), args: [])) { error in
                XCTAssertEqual((error as? UndraReplyError)?.status, .panic, "a panic is a status 2 reply, not a crash")
            }

            // A stream.
            var seen: [UInt32] = []
            for try await item in core.stream(on("ticks"), method: mid("Calculator", "ticks"), args: u32(40)) {
                seen.append(readU32(item))
            }
            XCTAssertEqual(seen, Array(0 ..< 40))

            // Host ports: sync answered inline with malloc'd memory, async answered later.
            let sum = try core.callSync(on("sum_on_host"), method: mid("Calculator", "sum_on_host"), args: u32(20) + u32(22))
            XCTAssertEqual(readU32(sum), 42)
            let echo = try await core.call(on("ping_host"), method: mid("Calculator", "ping_host"), args: u32(7))
            XCTAssertEqual(readU32(echo), 1007)

            // Logs reach the Log port.
            records.clear()
            await MainActor.run { core.observe(UndraHandle(rawValue: 0x7777_7777_0000_0001), signal: 0, on: true) }
            XCTAssertTrue(records.all.contains { $0.0 == 3 && $0.1 == "undra::runtime" }, "\(records.all)")

            // A store: initial value, then an update, through the mirror on the main actor.
            let counter = try core.construct(type: fnv("Counter"), method: mid("Counter", "new"), args: [])
            let values = Guarded<[UInt32]>([])
            core.mirror.register(counter) { _, _, reader in
                if let value = try? reader.readU32() {
                    values.withLock { (current: inout [UInt32]) -> Void in current.append(value) }
                }
            }
            await MainActor.run { core.observe(counter, signal: UInt32.max, on: true) }
            _ = try core.callSync(.objectMethod(handle: counter, methodId: mid("Counter", "bump")), method: mid("Counter", "bump"), args: [])
            for _ in 0 ..< 100 where values.withLock({ (current: inout [UInt32]) -> Int in current.count }) < 2 {
                try await Task.sleep(nanoseconds: 10_000_000)
            }
            XCTAssertEqual(values.withLock { (current: inout [UInt32]) -> [UInt32] in current }, [0, 1])

            // Snapshot and restore (restore invalidates non-store handles, so do it last).
            let snapshot = try core.snapshot()
            try core.restore(snapshot)
            XCTAssertThrowsError(try core.restore([1, 2, 3]))

            let stats = core.stats()
            XCTAssertGreaterThanOrEqual(stats.coreLiveHandles, 1)

            core.shutdown()
        }
    }
}
