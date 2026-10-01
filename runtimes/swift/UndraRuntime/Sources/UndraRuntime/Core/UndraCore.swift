// UndraCore: the runtime's public face (docs/SPEC.md sections 11 and 17.3).
//
// An `UndraCore` is one attachment to a Rust core, in process (the C ABI) or remote (a WebSocket).
// It owns what every runtime owns: call-id allocation, the map from replies to continuations,
// stream flow control, the mirror that applies change-sets on the main actor, and the port
// table. Generated code calls only the methods declared here.
//
// Threading rules this file lives by (docs/SPEC.md section 5.1):
//  * The core's callbacks (`onReply`, `onChangeSet`, `onStreamItem`, `onPortCall`) may run on any
//    thread, possibly while the core holds its lock. They copy, queue and resume continuations;
//    they never call an `undra_*` function. Work that must call into the core from there goes to
//    `deferredQueue`.
//  * Nothing here holds a lock while calling out (resuming a continuation, calling the transport,
//    running a port method).

import Dispatch

/// One attachment to a Rust core. Load one at startup and pass it (or leave it as
/// `UndraCore.shared`) to the generated bindings.
///
/// ```swift
/// let core = try UndraCore.load(.inproc(expectedSchemaHash: UndraIds.schemaHash))
/// let todos = try TodoStore()            // uses UndraCore.shared
/// ```
public final class UndraCore: @unchecked Sendable {
    // MARK: Constants

    /// The C ABI version this runtime implements (`undra_abi_version()` must report it).
    static let abiVersion: UInt32 = 1

    /// The Undra protocol version announced in the remote handshake.
    public static let undraVersion = "1.0.0"

    // MARK: State

    private struct State {
        var nextCallId: UInt32 = 0
        var pending: [UInt32: PendingCall] = [:]
        var ports: [UInt32: PortImpl] = [:]
        var adapters: [any UndraAdapter] = []
        var liveObjects = 0
        var isShutDown = false
        var schemaHash: UInt64 = 0
    }

    private static let sharedSlot = Guarded<UndraCore?>(nil)

    /// What `shared` returns when no core is loaded: shut down from the start, over a transport
    /// that reaches nothing.
    private static let unloaded = UndraCore(transport: UnloadedTransport(), isShutDown: true)

    /// Whether the placeholder's "load a core" message has been logged.
    private static let unloadedWarning = Guarded<Bool>(false)

    /// The change-set mirror: stores register with it and it applies the core's updates on the
    /// main actor, merged, once per display frame. `register(handle) { signal, op, reader in ... }`.
    public let mirror: Mirror

    let transport: any UndraTransport
    private let state: Guarded<State>
    private let blockingTimeout: Double
    private let onError: (@Sendable (UndraUnhandledError) -> Void)?
    private let deferredQueue = DispatchQueue(label: "dev.undra.runtime.deferred")

    /// True while `onError` runs on this task or thread, so a handler that makes a failing call
    /// is only logged, never reported again.
    @TaskLocal private static var isReporting = false

    init(
        transport: any UndraTransport,
        blockingCallTimeout: Double = 30,
        onError: (@Sendable (UndraUnhandledError) -> Void)? = nil,
        isShutDown: Bool = false,
        maxPendingEntries: Int = Mirror.defaultMaxPendingEntries,
        maxPendingBytes: Int = Mirror.defaultMaxPendingBytes,
        frameScheduler: (any FrameScheduler)? = nil
    ) {
        self.transport = transport
        self.mirror = Mirror(maxPendingEntries: maxPendingEntries, maxPendingBytes: maxPendingBytes, scheduler: frameScheduler)
        self.blockingTimeout = blockingCallTimeout
        self.onError = onError
        self.state = Guarded<State>(State(isShutDown: isShutDown))
        mirror.setResyncHandler { [weak self] handle, signal in
            self?.resync(handle, signal: signal)
        }
    }

    // MARK: Loading

