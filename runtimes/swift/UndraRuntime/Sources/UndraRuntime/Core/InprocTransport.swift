// The in-process transport: the native C ABI of docs/SPEC.md section 6, called through the
// `UndraFFI` module.
//
// Callback discipline (SPEC 5.1 and 6): the core invokes the reply, change-set, stream and port
// callbacks on its own threads or on the caller's, possibly while it holds its lock. The
// trampolines below copy the bytes and hand them to `UndraInbound`; they never call an `undra_*`
// function (except `undra_buf_free` on memory the core gave us, and only outside callbacks).

import UndraFFI

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// Calls the linked core through the C ABI. There is one core per process, so there is at most
/// one started `InprocTransport`.
final class InprocTransport: UndraTransport, @unchecked Sendable {
    /// The started transport, if any. Also what keeps the `user` pointer of the callbacks valid.
    private static let active = Guarded<InprocTransport?>(nil)

    private struct State {
        var inbound: (any UndraInbound)? = nil
        var isShutDown = false
        /// The `user` pointer handed to every callback: this object, retained for the life of
        /// the process.
        var user: UnsafeMutableRawPointer? = nil
    }

    /// The functions of the linked core that `start` calls before there is a transport to talk
    /// through. Tests replace them to script the order in which `start` uses them; the shipped
    /// value calls the C ABI.
    struct CoreEntry: Sendable {
        /// `undra_abi_version()`.
        var abiVersion: @Sendable () -> UInt32
        /// `undra_schema_hash()`, which works before `undra_init`.
        var schemaHash: @Sendable () -> UInt64
        /// `undra_init` with the encoded `RuntimeConfig` and the `user` pointer of every callback;
        /// returns its status code (0 when the core is running).
        var initialize: @Sendable (_ config: [UInt8], _ user: UnsafeMutableRawPointer) -> UInt32

        /// The core behind `UndraFFI`.
        static let linked = CoreEntry(
            abiVersion: { undra_abi_version() },
            schemaHash: { undra_schema_hash() },
            initialize: { InprocTransport.initializeLinkedCore(config: $0, user: $1) }
        )
    }

    private let state = Guarded<State>(State())
    private let entry: CoreEntry

    init(entry: CoreEntry = .linked) {
        self.entry = entry
    }

    /// The ABI version of whatever is linked behind `UndraFFI` (0 for the link-time stub).
    static var linkedABIVersion: UInt32 {
        return undra_abi_version()
    }

    var mode: UndraMode {
        return .inproc
    }

    var supportsDirectSync: Bool {
        return true
    }

    // MARK: Start and stop

    func start(inbound: any UndraInbound, options: TransportStartOptions) throws -> TransportInfo {
        let abi = entry.abiVersion()
        if abi != UndraCore.abiVersion {
            throw UndraLoadError.abiMismatch(expected: UndraCore.abiVersion, got: abi)
        }
        // The schema hash is compared before the core is initialised (docs/SPEC.md section 11:
        // the check is at attach). `undra_schema_hash` needs no running core, so bindings generated
        // for another schema are refused without `undra_init` having run, which matters when the
        // core is already initialised by someone else (a second `undra_init` is refused, and the
        // caller would see `coreInitFailed` instead of the mismatch) and saves starting a core
        // only to shut it down.
        let schemaHash = entry.schemaHash()
        if schemaHash != options.expectedSchemaHash {
            throw UndraSchemaMismatchError(expected: options.expectedSchemaHash, got: schemaHash)
        }
        let claimed = InprocTransport.active.withLock { (slot: inout InprocTransport?) -> Bool in
            if slot != nil {
                return false
            }
            slot = self
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
        let code = entry.initialize(config, user)
        if code != 0 {
            InprocTransport.active.withLock { (slot: inout InprocTransport?) -> Void in
                slot = nil
            }
            throw UndraLoadError.coreInitFailed(code: code)
        }
        return TransportInfo(schemaHash: schemaHash)
    }

    /// `undra_init` with the trampolines that route the core's callbacks to the transport that
    /// `user` points to.
    private static func initializeLinkedCore(config: [UInt8], user: UnsafeMutableRawPointer) -> UInt32 {
        let replyCallback: undra_reply_cb = { userData, callId, ptr, len in
            InprocTransport.deliverReply(userData, callId, ptr, len)
        }
        let changeSetCallback: undra_changeset_cb = { userData, ptr, len in
            InprocTransport.deliverChangeSet(userData, ptr, len)
        }
        let streamCallback: undra_stream_cb = { userData, callId, ptr, len in
            InprocTransport.deliverStreamItem(userData, callId, ptr, len)
        }
        return config.withUnsafeBufferPointer { (buffer: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return undra_init(
                buffer.baseAddress,
                UInt32(buffer.count),
                replyCallback,
                changeSetCallback,
                streamCallback,
                user
            )
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
        undra_shutdown()
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
            return undra_call(buffer.baseAddress, UInt32(truncatingIfNeeded: buffer.count))
        }
        return code == 0
    }

    func callSync(_ payload: [UInt8]) throws -> [UInt8] {
        if !isRunning {
            throw UndraTransportError.closed
        }
        let buffer = payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> UndraBuf in
            return undra_call_sync(bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
        return InprocTransport.takeBytes(buffer)
    }

    func cancel(callId: UInt32) {
        if isRunning {
            undra_cancel(callId)
        }
    }

    func streamCredit(callId: UInt32, credit: UInt32) {
        if isRunning {
            undra_stream_credit(callId, credit)
        }
    }

    func observe(handle: UndraHandle, signal: UInt32, on: Bool) {
        if isRunning {
            undra_observe(handle.rawValue, signal, on ? 1 : 0)
        }
    }

    func release(handle: UndraHandle) {
        if isRunning {
            undra_release(handle.rawValue)
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
        undra_port_register(portId, portCallback, user)
    }

    func portReply(_ payload: [UInt8]) {
        if !isRunning {
            return
        }
        payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> Void in
            undra_port_reply(bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
    }

    func event(portId: UInt32, methodId: UInt32, payload: [UInt8]) {
        if !isRunning {
            return
        }
        payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> Void in
            undra_event(portId, methodId, bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
    }

    func timerFired(_ timerId: UInt32) {
        if isRunning {
            undra_timer_fired(timerId)
        }
    }

    // MARK: Snapshots and statistics

    func snapshot() throws -> [UInt8] {
        if !isRunning {
            throw UndraTransportError.closed
        }
        return InprocTransport.takeBytes(undra_snapshot())
    }

    func restore(_ payload: [UInt8]) throws {
        if !isRunning {
            throw UndraTransportError.closed
        }
        let code = payload.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return undra_restore(bytes.baseAddress, UInt32(truncatingIfNeeded: bytes.count))
        }
        if code != 0 {
            throw UndraRestoreError(code: code)
        }
    }

    func statsJSON() -> String? {
        if !isRunning {
            return nil
        }
        let bytes = InprocTransport.takeBytes(undra_stats_json())
        return String(decoding: bytes, as: UTF8.self)
    }

    // MARK: Memory

    /// Copies a core-owned buffer into an array and frees it.
    private static func takeBytes(_ buffer: UndraBuf) -> [UInt8] {
        let mutablePointer: UnsafeMutablePointer<UInt8>? = buffer.ptr
        let readablePointer: UnsafePointer<UInt8>? = mutablePointer.map { (pointer: UnsafeMutablePointer<UInt8>) -> UnsafePointer<UInt8> in
            return UnsafePointer(pointer)
        }
        let copy = copyBytes(readablePointer, buffer.len)
        undra_buf_free(buffer)
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
