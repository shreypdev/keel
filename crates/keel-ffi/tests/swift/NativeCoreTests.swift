// The real C ABI driven by the real Swift runtime (KeelCore over InprocTransport), against the
// fixture core (crates/keel-ffi/tests/fixture) linked as a static library. It is not part of the
// Swift package: run.sh copies the package to a scratch directory, adds this file and links the
// core, so the Swift tree is untouched. Skips itself when the linked ABI is the stub.

import Foundation
import KeelFFI
import XCTest

@testable import KeelRuntime

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
    var w = KeelWriter()
    w.writeI64(value)
    return w.finish()
}

private func u32(_ value: UInt32) -> [UInt8] {
    var w = KeelWriter()
    w.writeU32(value)
    return w.finish()
}

private func readU32(_ bytes: [UInt8], at offset: Int = 0) -> UInt32 {
    var value: UInt32 = 0
    for i in 0 ..< 4 { value |= UInt32(bytes[offset + i]) << UInt32(8 * i) }
    return value
}

private func readI64(_ bytes: [UInt8]) throws -> Int64 {
    var r = KeelReader(bytes)
    return try r.readI64()
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
                var r = KeelReader(args)
                records.add(try r.readU8(), try r.readString(), try r.readString())
                return []
            },
        ])
        return Adapters.none
            .replacing(portId: portId("Sum"), with: sum)
            .replacing(portId: portId("Echo"), with: echo)
            .replacing(portId: portId("Log"), with: log)
    }

    func testRealCoreOverTheCABI() async throws {
        try XCTSkipUnless(InprocTransport.linkedABIVersion == 1, "the real core is not linked")
        let hash = keel_schema_hash()
        XCTAssertNotEqual(hash, 0)
        let records = Records()

        // Load, use, shut down, load again: keel_shutdown must leave the process ready for keel_init.
        for round in 1 ... 2 {
            let core = try KeelCore.load(.inproc(adapters: adapters(records), expectedSchemaHash: hash))
            XCTAssertEqual(core.schemaHash, hash, "round \(round)")

            // A free function and a constructor.
            let version = try core.callSync(.freeFunction(methodId: fnv("fn.version")), method: fnv("fn.version"), args: [])
            var vr = KeelReader(version)
            XCTAssertEqual(try vr.readString(), "keel-ffi test core 1")
            let calc = try core.construct(type: fnv("Calculator"), method: mid("Calculator", "new"), args: i64(100))
            func on(_ name: String) -> CallTarget { .objectMethod(handle: calc, methodId: mid("Calculator", name)) }

            // Sync and async calls, typed errors, bad requests, panics.
            let add = try core.callSync(on("add"), method: mid("Calculator", "add"), args: i64(2) + i64(3))
            XCTAssertEqual(try readI64(add), 105)
            let slow = try await core.call(on("slow_add"), method: mid("Calculator", "slow_add"), args: i64(2) + i64(3))
            XCTAssertEqual(try readI64(slow), 105)
            XCTAssertThrowsError(try core.callSync(on("fail"), method: mid("Calculator", "fail"), args: [])) { error in
                XCTAssertEqual((error as? KeelReplyError)?.status, .error)
            }
            XCTAssertThrowsError(try core.callSync(.freeFunction(methodId: 0xDEAD_BEEF), method: 0xDEAD_BEEF, args: [])) { error in
                XCTAssertEqual((error as? KeelReplyError)?.status, .badRequest)
            }
            do {
                _ = try await core.call(.freeFunction(methodId: 0xDEAD_BEEF), method: 0xDEAD_BEEF, args: [])
                XCTFail("an unknown method must fail")
            } catch {
                XCTAssertEqual((error as? KeelReplyError)?.status, .badRequest)
            }
            XCTAssertThrowsError(try core.callSync(on("boom"), method: mid("Calculator", "boom"), args: [])) { error in
                XCTAssertEqual((error as? KeelReplyError)?.status, .panic, "a panic is a status 2 reply, not a crash")
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
            await MainActor.run { core.observe(KeelHandle(rawValue: 0x7777_7777_0000_0001), signal: 0, on: true) }
            XCTAssertTrue(records.all.contains { $0.0 == 3 && $0.1 == "keel::runtime" }, "\(records.all)")

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