    /// Attaches to a core, checks its schema hash, registers the adapters, and makes the result
    /// `UndraCore.shared` if none is loaded yet.
    ///
    /// - Throws: `UndraSchemaMismatchError` if the core's schema hash is not
    ///   `options.expectedSchemaHash`; `UndraLoadError` if the core cannot be reached or
    ///   initialised (including the link-time stub, see the package README).
    ///
    /// A remote core is reached with a blocking handshake, so call this once at startup, not on
    /// a hot path.
    @discardableResult
    public static func load(_ options: LoadOptions) throws -> UndraCore {
        let transport: any UndraTransport
        switch options.mode {
        case .inproc:
            transport = InprocTransport()
        case .remote(let url):
            transport = try WebSocketTransport(urlString: url)
        }
        let core = try connect(transport: transport, options: options)
        sharedSlot.withLock { (slot: inout UndraCore?) -> Void in
            if slot == nil {
                slot = core
            }
        }
        return core
    }

    /// The core that `UndraCore.load(_:)` attached first and which is not shut down, or, when there
    /// is none, a permanently shut-down placeholder.
    ///
    /// Generated code uses it as the default `ctx`. Using it before a successful `load`, or after
    /// `shutdown()`, is a programming error but not a crash: calls on the placeholder fail with
    /// ``UndraCallError/unavailable(_:)`` (`.closed`), constructors throw it, commands only log
    /// (the placeholder has no `LoadOptions.onError`), and the first use logs what to do.
    /// ``current`` still returns `nil` in that state, and the placeholder never becomes the shared
    /// core.
    public static var shared: UndraCore {
        if let core = current {
            return core
        }
        let first = unloadedWarning.withLock { (warned: inout Bool) -> Bool in
            if warned {
                return false
            }
            warned = true
            return true
        }
        if first {
            UndraLog.error(
                "UndraCore.shared was used while no core is loaded (before UndraCore.load(_:) succeeds, or after shutdown()); calls on it fail with UndraCallError.unavailable(.closed). Load a core at app startup, before creating any Undra object."
            )
        }
        return unloaded
    }

    /// The shared core, or `nil` if none is loaded. While it is `nil`, ``shared`` returns the
    /// shut-down placeholder, so check `current` (not `shared`) to learn whether a core is loaded.
    public static var current: UndraCore? {
        return sharedSlot.withLock { (slot: inout UndraCore?) -> UndraCore? in
            return slot
        }
    }

    /// Starts `transport` and completes the attachment. Tests call this with a scripted
    /// transport (and, to drive frames by hand, a `frameScheduler`); it does not touch
    /// `UndraCore.shared`.
    static func connect(
        transport: any UndraTransport,
        options: LoadOptions,
        frameScheduler: (any FrameScheduler)? = nil
    ) throws -> UndraCore {
        let core = UndraCore(
            transport: transport,
            blockingCallTimeout: options.blockingCallTimeout,
            onError: options.onError,
            maxPendingEntries: options.maxPendingEntries,
            maxPendingBytes: options.maxPendingBytes,
            frameScheduler: frameScheduler
        )
        let startOptions = TransportStartOptions(
            platform: UndraCore.platformName,
            logLevel: options.logLevel,
            connectTimeout: options.connectTimeout,
            expectedSchemaHash: options.expectedSchemaHash
        )
        let info = try transport.start(inbound: core, options: startOptions)
        if info.schemaHash != options.expectedSchemaHash {
            transport.shutdown()
            throw UndraSchemaMismatchError(expected: options.expectedSchemaHash, got: info.schemaHash)
        }
        core.state.withLock { (current: inout State) -> Void in
            current.schemaHash = info.schemaHash
        }
        core.install(options.adapters)
        return core
    }

    /// The platform name sent to the core in `RuntimeConfig` and `Hello`.
    static var platformName: String {
        #if os(iOS)
        return "ios"
        #elseif os(macOS)
        return "macos"
        #else
        return "apple"
        #endif
    }

    private func install(_ adapters: Adapters) {
        for adapter in adapters.all {
            if let impl = adapter.makePortImpl(core: self) {
                registerPort(adapter.portId, impl)
            }
            state.withLock { (current: inout State) -> Void in
                current.adapters.append(adapter)
            }
            adapter.attach(to: self)
        }
    }

    // MARK: Introspection

    /// How this core is reached.
    public var mode: UndraMode {
        return transport.mode
    }

    /// The schema hash the core reported at load (and the bindings expect).
    public var schemaHash: UInt64 {
        return state.withLock { (current: inout State) -> UInt64 in
            return current.schemaHash
        }
    }

    /// Whether `shutdown()` has run.
    public var isShutDown: Bool {
        return state.withLock { (current: inout State) -> Bool in
            return current.isShutDown
        }
    }

