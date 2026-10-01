// The in-process transport: one core's native C ABI (docs/SPEC.md section 6), called through the
// entries of its `UndraApi` table (`CoreTable`, ADR-044).
//
// Callback discipline (SPEC 5.1 and 6): the core invokes the reply, change-set, stream and port
// callbacks on its own threads or on the caller's, possibly while it holds its lock. The
// trampolines below copy the bytes and hand them to `UndraInbound`; they never call an entry of
// the table (except `buf_free` on memory the core gave us, and only outside callbacks).

import UndraFFI

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// Calls one core through its C ABI table. A core runs at most once per process, so there is at
/// most one started `InprocTransport` per core namespace; cores of different namespaces (two
/// independent cores, ADR-044) run side by side, each with its own transport.
final class InprocTransport: UndraTransport, @unchecked Sendable {
    /// The started transports, by the namespace of their core.
    private static let active = Guarded<[String: InprocTransport]>([:])

    private struct State {
        var inbound: (any UndraInbound)? = nil
        var isShutDown = false
        /// The `user` pointer handed to every callback: this object, retained for the life of
        /// the process.
        var user: UnsafeMutableRawPointer? = nil
    }

    private let state = Guarded<State>(State())
    /// The core's entry points, read from its table once.
    private let table: CoreTable

    /// A transport over the core whose checked table is `table`.
    init(table: CoreTable) {
        self.table = table
    }

    /// Whether a transport over the core of `namespace` is started in this process.
    static func isClaimed(_ namespace: String) -> Bool {
        return active.withLock { (claims: inout [String: InprocTransport]) -> Bool in
            return claims[namespace] != nil
        }
    }

    var mode: UndraMode {
        return .inproc
    }

    var supportsDirectSync: Bool {
        return true
    }

    // MARK: Start and stop

    func start(inbound: any UndraInbound, options: TransportStartOptions) throws -> TransportInfo {
        // The ABI version was checked when the table was read (`CoreTable(reading:)`). The schema
        // hash is compared before the core is initialised (docs/SPEC.md section 11: the check is
        // at attach). It is plain data in the table, so bindings generated for another schema are
        // refused without `init` having run, which matters when the core is already running for
        // someone else (a second `init` is refused, and the caller would see `coreInitFailed`
        // instead of the mismatch) and saves starting a core only to shut it down.
        let schemaHash = table.schemaHash
        if schemaHash != options.expectedSchemaHash {
            throw UndraSchemaMismatchError(expected: options.expectedSchemaHash, got: schemaHash)
        }
        let namespace = table.namespace
        let claimed = InprocTransport.active.withLock { (claims: inout [String: InprocTransport]) -> Bool in
            if claims[namespace] != nil {
                return false
            }
            claims[namespace] = self
            return true
        }
        if !claimed {
            throw UndraLoadError.alreadyLoaded
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
        ).undraEncoded()
        let code = initializeCore(config: config, user: user)
        if code != 0 {
            state.withLock { (current: inout State) -> Void in
                current.isShutDown = true
                current.inbound = nil
            }
            releaseClaim()
            throw UndraLoadError.coreInitFailed(code: code)
        }
        return TransportInfo(schemaHash: schemaHash)
    }

