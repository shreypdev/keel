// The in-process transport: the native C ABI of docs/SPEC.md section 6, called through the
// `KeelFFI` module.
//
// Callback discipline (SPEC 5.1 and 6): the core invokes the reply, change-set, stream and port
// callbacks on its own threads or on the caller's, possibly while it holds its lock. The
// trampolines below copy the bytes and hand them to `KeelInbound`; they never call a `keel_*`
// function (except `keel_buf_free` on memory the core gave us, and only outside callbacks).

import KeelFFI

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// Calls the linked core through the C ABI. There is one core per process, so there is at most
/// one started `InprocTransport`.
final class InprocTransport: KeelTransport, @unchecked Sendable {
    /// The started transport, if any. Also what keeps the `user` pointer of the callbacks valid.
    private static let active = Guarded<InprocTransport?>(nil)

    private struct State {
        var inbound: (any KeelInbound)? = nil
        var isShutDown = false
        /// The `user` pointer handed to every callback: this object, retained for the life of
        /// the process.
        var user: UnsafeMutableRawPointer? = nil
    }

    private let state = Guarded<State>(State())

    init() {}

    /// The ABI version of whatever is linked behind `KeelFFI` (0 for the link-time stub).
    static var linkedABIVersion: UInt32 {
        return keel_abi_version()
    }

    var mode: KeelMode {
        return .inproc
    }

    var supportsDirectSync: Bool {
        return true
    }

    // MARK: Start and stop