    /// Counters from the core (when reachable) and from this runtime.
    public func stats() -> UndraStats {
        var stats = UndraStats(json: transport.statsJSON() ?? "")
        let host = state.withLock { (current: inout State) -> (live: Int, calls: Int, streams: Int, ports: Int) in
            var calls = 0
            var streams = 0
            for entry in current.pending.values {
                switch entry {
                case .unary, .blocking:
                    calls += 1
                case .stream:
                    streams += 1
                case .reserved:
                    break
                }
            }
            return (live: current.liveObjects, calls: calls, streams: streams, ports: current.ports.count)
        }
        stats.hostLiveHandles = host.live
        stats.hostPendingCalls = host.calls
        stats.hostOpenStreams = host.streams
        stats.hostRegisteredPorts = host.ports
        stats.hostMirroredStores = mirror.registeredCount
        stats.mirror = mirror.stats()
        return stats
    }

    // MARK: Calls

    /// Runs a synchronous method and returns the reply body.
    ///
    /// In process this is `undra_call_sync`: the call runs on the calling thread and returns at
    /// mutex-acquire cost. Over the remote transport there is no inline path, so it blocks the
    /// calling thread until the dev core answers (at most `LoadOptions.blockingCallTimeout`).
    ///
    /// Called on the main thread, it applies the change-sets that arrived before the reply to
    /// the stores before it returns or throws, so the code after it sees the call's effects
    /// (read-your-writes, docs/SPEC.md section 11). From inside a store's `apply` the running
    /// drain applies them instead, in its next round.
    ///
    /// - Parameter method: repeats the method id carried by `target`; `target` is authoritative.
    /// - Throws: `UndraReplyError` for any status other than ok; `UndraProtocolError` for an
    ///   undecodable reply; `UndraTransportError` if the core is shut down or does not answer.
    ///   Generated methods map these with ``UndraCallError/mapped(_:)`` and never expose them.
    public func callSync(_ target: CallTarget, method: UInt32, args: [UInt8]) throws -> [UInt8] {
        UndraCore.checkMethod(target, method)
        // Read-your-writes: on the main thread the call's change-sets are applied before it returns.
        return try mirror.withImmediateDrain {
            let callId = try reserveCallId()
            let payload = UndraCore.makeCallPayload(target, callId: callId, args: args)
            if transport.supportsDirectSync {
                defer {
                    removePending(callId)
                }
                let replyBytes = try transport.callSync(payload)
                let reply = try UndraCore.decodeReply(replyBytes)
                return try UndraCore.unwrap(reply)
            }
            return try blockingCall(callId, payload, operation: "callSync")
        }
    }