    /// The table's `init` with the trampolines that route the core's callbacks to the transport
    /// that `user` points to.
    private func initializeCore(config: [UInt8], user: UnsafeMutableRawPointer) -> UInt32 {
        let replyCallback: undra_reply_cb = { userData, callId, ptr, len in
            InprocTransport.deliverReply(userData, callId, ptr, len)
        }
        let changeSetCallback: undra_changeset_cb = { userData, ptr, len in
            InprocTransport.deliverChangeSet(userData, ptr, len)
        }
        let streamCallback: undra_stream_cb = { userData, callId, ptr, len in
            InprocTransport.deliverStreamItem(userData, callId, ptr, len)
        }
        let initialize = table.initialize
        return config.withUnsafeBufferPointer { (buffer: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return initialize(
                buffer.baseAddress,
                UInt32(buffer.count),
                replyCallback,
                changeSetCallback,
                streamCallback,
                user
            )
        }
    }

    /// Gives up this transport's claim on its core's namespace.
    private func releaseClaim() {
        let namespace = table.namespace
        InprocTransport.active.withLock { (claims: inout [String: InprocTransport]) -> Void in
            if claims[namespace] === self {
                claims[namespace] = nil
            }
        }
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
        table.shutdown()
        releaseClaim()
    }

    private var isRunning: Bool {
        return state.withLock { (current: inout State) -> Bool in
            return !current.isShutDown
        }
    }

    fileprivate func currentInbound() -> (any UndraInbound)? {
        return state.withLock { (current: inout State) -> (any UndraInbound)? in
            return current.inbound
        }
    }

    // MARK: Calls

    func send(call payload: [UInt8]) -> Bool {
        if !isRunning {
            return false
        }
        let code = payload.withUnsafeBufferPointer { (buffer: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return table.call(buffer.baseAddress, UInt32(truncatingIfNeeded: buffer.count))
        }
        return code == 0
    }

    func callSync(_ payload: [UInt8]) throws -> [UInt8] {
        if !isRunning {
            throw UndraTransportError.closed
        }
        let buffer = payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> UndraBuf in
            return table.callSync(bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
        return takeBytes(buffer)
    }

    func cancel(callId: UInt32) {
        if isRunning {
            table.cancel(callId)
        }
    }

    func streamCredit(callId: UInt32, credit: UInt32) {
        if isRunning {
            table.streamCredit(callId, credit)
        }
    }

    func observe(handle: UndraHandle, signal: UInt32, on: Bool) {
        if isRunning {
            table.observe(handle.rawValue, signal, on ? 1 : 0)
        }
    }

    func release(handle: UndraHandle) {
        if isRunning {
            table.release(handle.rawValue)
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
        let portCallback: undra_port_cb = { userData, callPortId, methodId, portCallId, ptr, len, outReply in
            return InprocTransport.deliverPortCall(userData, callPortId, methodId, portCallId, ptr, len, outReply)
        }
        table.portRegister(portId, portCallback, user)
    }

    func portReply(_ payload: [UInt8]) {
        if !isRunning {
            return
        }
        payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> Void in
            table.portReply(bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
    }

    func event(portId: UInt32, methodId: UInt32, payload: [UInt8]) {
        if !isRunning {
            return
        }
        payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> Void in
            table.event(portId, methodId, bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
    }

    func timerFired(_ timerId: UInt32) {
        if isRunning {
            table.timerFired(timerId)
        }
    }

    // MARK: Snapshots and statistics

    func snapshot() throws -> [UInt8] {
        if !isRunning {
            throw UndraTransportError.closed
        }
        return takeBytes(table.snapshot())
    }

    func restore(_ payload: [UInt8]) throws {
        if !isRunning {
            throw UndraTransportError.closed
        }
        let code = payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return table.restore(bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
        if code != 0 {
            throw UndraRestoreError(code: code)
        }
    }

    func statsJSON() -> String? {
        if !isRunning {
            return nil
        }
        let bytes = takeBytes(table.statsJSON())
        return String(decoding: bytes, as: UTF8.self)
    }

    // MARK: Memory

    /// Copies a core-owned buffer into an array and frees it with the table's `buf_free`.
    private func takeBytes(_ buffer: UndraBuf) -> [UInt8] {
        let mutablePointer: UnsafeMutablePointer<UInt8>? = buffer.ptr
        let readablePointer: UnsafePointer<UInt8>? = mutablePointer.map { (pointer: UnsafeMutablePointer<UInt8>) -> UnsafePointer<UInt8> in
            return UnsafePointer(pointer)
        }
        let copy = InprocTransport.copyBytes(readablePointer, buffer.len)
        table.bufFree(buffer)
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
    /// (see the comment next to `undra_port_cb` in `undra.h`).
    fileprivate static func deliverPortCall(
        _ raw: UnsafeMutableRawPointer?,
        _ portId: UInt32,
        _ methodId: UInt32,
        _ portCallId: UInt32,
        _ ptr: UnsafePointer<UInt8>?,
        _ len: UInt32,
        _ outReply: UnsafeMutablePointer<UndraBuf>?
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
            outReply.pointee = UndraBuf(
                ptr: memory.assumingMemoryBound(to: UInt8.self),
                len: UInt32(truncatingIfNeeded: reply.count),
                cap: 0
            )
            return 0
        }
    }
}