    func start(inbound: any KeelInbound, options: TransportStartOptions) throws -> TransportInfo {
        let abi = keel_abi_version()
        if abi != KeelCore.abiVersion {
            throw KeelLoadError.abiMismatch(expected: KeelCore.abiVersion, got: abi)
        }
        let claimed = InprocTransport.active.withLock { (slot: inout InprocTransport?) -> Bool in
            if slot != nil {
                return false
            }
            slot = self
            return true
        }
        if !claimed {
            throw KeelLoadError.alreadyLoaded
        }
        // The core keeps this pointer for the life of the process: the retained reference is
        // intentionally never released, so a callback that races with `shutdown()` still finds a
        // live object.
        let user = Unmanaged.passRetained(self).toOpaque()
        state.withLock { (current: inout State) -> Void in
            current.inbound = inbound
            current.isShutDown = false
            current.user = user
        }

        let config = RuntimeConfigRecord(
            platform: options.platform,
            mode: "inproc",
            coreThreads: 1,
            blockingThreads: 0,
            logLevel: options.logLevel
        ).keelEncoded()

        let replyCallback: keel_reply_cb = { userData, callId, ptr, len in
            InprocTransport.deliverReply(userData, callId, ptr, len)
        }
        let changeSetCallback: keel_changeset_cb = { userData, ptr, len in
            InprocTransport.deliverChangeSet(userData, ptr, len)
        }
        let streamCallback: keel_stream_cb = { userData, callId, ptr, len in
            InprocTransport.deliverStreamItem(userData, callId, ptr, len)
        }
        let code = config.withUnsafeBufferPointer { (buffer: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return keel_init(
                buffer.baseAddress,
                UInt32(buffer.count),
                replyCallback,
                changeSetCallback,
                streamCallback,
                user
            )
        }
        if code != 0 {
            InprocTransport.active.withLock { (slot: inout InprocTransport?) -> Void in
                slot = nil
            }
            throw KeelLoadError.coreInitFailed(code: code)
        }
        return TransportInfo(schemaHash: keel_schema_hash())
    }

    func shutdown() {
        let wasRunning = state.withLock { (current: inout State) -> Bool in
            if current.isShutDown {
                return false
            }
            current.isShutDown = true
            return true
        }
        if !wasRunning {
            return
        }
        keel_shutdown()
        InprocTransport.active.withLock { (slot: inout InprocTransport?) -> Void in
            if slot === self {
                slot = nil
            }
        }
    }

    private var isRunning: Bool {
        return state.withLock { (current: inout State) -> Bool in
            return !current.isShutDown
        }
    }

    fileprivate func currentInbound() -> (any KeelInbound)? {
        return state.withLock { (current: inout State) -> (any KeelInbound)? in
            return current.inbound
        }
    }

    // MARK: Calls

    func send(call payload: [UInt8]) -> Bool {
        if !isRunning {
            return false
        }
        let code = payload.withUnsafeBufferPointer { (buffer: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return keel_call(buffer.baseAddress, UInt32(truncatingIfNeeded: buffer.count))
        }
        return code == 0
    }

    func callSync(_ payload: [UInt8]) throws -> [UInt8] {
        if !isRunning {
            throw KeelTransportError.closed
        }
        let buffer = payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> KeelBuf in
            return keel_call_sync(bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
        return InprocTransport.takeBytes(buffer)
    }

    func cancel(callId: UInt32) {
        if isRunning {
            keel_cancel(callId)
        }
    }

    func streamCredit(callId: UInt32, credit: UInt32) {
        if isRunning {
            keel_stream_credit(callId, credit)
        }
    }

    func observe(handle: KeelHandle, signal: UInt32, on: Bool) {
        if isRunning {
            keel_observe(handle.rawValue, signal, on ? 1 : 0)
        }
    }

    func release(handle: KeelHandle) {
        if isRunning {
            keel_release(handle.rawValue)
        }
    }

    // MARK: Ports and events

    func registerPort(_ portId: UInt32) {
        if !isRunning {
            return
        }
        let user = state.withLock { (current: inout State) -> UnsafeMutableRawPointer? in
            return current.user
        }
        guard let user = user else {
            return
        }
        let portCallback: keel_port_cb = { userData, callPortId, methodId, portCallId, ptr, len, outReply in
            return InprocTransport.deliverPortCall(userData, callPortId, methodId, portCallId, ptr, len, outReply)
        }
        keel_port_register(portId, portCallback, user)
    }

    func portReply(_ payload: [UInt8]) {
        if !isRunning {
            return
        }
        payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> Void in
            keel_port_reply(bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
    }

    func event(portId: UInt32, methodId: UInt32, payload: [UInt8]) {
        if !isRunning {
            return
        }
        payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> Void in
            keel_event(portId, methodId, bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
    }

    func timerFired(_ timerId: UInt32) {
        if isRunning {
            keel_timer_fired(timerId)
        }
    }

    // MARK: Snapshots and statistics

    func snapshot() throws -> [UInt8] {
        if !isRunning {
            throw KeelTransportError.closed
        }
        return InprocTransport.takeBytes(keel_snapshot())
    }

    func restore(_ payload: [UInt8]) throws {
        if !isRunning {
            throw KeelTransportError.closed
        }
        let code = payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return keel_restore(bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
        if code != 0 {
            throw KeelRestoreError(code: code)
        }
    }

    func statsJSON() -> String? {
        if !isRunning {
            return nil
        }
        let bytes = InprocTransport.takeBytes(keel_stats_json())
        return String(decoding: bytes, as: UTF8.self)
    }

    // MARK: Memory

    /// Copies a core-owned buffer into an array and frees it.
    private static func takeBytes(_ buffer: KeelBuf) -> [UInt8] {
        let mutablePointer: UnsafeMutablePointer<UInt8>? = buffer.ptr
        let readablePointer: UnsafePointer<UInt8>? = mutablePointer.map { (pointer: UnsafeMutablePointer<UInt8>) -> UnsafePointer<UInt8> in
            return UnsafePointer(pointer)
        }
        let copy = copyBytes(readablePointer, buffer.len)
        keel_buf_free(buffer)
        return copy
    }

    /// Copies `len` bytes at `ptr` (borrowed from the core for the duration of a callback).
    fileprivate static func copyBytes(_ ptr: UnsafePointer<UInt8>?, _ len: UInt32) -> [UInt8] {
        guard let ptr = ptr, len > 0 else {
            return []
        }
        return Array(UnsafeBufferPointer(start: ptr, count: Int(len)))
    }

    // MARK: Callbacks from the core

    private static func instance(_ raw: UnsafeMutableRawPointer?) -> InprocTransport? {
        guard let raw = raw else {
            return nil
        }
        return Unmanaged<InprocTransport>.fromOpaque(raw).takeUnretainedValue()
    }

    fileprivate static func deliverReply(
        _ raw: UnsafeMutableRawPointer?,
        _ callId: UInt32,
        _ ptr: UnsafePointer<UInt8>?,
        _ len: UInt32
    ) {
        guard let transport = instance(raw), let inbound = transport.currentInbound() else {
            return
        }
        inbound.onReply(callId: callId, payload: copyBytes(ptr, len))
    }

    fileprivate static func deliverChangeSet(
        _ raw: UnsafeMutableRawPointer?,
        _ ptr: UnsafePointer<UInt8>?,
        _ len: UInt32
    ) {
        guard let transport = instance(raw), let inbound = transport.currentInbound() else {
            return
        }
        inbound.onChangeSet(copyBytes(ptr, len))
    }

    fileprivate static func deliverStreamItem(
        _ raw: UnsafeMutableRawPointer?,
        _ callId: UInt32,
        _ ptr: UnsafePointer<UInt8>?,
        _ len: UInt32
    ) {
        guard let transport = instance(raw), let inbound = transport.currentInbound() else {
            return
        }
        inbound.onStreamItem(callId: callId, payload: copyBytes(ptr, len))
    }

    /// Answers a port call (docs/SPEC.md section 6.3). A synchronous answer is written into
    /// `outReply` in memory from `malloc` with `cap = 0`; the core copies it and calls `free`
    /// (see the comment next to `keel_port_cb` in `keel.h`).
    fileprivate static func deliverPortCall(
        _ raw: UnsafeMutableRawPointer?,
        _ portId: UInt32,
        _ methodId: UInt32,
        _ portCallId: UInt32,
        _ ptr: UnsafePointer<UInt8>?,
        _ len: UInt32,
        _ outReply: UnsafeMutablePointer<KeelBuf>?
    ) -> UInt8 {
        guard let transport = instance(raw), let inbound = transport.currentInbound() else {
            return 2
        }
        let args = copyBytes(ptr, len)
        let outcome = inbound.onPortCall(portId: portId, methodId: methodId, portCallId: portCallId, args: args)
        switch outcome {
        case .unavailable:
            return 2
        case .async:
            return 1
        case .sync(let reply):
            guard let outReply = outReply, !reply.isEmpty, let memory = malloc(reply.count) else {
                return 2
            }
            reply.withUnsafeBytes { (bytes: UnsafeRawBufferPointer) -> Void in
                if let base = bytes.baseAddress {
                    memory.copyMemory(from: base, byteCount: reply.count)
                }
            }
            outReply.pointee = KeelBuf(
                ptr: memory.assumingMemoryBound(to: UInt8.self),
                len: UInt32(truncatingIfNeeded: reply.count),
                cap: 0
            )
            return 0
        }
    }
}