    /// Runs a method that may take a while and returns the reply body. Cancelling the calling
    /// task cancels the call in the core (`undra_cancel`) and throws `CancellationError`.
    ///
    /// The change-sets that arrived before the reply are applied to the stores before a caller on
    /// the main actor resumes, whether the call succeeded or failed (read-your-writes, docs/SPEC.md
    /// section 11).
    ///
    /// - Throws: `UndraReplyError` for any status other than ok (a typed error `E` is a reply with
    ///   `status == .error` whose `body` is the encoded `E`); `UndraTransportError` if the core is
    ///   shut down or disconnected before it answers. Generated methods map these with
    ///   ``UndraCallError/mapped(_:domain:)`` and never expose them.
    public func call(_ target: CallTarget, method: UInt32, args: [UInt8]) async throws -> [UInt8] {
        UndraCore.checkMethod(target, method)
        try Task.checkCancellation()
        let slot = CallSlot()
        let callId = try reserveCallId()
        setPending(callId, .unary(slot))
        let payload = UndraCore.makeCallPayload(target, callId: callId, args: args)
        return try await withTaskCancellationHandler(
            operation: {
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<[UInt8], any Error>) in
                    if !slot.install(continuation) {
                        // Cancelled before the call was sent: nothing to tell the core.
                        self.removePending(callId)
                        return
                    }
                    if !self.transport.send(call: payload) {
                        self.removePending(callId)
                        slot.complete(.failure(self.notSent()))
                    }
                }
            },
            onCancel: {
                if slot.cancel() {
                    self.removePending(callId)
                    self.transport.cancel(callId: callId)
                }
            }
        )
    }

    /// Opens a stream and returns its items as encoded bodies.
    ///
    /// The core is granted 16 items of credit when the stream opens and 9 to 16 more each time the
    /// consumer's unread window drops below 8, so the core never runs more than about 16 items
    /// ahead of the consumer. Ending the stream early (cancelling the consuming task, or dropping
    /// the stream) cancels it in the core. A stream that fails throws `UndraReplyError` from
    /// `next()`, in the vocabulary of a failed reply (docs/SPEC.md sections 3.4 and 3.7, ADR-036):
    /// the stream's own typed error arrives as `status == .error` with the encoded `E` in `body`;
    /// a stream the core ended itself as `.cancelled`, one that panicked as `.panic` and one the
    /// core refused as `.badRequest`, each with the body a reply of that status carries.
    public func stream(_ target: CallTarget, method: UInt32, args: [UInt8]) -> AsyncThrowingStream<[UInt8], any Error> {
        return stream(target, method: method, args: args, decode: { $0 })
    }

    /// Opens a stream and returns its items decoded, with the same flow control as
    /// ``stream(_:method:args:)``.
    ///
    /// This is what generated stream methods call. The returned stream pulls an item from the core
    /// only when its consumer asks for the next one, so the credit granted to the core follows what
    /// the consumer has read; copying a stream into another `AsyncThrowingStream` with `yield`
    /// would buffer without bound and lose that.
    ///
    /// - Parameters:
    ///   - decode: turns the body of one item into the element. If it throws, the stream ends with
    ///     that error (after `mapError`) and is cancelled in the core.
    ///   - mapError: turns a failure of the stream (an `UndraReplyError` carrying a typed error, for
    ///     instance) into the error the consumer sees. The default passes it through; generated
    ///     code passes `UndraCallError.mapped(streamFailure:)` or
    ///     `UndraCallError.mapped(streamFailure:domain:)`.
    public func stream<Item: Sendable>(
        _ target: CallTarget,
        method: UInt32,
        args: [UInt8],
        decode: @escaping @Sendable ([UInt8]) throws -> Item,
        mapError: @escaping @Sendable (any Error) -> any Error = { $0 }
    ) -> AsyncThrowingStream<Item, any Error> {
        let consumer = openStream(target, method: method, args: args)
        return AsyncThrowingStream<Item, any Error>(unfolding: {
            do {
                guard let body = try await consumer.next() else {
                    return nil
                }
                do {
                    return try decode(body)
                } catch {
                    consumer.stop()
                    throw error
                }
            } catch {
                throw mapError(error)
            }
        })
    }

    /// Sends the call that opens a stream, grants its initial credit and returns the consumer end.
    private func openStream(_ target: CallTarget, method: UInt32, args: [UInt8]) -> StreamConsumer {
        UndraCore.checkMethod(target, method)
        let callId: UInt32
        do {
            callId = try reserveCallId()
        } catch {
            let channel = StreamChannel(callId: 0, onCredit: { _, _ in }, onClose: { _ in })
            channel.finish(.failed(error))
            return StreamConsumer(channel)
        }
        let channel = StreamChannel(
            callId: callId,
            onCredit: { [weak self] id, credit in
                self?.transport.streamCredit(callId: id, credit: credit)
            },
            onClose: { [weak self] id in
                self?.closeStream(id)
            }
        )
        setPending(callId, .stream(channel))
        let payload = UndraCore.makeCallPayload(target, callId: callId, args: args)
        if transport.send(call: payload) {
            // Registered in the core by now (even a synchronous reply has been delivered), so
            // the initial credit cannot be lost. It is granted from here, not from the reply
            // callback, which may not call into the core.
            channel.noteInitialGrant()
            transport.streamCredit(callId: callId, credit: StreamChannel.initialCredit)
        } else {
            removePending(callId)
            channel.finish(.failed(notSent()))
        }
        return StreamConsumer(channel)
    }

    /// Runs a synchronous constructor and returns the new object's handle. The caller owns the
    /// handle: wrap it in an `UndraObject` (or `UndraStore`), which releases it on `close()`. Like
    /// `callSync`, it applies the change-sets the constructor caused before it returns when called
    /// on the main thread.
    ///
    /// Asynchronous constructors go through `call(.constructor(...))` and decode the handle from
    /// the reply themselves.
    public func construct(type: UInt32, method: UInt32, args: [UInt8]) throws -> UndraHandle {
        let body = try callSync(.constructor(typeId: type, methodId: method), method: method, args: args)
        let handle: UndraHandle
        do {
            handle = try UndraHandle.undraDecoded(from: body)
        } catch let error as WireError {
            throw UndraProtocolError.malformedMessage(context: "constructor result", error: error)
        }
        if handle.isNull {
            throw UndraProtocolError.nullHandle
        }
        return handle
    }

    // MARK: Reporting

    /// Reports a failure that no caller can see (ADR-032): logs it at error level and passes it to
    /// `LoadOptions.onError`. Generated commands and store `apply` call it; it never throws and never
    /// stops the process.
    ///
    /// `error` is mapped the way a throwing call's error is (``UndraCallError/mapped(_:)``), so the
    /// handler always receives an ``UndraCallError``. The handler runs synchronously on the calling
    /// thread. A report made while the handler is running (a handler that calls a failing command) is
    /// only logged.
    ///
    /// - Parameters:
    ///   - error: What the call threw.
    ///   - operation: What failed, as Swift spells it, for example `"Todos.toggle"`.
    public func report(_ error: any Error, operation: String) {
        let mapped = (UndraCallError.mapped(error) as? UndraCallError)
            ?? UndraCallError.malformed(String(describing: error))
        let unhandled = UndraUnhandledError(operation: operation, error: mapped)
        UndraLog.error(unhandled.description)
        guard let handler = onError, !UndraCore.isReporting else {
            return
        }
        UndraCore.$isReporting.withValue(true) {
            handler(unhandled)
        }
    }

    // MARK: Observation and handles

    /// Starts (`on: true`) or stops observing a store's signal (`Observe.allSignals` for all of
    /// them).
    ///
    /// In process the core delivers the current values before `undra_observe` returns, and this
    /// method applies them to the mirror before it returns (with anything else queued), so a
    /// store never exposes its placeholder values. Over the remote transport the initial values
    /// arrive with a later drain.
    @MainActor
    public func observe(_ handle: UndraHandle, signal: UInt32, on: Bool) {
        if isShutDown {
            return
        }
        mirror.withImmediateDrain {
            transport.observe(handle: handle, signal: signal, on: on)
        }
    }

    /// Releases a handle. `UndraObject.close()` calls it; call it directly only for a handle that
    /// was obtained from `construct` and never wrapped.
    public func release(_ handle: UndraHandle) {
        if isShutDown {
            return
        }
        transport.release(handle: handle)
    }

    /// Sends a host-to-core event of an event port (`Connectivity.changed`, `Lifecycle.changed`).
    /// `payload` is the method's parameters, encoded.
    public func event(port: UInt32, method: UInt32, payload: [UInt8]) {
        if isShutDown {
            return
        }
        transport.event(portId: port, methodId: method, payload: payload)
    }

    /// Tells the core that timer `timerId`, armed through the Timer port, has come due.
    public func timerFired(_ timerId: UInt32) {
        if isShutDown {
            return
        }
        transport.timerFired(timerId)
    }

    // MARK: Ports

    /// Registers the host implementation of port `id`, replacing any earlier one.
    ///
    /// `impl` usually comes from a generated `<name>PortImpl(_:)` function. The standard ports
    /// have default adapters (`LoadOptions.adapters`); registering one afterwards overrides it. A
    /// shut-down core (or the placeholder `shared` returns before a core is loaded) ignores the
    /// call and logs a warning: register ports on the core `load(_:)` returned.
    public func registerPort(_ id: UInt32, _ impl: PortImpl) {
        if isShutDown {
            UndraLog.warning("registerPort(\(id)) on a shut-down UndraCore is ignored; register ports on the core UndraCore.load(_:) returned")
            return
        }
        state.withLock { (current: inout State) -> Void in
            current.ports[id] = impl
        }
        transport.registerPort(id)
    }

    // MARK: Snapshots

    /// The persisted state of every store (docs/SPEC.md section 5.9).
    ///
    /// - Throws: `UndraModeError` over the remote transport.
    public func snapshot() throws -> [UInt8] {
        return try transport.snapshot()
    }

    /// Rebuilds the stores from `snapshot`; handles held by the host stay valid. A rejected
    /// snapshot leaves the core unchanged. Called on the main thread, it applies the restored
    /// values to the stores before it returns, like any synchronous call (docs/SPEC.md section 11).
    ///
    /// - Throws: `UndraRestoreError` if the core rejects it; `UndraModeError` over the remote
    ///   transport.
    public func restore(_ snapshot: [UInt8]) throws {
        try mirror.withImmediateDrain {
            try transport.restore(snapshot)
        }
    }

    // MARK: Shutdown

    /// Detaches from the core: stops the adapters, fails every call and stream in flight with
    /// `UndraTransportError.closed`, and (in process) shuts the core down. Idempotent. After it,
    /// a new core can be loaded.
    public func shutdown() {
        let adapters = state.withLock { (current: inout State) -> [any UndraAdapter]? in
            if current.isShutDown {
                return nil
            }
            current.isShutDown = true
            let list = current.adapters
            current.adapters = []
            return list
        }
        guard let adapters = adapters else {
            return
        }
        for adapter in adapters {
            adapter.detach()
        }
        failAllPending(UndraTransportError.closed)
        transport.shutdown()
        mirror.invalidateScheduler()
        UndraCore.sharedSlot.withLock { (slot: inout UndraCore?) -> Void in
            if slot === self {
                slot = nil
            }
        }
    }

    // MARK: Object accounting (called by UndraObject)

    func noteHandleAdopted() {
        state.withLock { (current: inout State) -> Void in
            current.liveObjects += 1
        }
    }

    func noteHandleReleased() {
        state.withLock { (current: inout State) -> Void in
            if current.liveObjects > 0 {
                current.liveObjects -= 1
            }
        }
    }

    // MARK: Call table

    private func reserveCallId() throws -> UInt32 {
        let id = state.withLock { (current: inout State) -> UInt32? in
            if current.isShutDown {
                return nil
            }
            var candidate = current.nextCallId
            var attempts = 0
            repeat {
                candidate = candidate &+ 1
                if candidate == 0 {
                    candidate = 1
                }
                attempts += 1
            } while current.pending[candidate] != nil && attempts < 1_000_000
            current.nextCallId = candidate
            current.pending[candidate] = .reserved
            return candidate
        }
        guard let callId = id else {
            throw UndraTransportError.closed
        }
        return callId
    }

    private func setPending(_ callId: UInt32, _ entry: PendingCall) {
        state.withLock { (current: inout State) -> Void in
            current.pending[callId] = entry
        }
    }

    @discardableResult
    private func removePending(_ callId: UInt32) -> PendingCall? {
        return state.withLock { (current: inout State) -> PendingCall? in
            return current.pending.removeValue(forKey: callId)
        }
    }

    private func closeStream(_ callId: UInt32) {
        removePending(callId)
        transport.cancel(callId: callId)
    }

    private func blockingCall(_ callId: UInt32, _ payload: [UInt8], operation: String) throws -> [UInt8] {
        let box = OneShot<Result<[UInt8], any Error>>()
        setPending(callId, .blocking(box))
        if !transport.send(call: payload) {
            removePending(callId)
            throw notSent()
        }
        guard let result = box.wait(timeoutSeconds: blockingTimeout) else {
            removePending(callId)
            transport.cancel(callId: callId)
            throw UndraTransportError.timedOut(operation: operation)
        }
        return try result.get()
    }

    /// Fails everything in flight with `error`.
    private func failAllPending(_ error: any Error) {
        let entries = state.withLock { (current: inout State) -> [PendingCall] in
            let all = Array(current.pending.values)
            current.pending.removeAll()
            return all
        }
        for entry in entries {
            switch entry {
            case .reserved:
                break
            case .unary(let slot):
                slot.complete(.failure(error))
            case .blocking(let box):
                box.fulfill(.failure(error))
            case .stream(let channel):
                channel.finish(.failed(error))
            }
        }
    }

    /// Asks the core for the current value of a signal whose pending updates the mirror dropped
    /// (a merged keyed patch past the backlog's bounds). Called by a drain on the main actor,
    /// never from a core callback; the value arrives as a change-set.
    @MainActor
    private func resync(_ handle: UndraHandle, signal: UInt32) {
        if isShutDown {
            return
        }
        transport.observe(handle: handle, signal: signal, on: true)
    }

    /// Runs `work` on the deferred queue: the way a callback context asks for something that
    /// calls into the core.
    private func deferToQueue(_ work: @escaping @Sendable () -> Void) {
        deferredQueue.async {
            work()
        }
    }

    // MARK: Encoding helpers

    private static func checkMethod(_ target: CallTarget, _ method: UInt32) {
        switch target {
        case .freeFunction(let id), .objectMethod(_, let id), .constructor(_, let id):
            assert(id == method, "UndraCore: `method` (\(method)) differs from the method id in `target` (\(id))")
        case .lazyListPage:
            break
        }
    }

    static func makeCallPayload(_ target: CallTarget, callId: UInt32, args: [UInt8]) -> [UInt8] {
        var writer = UndraWriter(capacity: 32 + args.count)
        Wire.Call.writeHeader(into: &writer, target: target, callId: callId)
        if case .lazyListPage = target {
            return writer.finish()
        }
        writer.writeRaw(args)
        return writer.finish()
    }

    static func decodeReply(_ bytes: [UInt8]) throws -> Wire.Reply {
        do {
            return try Wire.Reply.decode(bytes)
        } catch let error as WireError {
            throw UndraProtocolError.malformedMessage(context: "reply", error: error)
        }
    }

    /// The reply body of an ok reply, or the error for any other status.
    static func unwrap(_ reply: Wire.Reply) throws -> [UInt8] {
        switch reply.status {
        case .ok:
            return Array(reply.body)
        case .error, .panic, .cancelled, .streamOpened, .badRequest:
            throw UndraReplyError(status: reply.status, body: Array(reply.body))
        }
    }

    static func unaryResult(_ reply: Wire.Reply) -> Result<[UInt8], any Error> {
        do {
            let body = try unwrap(reply)
            return .success(body)
        } catch {
            return .failure(error)
        }
    }

    /// The error for a call the transport did not send. The remote transport refuses to send only
    /// once its connection is closed, and a call that raced with `shutdown()` found the transport
    /// already shut: both are `UndraTransportError.closed`, as for a call in flight when the
    /// connection went. Otherwise the in-process core refused it (`rejection()`).
    private func notSent() -> any Error {
        if transport.mode == .remote || isShutDown {
            return UndraTransportError.closed
        }
        return UndraCore.rejection()
    }

    /// The error for a call the core refused without replying (`undra_call` returned 5).
    static func rejection() -> UndraReplyError {
        var writer = UndraWriter()
        writer.writeString("the core rejected the call without a reply (malformed, duplicate call id, or shut down)")
        return UndraReplyError(status: .badRequest, body: writer.finish())
    }
}

