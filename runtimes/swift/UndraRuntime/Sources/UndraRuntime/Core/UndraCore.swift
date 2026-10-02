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
//    they never call an entry of the core's table. Work that must call into the core from there
//    goes to `deferredQueue`.
//  * Nothing here holds a lock while calling out (resuming a continuation, calling the transport,
//    running a port method).

import Dispatch
import Foundation

/// One attachment to a Rust core. Load one at startup through the entry generated for the core,
/// `Undra<Namespace>`; the generated bindings use that entry's core unless they are given another.
///
/// ```swift
/// let core = try UndraPlaygroundCore.load()
/// let todos = try TodoStore()            // uses UndraPlaygroundCore.core
/// ```
public final class UndraCore: @unchecked Sendable {
    // MARK: Constants

    /// The C ABI version this runtime implements: the `abi_version` of every core table it loads
    /// (`UNDRA_ABI_VERSION` of `undra.h`, ADR-044).
    static let abiVersion: UInt32 = 2

    /// The Undra protocol version announced in the remote handshake.
    public static let undraVersion = "1.0.0"

    // MARK: State

    private struct State {
        var nextCallId: UInt32 = 0
        var pending: [UInt32: PendingCall] = [:]
        var ports: [UInt32: PortImpl] = [:]
        /// The type of the adapter that registered each port from `LoadOptions.adapters`, for the
        /// log line of an adapter bug. A port registered with `registerPort(_:_:)` has none.
        var portAdapters: [UInt32: String] = [:]
        var adapters: [any UndraAdapter] = []
        var liveObjects = 0
        var isShutDown = false
        var schemaHash: UInt64 = 0
        /// What the connection is doing (``UndraCore/connectionState``).
        var connection: UndraConnectionState = .connecting
        var watchers: [UUID: AsyncStream<UndraConnectionState>.Continuation] = [:]
        /// The signals the app observes, per store: observed again after a reconnect (ADR-051).
        var observed: [UndraHandle: Set<UInt32>] = [:]
        /// The objects the app's constructors made and it has not released: what the server is asked to keep for it.
        var constructed: Set<UndraHandle> = []
        /// Handles released while the connection was down, once per reference: released at the server once it is back.
        var releasedWhileDown: [UndraHandle] = []
        /// Counts the times the connection was lost, so that a replay that a newer loss overtook does not announce a connection.
        var lossEpoch = 0
    }

    private static let sharedSlot = Guarded<UndraCore?>(nil)

    /// The `Log` target of the messages `undra dev` addresses to the developer (ADR-053).
    private static let devNoticeTarget = "undra::dev"

    /// What `shared` (and a generated entry's `core`) returns when no core is loaded: shut down
    /// from the start, over a transport that reaches nothing.
    static let unloaded = UndraCore(transport: UnloadedTransport(), isShutDown: true)

    /// Whether the placeholder's "load a core" message has been logged.
    private static let unloadedWarning = Guarded<Bool>(false)

    /// The change-set mirror: stores register with it and it applies the core's updates on the
    /// main actor, merged, once per display frame. `register(handle) { signal, op, reader in ... }`.
    public let mirror: Mirror

    /// The connection state, for SwiftUI on iOS 17 / macOS 14 and later: an `@Observable` object updated on
    /// the main actor (ADR-051). ``connectionState`` is the same news for any thread. Below iOS 17 use
    /// ``connectionObject``.
    @available(iOS 17, macOS 14, *)
    public var connection: UndraConnection {
        // Made by `init` whenever the OS has it; the fallback is unreachable.
        return existingConnection ?? UndraConnection()
    }

    /// The connection state, for SwiftUI on every iOS and macOS version: an `ObservableObject` with a
    /// `@Published` state, updated on the main actor (ADR-045). The same news as ``connection``.
    public let connectionObject: UndraConnectionObject

    /// The `@Observable` twin of ``connectionObject``, made by `init` where the OS has it (iOS 17 / macOS 14).
    /// It needs iOS 17, so it cannot be a stored property of its own type.
    private let observation: Guarded<(any Sendable)?>

    /// The app's callback implementations this core holds references to (ADR-041), and the bridges that
    /// deliver the core's calls to them.
    public let callbacks: UndraCallbacks

    /// One wrapper per handle (ADR-040): see ``adopt(_:_:)``.
    let identities = ObjectIdentityMap()

