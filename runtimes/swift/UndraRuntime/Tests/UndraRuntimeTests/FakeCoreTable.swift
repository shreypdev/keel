// A scripted core behind a real `UndraApi` table (`undra.h`, ADR-044), built in Swift: every entry
// is a `@convention(c)` closure, so the runtime reads, checks and calls it exactly as it does the
// table a Rust core's `<namespace>_undra_api()` returns.

import Foundation
import UndraFFI
@testable import UndraRuntime

/// What the fake core's entries record and answer.
///
/// A C function pointer captures nothing, so the script is process-wide. Every test that uses a
/// `FakeCoreTable` calls `FakeCore.reset()` first; XCTest runs the tests of a process one at a time.
enum FakeCore {
    struct Script {
        /// The entries called, in order (`init`, `call_sync`, `observe 7 1 1`, ...). `buf_free` is
        /// counted in `freed` instead.
        var calls: [String] = []
        /// What `init` returns.
        var initCode: UInt32 = 0
        /// What `restore` returns.
        var restoreCode: UInt32 = 0
        /// The encoded `RuntimeConfig` of every `init`.
        var configs: [[UInt8]] = []
        /// The reply callback and `user` of the last `init`, until `shutdown`.
        var reply: undra_reply_cb? = nil
        var user: UnsafeMutableRawPointer? = nil
        /// The port callbacks registered, by port id, until `shutdown`.
        var ports: [UInt32: (callback: undra_port_cb, user: UnsafeMutableRawPointer?)] = [:]
        /// Buffers handed to the runtime, and buffers it gave back to `buf_free`.
        var allocated = 0
        var freed = 0
    }

    static let script = Guarded<Script>(Script())

    /// Starts a test with an empty script.
    static func reset(initCode: UInt32 = 0) {
        script.withLock { (current: inout Script) -> Void in
            current = Script()
            current.initCode = initCode
        }
    }

    static func record(_ call: String) {
        script.withLock { (current: inout Script) -> Void in
            current.calls.append(call)
        }
    }

    /// The entries called so far.
    static var calls: [String] {
        return script.withLock { (current: inout Script) -> [String] in current.calls }
    }

    /// Whether `init` ran.
    static var initialised: Bool {
        return calls.contains("init")
    }

    /// A core-owned buffer holding `bytes` (from `malloc`, released by the table's `buf_free`).
    static func buffer(_ bytes: [UInt8]) -> UndraBuf {
        guard !bytes.isEmpty, let memory = malloc(bytes.count) else {
            return UndraBuf(ptr: nil, len: 0, cap: 0)
        }
        bytes.withUnsafeBytes { (source: UnsafeRawBufferPointer) -> Void in
            memory.copyMemory(from: source.baseAddress!, byteCount: bytes.count)
        }
        script.withLock { (current: inout Script) -> Void in
            current.allocated += 1
        }
        let length = UInt32(bytes.count)
        return UndraBuf(ptr: memory.assumingMemoryBound(to: UInt8.self), len: length, cap: length)
    }

    static func bytes(_ ptr: UnsafePointer<UInt8>?, _ len: UInt32) -> [UInt8] {
        guard let ptr = ptr, len > 0 else {
            return []
        }
        return Array(UnsafeBufferPointer(start: ptr, count: Int(len)))
    }

    /// The reply the fake core gives every call: status ok, the call's own arguments as the body.
    static func echo(_ ptr: UnsafePointer<UInt8>?, _ len: UInt32) -> (callId: UInt32, reply: [UInt8])? {
        guard let call = try? Wire.Call.decode(bytes(ptr, len)) else {
            return nil
        }
        return (call.callId, Wire.Reply(callId: call.callId, status: .ok, body: call.args).encode())
    }

    /// Calls the port callback registered for `portId` as the core would, and returns what it
    /// answered: its return code and, for 0, the `PortReply` bytes (whose block it frees with
    /// `free`, the host reply memory rule of `undra.h`).
    static func callPort(_ portId: UInt32, method: UInt32, portCallId: UInt32, args: [UInt8]) -> (code: UInt8, reply: [UInt8])? {
        let registration = script.withLock { (current: inout Script) -> (callback: undra_port_cb, user: UnsafeMutableRawPointer?)? in
            return current.ports[portId]
        }
        guard let registration = registration else {
            return nil
        }
        var out = UndraBuf(ptr: nil, len: 0, cap: 0)
        let code = args.withUnsafeBufferPointer { (buffer: UnsafeBufferPointer<UInt8>) -> UInt8 in
            return registration.callback(registration.user, portId, method, portCallId, buffer.baseAddress, UInt32(buffer.count), &out)
        }
        let reply = bytes(out.ptr.map { UnsafePointer($0) }, out.len)
        if code == 0 {
            precondition(out.cap == 0, "the host sets cap to 0")
            free(out.ptr)
        }
        return (code, reply)
    }
}

/// One fake core's table, in memory this object owns.
final class FakeCoreTable {
    /// The entries of `UndraApi` in their order (SPEC 6).
    static let entryNames = [
        "schema_json", "init", "shutdown", "call", "call_sync", "cancel", "stream_credit", "observe",
        "release", "port_register", "port_reply", "event", "timer_fired", "snapshot", "restore",
        "stats_json", "buf_free",
    ]