// MARK: - Callbacks from the transport

extension UndraCore: UndraInbound {
    func onReply(callId: UInt32, payload: [UInt8]) {
        let reply: Wire.Reply
        do {
            reply = try Wire.Reply.decode(payload)
        } catch let error as WireError {
            failPending(callId, UndraProtocolError.malformedMessage(context: "reply", error: error))
            return
        } catch {
            failPending(callId, error)
            return
        }
        let entry = state.withLock { (current: inout State) -> PendingCall? in
            guard let found = current.pending[callId] else {
                return nil
            }
            if case .stream = found, reply.status == .streamOpened {
                return found
            }
            current.pending[callId] = nil
            return found
        }
        guard let entry = entry else {
            // A reply for a call that was cancelled or already failed.
            return
        }
        switch entry {
        case .unary, .blocking:
            // Read-your-writes: the change-sets that arrived before this reply are applied before
            // a caller on the main actor resumes (the drain is queued on the main queue first).
            mirror.drainBeforeResuming()
        case .reserved, .stream:
            break
        }
        switch entry {
        case .reserved:
            break
        case .unary(let slot):
            slot.complete(UndraCore.unaryResult(reply))
            if reply.status == .streamOpened {
                cancelDeferred(callId)
            }
        case .blocking(let box):
            box.fulfill(UndraCore.unaryResult(reply))
            if reply.status == .streamOpened {
                cancelDeferred(callId)
            }
        case .stream(let channel):
            switch reply.status {
            case .streamOpened:
                break
            case .ok:
                channel.finish(.failed(UndraProtocolError.notAStream(callId: callId)))
            case .error, .panic, .cancelled, .badRequest:
                channel.finish(.failed(UndraReplyError(status: reply.status, body: Array(reply.body))))
            }
        }
    }