    let transport: any UndraTransport
    private let state: Guarded<State>
    private let blockingTimeout: Double
    private let onError: (@Sendable (UndraUnhandledError) -> Void)?
    private let onConnectionChange: (@Sendable (UndraConnectionState) -> Void)?
    private let onDevNotice: (@Sendable (String) -> Void)?
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
        frameScheduler: (any FrameScheduler)? = nil,
        onConnectionChange: (@Sendable (UndraConnectionState) -> Void)? = nil,
        onDevNotice: (@Sendable (String) -> Void)? = nil
    ) {
        self.transport = transport
        self.mirror = Mirror(maxPendingEntries: maxPendingEntries, maxPendingBytes: maxPendingBytes, scheduler: frameScheduler)
        self.blockingTimeout = blockingCallTimeout
        self.onError = onError
        self.onConnectionChange = onConnectionChange
        self.onDevNotice = onDevNotice
        var initial = State(isShutDown: isShutDown)
        if isShutDown {
            initial.connection = .closed(.requested)
        }
        let observable = UndraConnectionObject()
        self.connectionObject = observable
        var twin: (any Sendable)?
        if #available(iOS 17, macOS 14, *) {
            twin = UndraConnection()
        }
        self.observation = Guarded<(any Sendable)?>(twin)
        self.callbacks = UndraCallbacks()
        self.state = Guarded<State>(initial)
        if isShutDown {
            // The placeholder `shared` returns: its observables say so too, not `.connecting`. They are
            // captured, not the core: a state they were promised reaches them even if the core is gone.
            let twin = observation.withLock { (slot: inout (any Sendable)?) -> (any Sendable)? in
                return slot
            }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    observable.state = .closed(.requested)
                    if #available(iOS 17, macOS 14, *), let twin = twin as? UndraConnection {
                        twin.state = .closed(.requested)
                    }
                }
            }
        }
        mirror.setResyncHandler { [weak self] handle, signal in
            self?.resync(handle, signal: signal)
        }
        callbacks.attach(to: self)
    }

    // MARK: Loading

    /// Attaches to a core, checks its schema hash, registers the adapters, and makes the result
    /// `UndraCore.shared` if none is loaded yet.
    ///
    /// Apps call the generated entry of their core instead (`UndraPlaygroundCore.load()`), which
    /// fills in `options.api` and `options.expectedSchemaHash` and makes the result the core of
    /// its bindings. In process, the table is checked before anything in it is called: its C ABI
    /// version, its size and entries, then its schema hash, and only then is the core started.
    /// One core of a namespace runs at a time; cores of different namespaces run side by side.
    ///
    /// - Throws: `UndraSchemaMismatchError` if the core's schema hash is not
    ///   `options.expectedSchemaHash`; `UndraLoadError` if the options lack the table or the hash,
    ///   the table is of another ABI version or unusable, the core is already loaded, or it cannot
    ///   be reached or initialised.
    ///
    /// A remote core is reached with a blocking handshake, so call this once at startup, not on
    /// a hot path.
    @discardableResult
    public static func load(_ options: LoadOptions) throws -> UndraCore {
        if options.expectedSchemaHash == nil {
            throw UndraLoadError.missingSchemaHash
        }
        let transport: any UndraTransport
        switch options.mode {
        case .inproc:
            guard let api = options.api else {
                throw UndraLoadError.missingCoreTable
            }
            transport = InprocTransport(table: try CoreTable(reading: api))
        case .remote(let url):
            transport = try WebSocketTransport(
                urlString: url,
                reconnect: options.reconnect,
                session: UUID().uuidString
            )
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
    /// For app code with one core. Generated code never reads it: its default `ctx` is the core of
    /// its own generated entry (`UndraPlaygroundCore.core`, ADR-044), so an app with several cores
    /// gets the right one everywhere. Using it before a successful `load`, or after
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
                "UndraCore.shared was used while no core is loaded (before a load succeeds, or after shutdown()); calls on it fail with UndraCallError.unavailable(.closed). Load the core at app startup (its generated entry, Undra<Namespace>.load()), before creating any Undra object."
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
        guard let expectedSchemaHash = options.expectedSchemaHash else {
            throw UndraLoadError.missingSchemaHash
        }
        let core = UndraCore(
            transport: transport,
            blockingCallTimeout: options.blockingCallTimeout,
            onError: options.onError,
            maxPendingEntries: options.maxPendingEntries,
            maxPendingBytes: options.maxPendingBytes,
            frameScheduler: frameScheduler,
            onConnectionChange: options.onConnectionChange,
            onDevNotice: options.onDevNotice
        )
        options.onConnectionChange?(.connecting)
        let startOptions = TransportStartOptions(
            platform: UndraCore.platformName,
            logLevel: options.logLevel,
            connectTimeout: options.connectTimeout,
            expectedSchemaHash: expectedSchemaHash
        )
        let info = try transport.start(inbound: core, options: startOptions)
        if info.schemaHash != expectedSchemaHash {
            transport.shutdown()
            throw UndraSchemaMismatchError(expected: expectedSchemaHash, got: info.schemaHash)
        }
        core.state.withLock { (current: inout State) -> Void in
            current.schemaHash = info.schemaHash
        }
        core.install(options.adapters)
        core.setConnectionState(.connected)
        return core
    }

    /// Puts a core of your own under an `UndraCore`: starts `transport`, checks the schema hash it reports against
    /// `options.expectedSchemaHash`, registers the adapters of `options` and returns the core, which also becomes
    /// `UndraCore.shared` if none is loaded. The testing kit's recorded core is made this way; the seam is `package`, so
    /// only code of this package (the kit) can use it.
    package static func attach(transport: any UndraTransport, options: LoadOptions) throws -> UndraCore {
        let core = try connect(transport: transport, options: options)
        sharedSlot.withLock { (slot: inout UndraCore?) -> Void in
            if slot == nil {
                slot = core
            }
        }
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
                register(adapter.portId, impl, adapter: String(describing: type(of: adapter)))
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

    /// What the connection to the core is doing (ADR-051). A remote core is `.reconnecting` while `undra dev` is
    /// unreachable: calls fail at once with ``UndraCallError/unavailable(_:)``, and what was in flight when the
    /// connection dropped failed with it. When it is `.connected` again every store the app observes has been
    /// observed again, so the mirrors converge on the core's current values by themselves. `.closed` is final.
    public var connectionState: UndraConnectionState {
        return state.withLock { (current: inout State) -> UndraConnectionState in
            return current.connection
        }
    }

    /// The connection state now, then every change, ending after `.closed`. For code that is not a SwiftUI view
    /// (see ``connection`` for those).
    public func connectionStates() -> AsyncStream<UndraConnectionState> {
        return AsyncStream { continuation in
            let id = UUID()
            let current = state.withLock { (current: inout State) -> UndraConnectionState in
                if case .closed = current.connection {
                    return current.connection
                }
                current.watchers[id] = continuation
                return current.connection
            }
            continuation.yield(current)
            if case .closed = current {
                continuation.finish()
            }
            continuation.onTermination = { [weak self] _ in
                self?.state.withLock { (current: inout State) -> Void in
                    current.watchers[id] = nil
                }
            }
        }
    }

    /// Records a new connection state and tells whoever listens.
    func setConnectionState(_ next: UndraConnectionState) {
        let watchers = state.withLock { (current: inout State) -> [AsyncStream<UndraConnectionState>.Continuation]? in
            if current.connection == next {
                return nil
            }
            if case .closed = current.connection {
                return nil // final
            }
            current.connection = next
            let list = Array(current.watchers.values)
            if case .closed = next {
                current.watchers = [:]
            }
            return list
        }
        guard let watchers = watchers else {
            return
        }
        for watcher in watchers {
            watcher.yield(next)
            if case .closed = next {
                watcher.finish()
            }
        }
        onConnectionChange?(next)
        // Both observables are captured, not `self`: a connection a view holds hears `.closed` even when
        // the core is released right after it closes (`onConnectionChange` runs before this hop).
        let observable = connectionObject
        let twin = observation.withLock { (slot: inout (any Sendable)?) -> (any Sendable)? in
            return slot
        }
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                observable.state = next
                if #available(iOS 17, macOS 14, *), let twin = twin as? UndraConnection {
                    twin.state = next
                }
            }
        }
    }

    /// ``connection`` if something has asked for it yet.
    @available(iOS 17, macOS 14, *)
    private var existingConnection: UndraConnection? {
        return observation.withLock { (slot: inout (any Sendable)?) -> UndraConnection? in
            return slot as? UndraConnection
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
    ///
    /// `lending` lists the callback instances the arguments carry (``UndraCallbacks/lend(_:)``): when the
    /// call never reaches the core or the core refuses it (status 5), it holds none of them, and they are
    /// given back here; any other outcome means the core owns them (ADR-041).
    public func callSync(_ target: CallTarget, method: UInt32, args: [UInt8], lending: [UInt64?] = []) throws -> [UInt8] {
        UndraCore.checkMethod(target, method)
        let lent = LentInstances(lending, to: callbacks)
        // Read-your-writes: on the main thread the call's change-sets are applied before it returns.
        return try lent.givingBackIfRefused {
            try mirror.withImmediateDrain {
                let callId = try lent.unlessSent { try reserveCallId() }
                let payload = UndraCore.makeCallPayload(target, callId: callId, args: args)
                if transport.supportsDirectSync {
                    defer {
                        removePending(callId)
                    }
                    let replyBytes = try lent.unlessSent { try transport.callSync(payload) }
                    let reply = try UndraCore.decodeReply(replyBytes)
                    return try UndraCore.unwrap(reply)
                }
                return try blockingCall(callId, payload, operation: "callSync", lent: lent)
            }
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
    ///
    /// `lending` lists the callback instances the arguments carry, as for ``callSync(_:method:args:lending:)``.
    public func call(_ target: CallTarget, method: UInt32, args: [UInt8], lending: [UInt64?] = []) async throws -> [UInt8] {
        UndraCore.checkMethod(target, method)
        let lent = LentInstances(lending, to: callbacks)
        return try await lent.givingBackIfRefused {
            try await send(target, args: args, lent: lent)
        }
    }

    /// The body of ``call(_:method:args:lending:)``.
    private func send(_ target: CallTarget, args: [UInt8], lent: LentInstances) async throws -> [UInt8] {
        try lent.unlessSent { try Task.checkCancellation() }
        let slot = CallSlot()
        let callId = try lent.unlessSent { try reserveCallId() }
        setPending(callId, .unary(slot))
        let payload = UndraCore.makeCallPayload(target, callId: callId, args: args)
        return try await withTaskCancellationHandler(
            operation: {
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<[UInt8], any Error>) in
                    if !slot.install(continuation) {
                        // Cancelled before the call was sent: nothing to tell the core.
                        self.removePending(callId)
                        lent.giveBack()
                        return
                    }
                    if !self.transport.send(call: payload) {
                        self.removePending(callId)
                        lent.giveBack()
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
    public func construct(type: UInt32, method: UInt32, args: [UInt8], lending: [UInt64?] = []) throws -> UndraHandle {
        let body = try callSync(.constructor(typeId: type, methodId: method), method: method, args: args, lending: lending)
        let handle: UndraHandle
        do {
            handle = try UndraHandle.undraDecoded(from: body)
        } catch let error as WireError {
            throw UndraProtocolError.malformedMessage(context: "constructor result", error: error)
        }
        if handle.isNull {
            throw UndraProtocolError.nullHandle
        }
        state.withLock { (current: inout State) -> Void in
            current.constructed.insert(handle)
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
    /// only logged, and so is a failure that is a remote core's connection being down
    /// (``UndraCallError/unavailable(_:)`` while ``connectionState`` is `.reconnecting`, or `.closed` for a reason
    /// other than `shutdown()`): the connection state and `LoadOptions.onConnectionChange` already report that,
    /// once, and a command tapped meanwhile is not a second failure to hand to a crash reporter.
    ///
    /// - Parameters:
    ///   - error: What the call threw.
    ///   - operation: What failed, as Swift spells it, for example `"Todos.toggle"`.
    public func report(_ error: any Error, operation: String) {
        let mapped = (UndraCallError.mapped(error) as? UndraCallError)
            ?? UndraCallError.malformed(String(describing: error))
        let unhandled = UndraUnhandledError(operation: operation, error: mapped)
        if isConnectionDown(mapped) {
            UndraLog.warning("\(unhandled.description) (the connection to the core is down: see connectionState)")
            return
        }
        UndraLog.error(unhandled.description)
        guard let handler = onError, !UndraCore.isReporting else {
            return
        }
        UndraCore.$isReporting.withValue(true) {
            handler(unhandled)
        }
    }

    /// Whether `error` is the connection of a remote core being down, which ``connectionState`` already reports. A
    /// core the app shut down itself and an in-process core are not: those are still reported.
    private func isConnectionDown(_ error: UndraCallError) -> Bool {
        guard case .unavailable(let reason) = error, transport.mode == .remote else {
            return false
        }
        if case .connectionLost = reason {
            return true
        }
        switch connectionState {
        case .reconnecting:
            return true
        case .closed(let why):
            return why != .requested
        case .connecting, .connected:
            return false
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
        // Remembered, so that a reconnect observes it again (a call made while reconnecting is not lost either).
        state.withLock { (current: inout State) -> Void in
            if on {
                current.observed[handle, default: []].insert(signal)
            } else if signal == Observe.allSignals {
                current.observed[handle] = nil
            } else {
                current.observed[handle]?.remove(signal)
                if current.observed[handle]?.isEmpty == true {
                    current.observed[handle] = nil
                }
            }
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
        let reconnecting = state.withLock { (current: inout State) -> Bool in
            current.observed[handle] = nil
            current.constructed.remove(handle)
            if case .reconnecting = current.connection {
                // The server keeps the object for us (ADR-051); it is released when the connection is back.
                current.releasedWhileDown.append(handle)
                return true
            }
            return false
        }
        if !reconnecting {
            transport.release(handle: handle)
        }
    }

    /// Gives back one reference to `handle` that a reply carried while a live wrapper already owns one
    /// (``adopt(_:_:)``): the wrapper's own bookkeeping (what it observes, that the host holds it) stays.
    func releaseExtraReference(_ handle: UndraHandle) {
        if isShutDown {
            return
        }
        let reconnecting = state.withLock { (current: inout State) -> Bool in
            if case .reconnecting = current.connection {
                current.releasedWhileDown.append(handle)
                return true
            }
            return false
        }
        if !reconnecting {
            transport.release(handle: handle)
        }
    }

    /// Records that the host holds `handle` (a wrapper owns a reference), so that a remote core asks
    /// the server to keep it across a reconnect.
    func noteHeld(_ handle: UndraHandle) {
        state.withLock { (current: inout State) -> Void in
            current.constructed.insert(handle)
        }
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
        register(id, impl, adapter: nil)
    }

    /// `registerPort(_:_:)`, remembering which adapter type (if any) the implementation came from.
    private func register(_ id: UInt32, _ impl: PortImpl, adapter: String?) {
        if isShutDown {
            UndraLog.warning("registerPort(\(id)) on a shut-down UndraCore is ignored; register ports on the core UndraCore.load(_:) returned")
            return
        }
        state.withLock { (current: inout State) -> Void in
            current.ports[id] = impl
            current.portAdapters[id] = adapter
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
    /// A snapshot taken by another build restores when its stores' types are unchanged or migrate
    /// by name (ADR-037); one that cannot is refused as a whole with
    /// ``UndraRestoreError/incompatible``, and a store type this build no longer has is left out.
    ///
    /// - Throws: `UndraRestoreError` if the core rejects it (``UndraRestoreError/badSnapshot``,
    ///   ``UndraRestoreError/incompatible``, ...); `UndraModeError` over the remote transport.
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
        closeForGood(.requested, failing: UndraTransportError.closed)
    }

    /// Ends this core for good for `reason`, failing what is in flight with `error`.
    private func closeForGood(_ reason: UndraClosedReason, failing error: any Error) {
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
        setConnectionState(.closed(reason))
        for adapter in adapters {
            adapter.detach()
        }
        failAllPending(error)
        callbacks.close()
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

    private func blockingCall(_ callId: UInt32, _ payload: [UInt8], operation: String, lent: LentInstances? = nil) throws -> [UInt8] {
        let box = OneShot<Result<[UInt8], any Error>>()
        setPending(callId, .blocking(box))
        if !transport.send(call: payload) {
            removePending(callId)
            lent?.giveBack()
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
        if case .reconnecting = connectionState {
            return UndraTransportError.connectionLost(reason: "the dev server is unreachable; reconnecting")
        }
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
    package func onReply(callId: UInt32, payload: [UInt8]) {
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

    package func onChangeSet(_ payload: [UInt8]) {
        mirror.enqueue(payload)
    }

    package func onStreamItem(callId: UInt32, payload: [UInt8]) {
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

    package func onPortCall(portId: UInt32, methodId: UInt32, portCallId: UInt32, args: [UInt8]) -> PortCallOutcome {
        // A callback interface's port (ADR-041): queued for the app's implementation, never run here.
        if let outcome = callbacks.route(portId: portId, methodId: methodId, portCallId: portCallId, args: args) {
            return outcome
        }
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
                logUntypedPortFailure(portId: portId, methodId: methodId, error: error)
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
                    self.logUntypedPortFailure(portId: portId, methodId: methodId, error: error)
                    reply = Wire.PortReply(portCallId: portCallId, status: .unavailable)
                }
                if !self.isShutDown {
                    self.transport.portReply(reply.encode())
                }
            }
            return .async
        }
    }

    /// A port method threw something other than `UndraPortError`: the bridge answers "unavailable"
    /// (port status 2), as it always has, and says so at ERROR level, naming the port, the method
    /// and the adapter, because a port reports its failures as typed errors (status 1, ADR-049)
    /// and anything else is a bug in the adapter (or arguments it could not decode).
    func logUntypedPortFailure(portId: UInt32, methodId: UInt32, error: any Error) {
        let adapter = state.withLock { (current: inout State) -> String? in
            return current.portAdapters[portId]
        }
        let port = StandardPorts.describe(portId: portId)
        let method = StandardPorts.describe(methodId: methodId)
        let source = adapter.map { "adapter \($0) (\(port))" } ?? "the \(port) implementation registered with registerPort"
        UndraLog.error(
            "\(source) failed \(method) with an untyped error (\(type(of: error)): \(error)); "
                + "answered \"unavailable\" (port status 2). A port method reports a failure by throwing "
                + "UndraPortError carrying the encoded error of its signature; anything else is a bug in the adapter."
        )
    }

    package func onLog(level: UInt8, target: String, message: String) {
        // Only `undra dev` says things to the developer; a core in this process never does (ADR-053).
        if target == Self.devNoticeTarget, transport.mode == .remote, let notify = onDevNotice {
            deferToQueue {
                notify(message)
            }
        }
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

    package func onDisconnect(_ error: any Error) {
        let reason: UndraClosedReason
        if let mismatch = error as? UndraSchemaMismatchError {
            reason = .schemaMismatch(expected: mismatch.expected, got: mismatch.got)
        } else if error is UndraSessionLostError {
            reason = .sessionLost
        } else {
            reason = .failed(String(describing: error))
        }
        closeForGood(reason, failing: error)
    }

    package func onReconnecting(attempt: Int, error: any Error) {
        let proceed = state.withLock { (current: inout State) -> Bool in
            if current.isShutDown {
                return false
            }
            if attempt == 1 {
                current.lossEpoch += 1
            }
            return true
        }
        guard proceed else {
            return
        }
        if attempt == 1 {
            failAllPending(UndraTransportError.connectionLost(reason: "\(error); reconnecting"))
        }
        setConnectionState(.reconnecting(attempt: attempt))
    }

    package func onReconnected() {
        let epoch = state.withLock { (current: inout State) -> Int in
            return current.lossEpoch
        }
        // On a queue of ours: the transport's callback must not call back into it.
        deferToQueue { [self] in
            self.observeAgain(epoch: epoch)
        }
    }

    package func holdsObjects() -> Bool {
        return state.withLock { (current: inout State) -> Bool in
            return !current.constructed.isEmpty
        }
    }

    /// The connection is back: release what was released meanwhile and observe what the app observes again. The core
    /// answers each observation with the current values, so every mirror converges by itself.
    private func observeAgain(epoch: Int) {
        let work = state.withLock { (current: inout State) -> (released: [UndraHandle], observed: [(UndraHandle, [UInt32])])? in
            if current.isShutDown || current.lossEpoch != epoch {
                return nil
            }
            return (Array(current.releasedWhileDown), current.observed.map { ($0.key, Array($0.value)) })
        }
        guard let work = work else {
            return
        }
        for handle in work.released {
            transport.release(handle: handle)
        }
        state.withLock { (current: inout State) -> Void in
            current.releasedWhileDown.removeFirst(Swift.min(work.released.count, current.releasedWhileDown.count))
        }
        for (handle, signals) in work.observed {
            for signal in signals {
                transport.observe(handle: handle, signal: signal, on: true)
            }
        }
        let stillCurrent = state.withLock { (current: inout State) -> Bool in
            return !current.isShutDown && current.lossEpoch == epoch
        }
        if stillCurrent {
            setConnectionState(.connected)
        }
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