    let api: UnsafeMutablePointer<UndraApi>
    private let name: UnsafeMutablePointer<CChar>?

    /// What `<namespace>_undra_api()` would return.
    var pointer: UnsafeRawPointer {
        return UnsafeRawPointer(api)
    }

    /// A version 2 table of the core `namespace` (`nil`: a table without one) built from
    /// `schemaHash`; `abiVersion` and `size` override the header fields.
    init(
        namespace: String? = "fake_core",
        schemaHash: UInt64 = 0xFA4E_C0DE_0000_0001,
        abiVersion: UInt32 = UndraCore.abiVersion,
        size: UInt32 = UInt32(MemoryLayout<UndraApi>.size)
    ) {
        name = namespace.map { strdup($0) }
        var table = UndraApi()
        table.abi_version = abiVersion
        table.size = size
        table.schema_hash = schemaHash
        table.name_space = UnsafePointer(name)
        table.schema_json = {
            FakeCore.record("schema_json")
            return FakeCore.buffer(Array("{}".utf8))
        }
        table.`init` = { cfg, len, reply, _, _, user in
            FakeCore.record("init")
            return FakeCore.script.withLock { (current: inout FakeCore.Script) -> UInt32 in
                current.configs.append(FakeCore.bytes(cfg, len))
                if current.initCode == 0 {
                    current.reply = reply
                    current.user = user
                }
                return current.initCode
            }
        }
        table.shutdown = {
            FakeCore.record("shutdown")
            FakeCore.script.withLock { (current: inout FakeCore.Script) -> Void in
                current.reply = nil
                current.user = nil
                current.ports = [:]
            }
        }
        table.call = { ptr, len in
            FakeCore.record("call")
            guard let echoed = FakeCore.echo(ptr, len) else {
                return 5
            }
            let target = FakeCore.script.withLock { (current: inout FakeCore.Script) -> (undra_reply_cb?, UnsafeMutableRawPointer?) in
                return (current.reply, current.user)
            }
            // A core may reply before `call` returns (undra.h, contract 2).
            echoed.reply.withUnsafeBufferPointer { (buffer: UnsafeBufferPointer<UInt8>) -> Void in
                target.0?(target.1, echoed.callId, buffer.baseAddress, UInt32(buffer.count))
            }
            return 0
        }
        table.call_sync = { ptr, len in
            FakeCore.record("call_sync")
            guard let echoed = FakeCore.echo(ptr, len) else {
                return UndraBuf(ptr: nil, len: 0, cap: 0)
            }
            return FakeCore.buffer(echoed.reply)
        }
        table.cancel = { callId in FakeCore.record("cancel \(callId)") }
        table.stream_credit = { callId, credit in FakeCore.record("stream_credit \(callId) \(credit)") }
        table.observe = { handle, signal, on in FakeCore.record("observe \(handle) \(signal) \(on)") }
        table.release = { handle in FakeCore.record("release \(handle)") }
        table.port_register = { portId, callback, user in
            FakeCore.record("port_register \(portId)")
            FakeCore.script.withLock { (current: inout FakeCore.Script) -> Void in
                current.ports[portId] = callback.map { (callback: $0, user: user) }
            }
        }
        table.port_reply = { _, len in FakeCore.record("port_reply \(len)") }
        table.event = { portId, methodId, _, len in FakeCore.record("event \(portId) \(methodId) \(len)") }
        table.timer_fired = { timerId in FakeCore.record("timer_fired \(timerId)") }
        table.snapshot = {
            FakeCore.record("snapshot")
            return FakeCore.buffer([1, 2, 3])
        }
        table.restore = { _, len in
            FakeCore.record("restore \(len)")
            return FakeCore.script.withLock { (current: inout FakeCore.Script) -> UInt32 in current.restoreCode }
        }
        table.stats_json = {
            FakeCore.record("stats_json")
            return FakeCore.buffer(Array(#"{"live_handles":3}"#.utf8))
        }
        table.buf_free = { buffer in
            guard let ptr = buffer.ptr else {
                return
            }
            free(ptr)
            FakeCore.script.withLock { (current: inout FakeCore.Script) -> Void in
                current.freed += 1
            }
        }
        api = UnsafeMutablePointer<UndraApi>.allocate(capacity: 1)
        api.initialize(to: table)
    }

    /// Makes the entry `name` (one of `entryNames`) a null pointer.
    func nullEntry(_ name: String) {
        guard let index = FakeCoreTable.entryNames.firstIndex(of: name), let first = MemoryLayout<UndraApi>.offset(of: \UndraApi.schema_json) else {
            preconditionFailure("no entry \(name)")
        }
        let offset = first + index * MemoryLayout<UnsafeRawPointer>.stride
        UnsafeMutableRawPointer(api).storeBytes(of: 0, toByteOffset: offset, as: UInt.self)
    }

    deinit {
        api.deinitialize(count: 1)
        api.deallocate()
        free(name)
    }
}