    func onChangeSet(_ payload: [UInt8]) {
        mirror.enqueue(payload)
    }

    func onStreamItem(callId: UInt32, payload: [UInt8]) {
        let item: Wire.StreamItem
        do {
            item = try Wire.StreamItem.decode(payload)
        } catch let error as WireError {
            failPending(callId, UndraProtocolError.malformedMessage(context: "stream item", error: error))
            cancelDeferred(callId)
            return
        } catch {
            failPending(callId, error)
            cancelDeferred(callId)
            return
        }
        let channel = state.withLock { (current: inout State) -> StreamChannel? in
            guard let found = current.pending[callId], case .stream(let channel) = found else {
                return nil
            }
            if item.flag != .item {
                current.pending[callId] = nil
            }
            return channel
        }
        guard let channel = channel else {
            return
        }
        switch item.flag {
        case .item:
            channel.push(Array(item.body))
        case .end:
            channel.finish(.ended)
        case .error:
            channel.finish(.failed(UndraReplyError(status: .error, body: Array(item.body))))
        case .failed:
            // The last item of the stream in the core whatever its body says, so nothing is
            // cancelled even when the body does not decode.
            channel.finish(.failed(UndraCore.streamFailureError(item.body)))
        }
    }

    /// The error a `.failed` stream item ends its stream with (ADR-036): the failed reply it
    /// stands for, an `UndraReplyError` with the item's status and the docs/SPEC.md section 3.4
    /// body of that status, so that it maps exactly as a failed reply does. A body that does not
    /// decode is `UndraProtocolError.malformedMessage`.
    static func streamFailureError(_ body: ArraySlice<UInt8>) -> any Error {
        let failure: Wire.StreamFailure
        do {
            failure = try Wire.StreamFailure.decode(slice: body)
        } catch let error as WireError {
            return UndraProtocolError.malformedMessage(context: "stream failure", error: error)
        } catch {
            return error
        }
        return UndraReplyError(status: failure.status, body: failure.replyBody())
    }

    func onPortCall(portId: UInt32, methodId: UInt32, portCallId: UInt32, args: [UInt8]) -> PortCallOutcome {
        let impl = state.withLock { (current: inout State) -> PortImpl? in
            return current.ports[portId]
        }
        guard let impl = impl else {
            return .unavailable
        }
        switch impl {
        case .sync(let table):
            guard let method = table[methodId] else {
                return .unavailable
            }
            let reply: Wire.PortReply
            do {
                let body = try method(args)
                reply = Wire.PortReply(portCallId: portCallId, status: .ok, body: ArraySlice(body))
            } catch let error as UndraPortError {
                reply = Wire.PortReply(portCallId: portCallId, status: .error, body: ArraySlice(error.body))
            } catch {
                UndraLog.warning("sync port \(portId) method \(methodId) failed: \(error)")
                reply = Wire.PortReply(portCallId: portCallId, status: .unavailable)
            }
            return .sync(reply: reply.encode())
        case .async(let table):
            guard let method = table[methodId] else {
                return .unavailable
            }
            Task { [self] in
                let reply: Wire.PortReply
                do {
                    let body = try await method(args)
                    reply = Wire.PortReply(portCallId: portCallId, status: .ok, body: ArraySlice(body))
                } catch let error as UndraPortError {
                    reply = Wire.PortReply(portCallId: portCallId, status: .error, body: ArraySlice(error.body))
                } catch {
                    UndraLog.warning("async port \(portId) method \(methodId) failed: \(error)")
                    reply = Wire.PortReply(portCallId: portCallId, status: .unavailable)
                }
                if !self.isShutDown {
                    self.transport.portReply(reply.encode())
                }
            }
            return .async
        }
    }

    func onLog(level: UInt8, target: String, message: String) {
        let impl = state.withLock { (current: inout State) -> PortImpl? in
            return current.ports[StandardPorts.Log.portId]
        }
        if case .sync(let table)? = impl, let method = table[StandardPorts.Log.log] {
            var writer = UndraWriter()
            writer.writeU8(level)
            writer.writeString(target)
            writer.writeString(message)
            _ = try? method(writer.finish())
            return
        }
        UndraLog.forward(level: level, target: target, message: message)
    }

    func onDisconnect(_ error: any Error) {
        failAllPending(error)
    }

    /// Fails one call with `error` (for a reply that cannot be decoded).
    private func failPending(_ callId: UInt32, _ error: any Error) {
        let entry = removePending(callId)
        guard let entry = entry else {
            return
        }
        switch entry {
        case .reserved:
            break
        case .unary(let slot):
            slot.complete(.failure(error))
        case .blocking(let box):
            box.fulfill(.failure(error))
        case .stream(let channel):
            channel.finish(.failed(error))
        }
    }

    private func cancelDeferred(_ callId: UInt32) {
        deferToQueue { [self] in
            self.transport.cancel(callId: callId)
        }
    }
}
