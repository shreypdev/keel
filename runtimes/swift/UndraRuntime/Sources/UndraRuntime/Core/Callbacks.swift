// Host callback interfaces (ADR-041): the registry of the app's implementations a core holds
// references to, and the bridge that delivers the core's calls to them.
//
// A `#[undra::callback]` trait is a port with many instances. The app implements the generated
// protocol; passing an implementation to a method lends it to the core under an instance handle the
// host chooses (`UndraCallbacks.lend`), one reference per crossing. The core calls an instance with a
// port call on the trait's port id whose arguments start with the instance, and gives references back
// with the reserved `__release` method.
//
// The port callback runs on the core's thread, possibly under its lock, so it only queues (SPEC 6 host
// contract 2 and 4): a main-thread interface's calls join the mirror's queue in arrival order with the
// change-sets (ADR-031) and run at the drain; a background interface's calls run on a serial queue of
// their instance. Asynchronous methods answer through `port_reply`.

import Dispatch
import Foundation

// MARK: - The bridge of one interface (generated)

/// How the runtime delivers the core's calls of one callback interface to the app's implementations.
///
/// The bindings generate one per `#[undra::callback]` trait and their core's entry installs them
/// (``UndraCallbacks/install(_:)``) when it loads; an app never builds one.
public struct UndraCallbackInterface: Sendable {
    /// The trait's name, for reports (`Reporter`).
    public let name: String
    /// The trait's port id (`fnv1a32("port.<Trait>")`).
    public let portId: UInt32
    /// The reserved `__release` method: the core gives one reference to an instance back.
    public let releaseInstance: UInt32
    /// The reserved `__cancel` method: the core no longer waits for an asynchronous call.
    public let cancelCall: UInt32
    /// The trait's methods by method id.
    public let methods: [UInt32: UndraCallbackMethod]

    /// Describes an interface: its name, port id, reserved method ids and methods.
    public init(
        name: String,
        portId: UInt32,
        releaseInstance: UInt32,
        cancelCall: UInt32,
        methods: [UInt32: UndraCallbackMethod]
    ) {
        self.name = name
        self.portId = portId
        self.releaseInstance = releaseInstance
        self.cancelCall = cancelCall
        self.methods = methods
    }

    /// Whether the interface is delivered off the main thread (`#[undra::callback(background)]`).
    var isBackground: Bool {
        return methods.values.contains { $0.isBackground }
    }
}

/// One method of a callback interface: how to decode its arguments and call an implementation.
///
/// The generated bridge builds them with the four factories, one per delivery and shape:
/// ``notify(_:coalesce:_:)`` and ``call(_:_:)`` for a main-thread interface,
/// ``notifyInBackground(_:_:)`` and ``callInBackground(_:_:)`` for a background one. Each closure
/// decodes the arguments from the reader (and checks it is finished) and calls the implementation; an
/// asynchronous one returns the encoded result, or throws `UndraPortError` carrying the method's
/// encoded error.
public struct UndraCallbackMethod: Sendable {
    enum Kind: Sendable {
        case notify(@MainActor @Sendable (AnyObject, inout UndraReader) throws -> Void)
        case call(@MainActor @Sendable (AnyObject, inout UndraReader) async throws -> [UInt8])
        case notifyInBackground(@Sendable (AnyObject, inout UndraReader) throws -> Void)
        case callInBackground(@Sendable (AnyObject, inout UndraReader) async throws -> [UInt8])
    }

    /// The method's name, for reports (`progress`).
    public let name: String
    /// Whether only the newest pending call per instance is delivered (`#[undra(coalesce)]`).
    public let coalesce: Bool
    let kind: Kind

    var isBackground: Bool {
        switch kind {
        case .notify, .call:
            return false
        case .notifyInBackground, .callInBackground:
            return true
        }
    }

    /// A fire-and-forget method of a main-thread interface, run by the mirror's drain.
    public static func notify<Implementation>(
        _ name: String,
        coalesce: Bool = false,
        _ run: @escaping @MainActor @Sendable (Implementation, inout UndraReader) throws -> Void
    ) -> UndraCallbackMethod {
        return UndraCallbackMethod(name: name, coalesce: coalesce, kind: .notify { target, reader in
            try run(try UndraCallbacks.mainImplementation(target, as: Implementation.self), &reader)
        })
    }

    /// An asynchronous method of a main-thread interface, started by the mirror's drain on the main actor.
    public static func call<Implementation>(
        _ name: String,
        _ run: @escaping @MainActor @Sendable (Implementation, inout UndraReader) async throws -> [UInt8]
    ) -> UndraCallbackMethod {
        return UndraCallbackMethod(name: name, coalesce: false, kind: .call { target, reader in
            return try await run(try UndraCallbacks.mainImplementation(target, as: Implementation.self), &reader)
        })
    }

    /// A fire-and-forget method of a background interface, run on its instance's serial queue.
    public static func notifyInBackground<Implementation>(
        _ name: String,
        _ run: @escaping @Sendable (Implementation, inout UndraReader) throws -> Void
    ) -> UndraCallbackMethod {
        return UndraCallbackMethod(name: name, coalesce: false, kind: .notifyInBackground { target, reader in
            try run(try UndraCallbacks.implementation(target, as: Implementation.self), &reader)
        })
    }

    /// An asynchronous method of a background interface, started from its instance's serial queue.
    public static func callInBackground<Implementation>(
        _ name: String,
        _ run: @escaping @Sendable (Implementation, inout UndraReader) async throws -> [UInt8]
    ) -> UndraCallbackMethod {
        return UndraCallbackMethod(name: name, coalesce: false, kind: .callInBackground { target, reader in
            return try await run(try UndraCallbacks.implementation(target, as: Implementation.self), &reader)
        })
    }
}

/// A generated weak wrapper of a main-thread callback interface (`WeakUploadListener`): the runtime
/// calls the wrapper's target directly while it lives and answers for it once it is gone (a
/// fire-and-forget call does nothing; an asynchronous one is answered as unavailable).
@MainActor
public protocol UndraWeakMainCallback: AnyObject {
    /// The implementation the calls go to, while it lives.
    var undraTarget: AnyObject? { get }
}

/// A generated weak wrapper of a background callback interface; see ``UndraWeakMainCallback``.
public protocol UndraWeakCallback: AnyObject, Sendable {
    /// The implementation the calls go to, while it lives.
    var undraTarget: AnyObject? { get }
}

/// What the bridge throws for a weak wrapper whose target is gone: answered as unavailable, not reported.
struct UndraCallbackTargetGone: Error {}

// MARK: - The registry

/// The app's callback implementations one core holds references to (ADR-041): `UndraCore.callbacks`.
///
/// Passing an implementation to a generated method lends it to the core (``lend(_:)``): the core gets
/// its instance handle and owns one reference per crossing, and the registry holds the implementation
/// strongly until the core has given every reference back (``release(_:)``, the core's `__release`).
/// The same implementation passed twice is the same instance with two references. To keep a listener
/// from keeping alive what holds it, pass the generated weak wrapper (`WeakUploadListener(self)`) or
/// close the subscription object the method returned.
public final class UndraCallbacks: @unchecked Sendable {
    private struct Entry {
        let object: AnyObject
        var references: Int
        /// The serial queue of a background instance, made with its first call.
        var lane: DispatchQueue?
    }

    private struct CallKey: Hashable {
        let instance: UInt64
        let portCallId: UInt32
    }

    /// An asynchronous call between the core's port call and its reply.
    private enum PendingCall {
        /// Queued, not started: a cancel drops it.
        case queued
        /// Running: a cancel cancels the task.
        case running(Task<Void, Never>)
    }

    private struct State {
        weak var core: UndraCore?
        var nextInstance: UInt64 = 0
        var entries: [UInt64: Entry] = [:]
        var instances: [ObjectIdentifier: UInt64] = [:]
        var interfaces: [UInt32: UndraCallbackInterface] = [:]
        var calls: [CallKey: PendingCall] = [:]
        var isClosed = false
    }

    private let state = Guarded<State>(State())

    init() {}

    func attach(to core: UndraCore) {
        state.withLock { (current: inout State) -> Void in
            current.core = core
        }
    }

    // MARK: Interfaces

    /// Registers the bridges of the core's callback interfaces, one port each. The generated entry of the
    /// core does it when it loads, before any call can pass an implementation.
    public func install(_ interfaces: [UndraCallbackInterface]) {
        let core = state.withLock { (current: inout State) -> UndraCore? in
            for interface in interfaces {
                current.interfaces[interface.portId] = interface
            }
            return current.core
        }
        guard let core = core, !core.isShutDown else {
            return
        }
        for interface in interfaces {
            core.transport.registerPort(interface.portId)
        }
    }

    // MARK: References

    /// Lends `implementation` to the core for one crossing and returns its instance handle: a new one
    /// (non-zero, never reused by this core) the first time, the same one while the core still holds a
    /// reference. Generated code calls it for each callback argument; the reference is the core's once the
    /// call is sent.
    public func lend(_ implementation: AnyObject) -> UInt64 {
        let identity = ObjectIdentifier(implementation)
        return state.withLock { (current: inout State) -> UInt64 in
            if let instance = current.instances[identity], current.entries[instance] != nil {
                current.entries[instance]?.references += 1
                return instance
            }
            current.nextInstance += 1
            let instance = current.nextInstance
            current.entries[instance] = Entry(object: implementation, references: 1, lane: nil)
            current.instances[identity] = instance
            return instance
        }
    }

    /// Takes back one reference that was lent but never reached the core (the call was refused, status 5,
    /// or not sent).
    public func giveBack(_ instance: UInt64) {
        drop(instance, by: "the host")
    }

    /// Takes back one reference the core gave back (its `__release`). The implementation is let go when
    /// no reference is left. Releasing an instance that holds none is logged, never fatal.
    public func release(_ instance: UInt64) {
        drop(instance, by: "the core")
    }

    /// How many implementations the core holds references to.
    public var liveCount: Int {
        return state.withLock { (current: inout State) -> Int in
            return current.entries.count
        }
    }

    /// How many references to `implementation` the core holds (0 when it holds none).
    public func count(of implementation: AnyObject) -> Int {
        let identity = ObjectIdentifier(implementation)
        return state.withLock { (current: inout State) -> Int in
            guard let instance = current.instances[identity] else {
                return 0
            }
            return current.entries[instance]?.references ?? 0
        }
    }

    private func drop(_ instance: UInt64, by whom: String) {
        let outcome = state.withLock { (current: inout State) -> (known: Bool, closed: Bool) in
            guard var entry = current.entries[instance] else {
                return (known: false, closed: current.isClosed)
            }
            entry.references -= 1
            if entry.references > 0 {
                current.entries[instance] = entry
            } else {
                current.entries[instance] = nil
                current.instances[ObjectIdentifier(entry.object)] = nil
            }
            return (known: true, closed: current.isClosed)
        }
        if !outcome.known && !outcome.closed {
            UndraLog.error("callback instance \(instance) was given back by \(whom) more often than it was lent; ignored (ADR-041: the registry counts per instance and never lets an implementation go early)")
        }
    }

    /// Lets every implementation go and cancels what runs: the core shut down or its connection is gone
    /// for good (its proxies answer unavailable until they are dropped).
    func close() {
        let running = state.withLock { (current: inout State) -> [Task<Void, Never>] in
            current.isClosed = true
            current.entries = [:]
            current.instances = [:]
            var tasks: [Task<Void, Never>] = []
            for call in current.calls.values {
                if case .running(let task) = call {
                    tasks.append(task)
                }
            }
            current.calls = [:]
            return tasks
        }
        for task in running {
            task.cancel()
        }
    }

    // MARK: Routing the core's calls

    /// Queues the core's call of a callback port for the app's implementation and returns at once (the
    /// port callback must not run app code); `nil` when `portId` is not a callback interface's.
    func route(portId: UInt32, methodId: UInt32, portCallId: UInt32, args: [UInt8]) -> PortCallOutcome? {
        let found = state.withLock { (current: inout State) -> (interface: UndraCallbackInterface, core: UndraCore?)? in
            guard let interface = current.interfaces[portId] else {
                return nil
            }
            return (interface: interface, core: current.core)
        }
        guard let (interface, core) = found else {
            return nil
        }
        guard let core = core else {
            return .unavailable
        }
        var reader = UndraReader(args)
        guard let instance = try? reader.readU64() else {
            UndraLog.error("a call of the callback interface \(interface.name) without an instance; answered unavailable")
            return .unavailable
        }
        if methodId == interface.releaseInstance {
            scheduleRelease(instance, interface: interface, core: core)
            return .async
        }
        if methodId == interface.cancelCall {
            if let call = try? reader.readU32() {
                cancel(CallKey(instance: instance, portCallId: call))
            }
            return .async
        }
        guard let method = interface.methods[methodId] else {
            UndraLog.error("the core called an unknown method (\(methodId)) of the callback interface \(interface.name); answered unavailable")
            return .unavailable
        }
        let operation = "\(interface.name).\(method.name)"
        let arguments = Array(args.dropFirst(8))
        switch method.kind {
        case .notify(let run):
            guard let target = target(of: instance, lane: false)?.object else {
                return .unavailable
            }
            let box = TargetBox(target)
            let coalesce = method.coalesce ? MirrorInvocation.CoalesceKey(instance: instance, method: methodId) : nil
            core.mirror.enqueue(MirrorInvocation(coalesce: coalesce, bytes: arguments.count) { [weak core] in
                var reader = UndraReader(arguments)
                do {
                    try run(box.object, &reader)
                } catch is UndraCallbackTargetGone {
                    // A weak wrapper whose target is gone: nothing to tell.
                } catch {
                    core?.report(error, operation: operation)
                }
            })
            return .async
        case .call(let run):
            guard portCallId != 0, let target = target(of: instance, lane: false)?.object else {
                return .unavailable
            }
            let key = CallKey(instance: instance, portCallId: portCallId)
            queue(key)
            let box = TargetBox(target)
            core.mirror.enqueue(MirrorInvocation(coalesce: nil, bytes: arguments.count) { [weak self] in
                guard let self = self else {
                    return
                }
                self.start(key, operation: operation) { @MainActor () async throws -> [UInt8] in
                    var reader = UndraReader(arguments)
                    return try await run(box.object, &reader)
                }
            })
            return .async
        case .notifyInBackground(let run):
            guard let found = target(of: instance, lane: true), let lane = found.lane else {
                return .unavailable
            }
            let box = TargetBox(found.object)
            lane.async { [weak core] in
                var reader = UndraReader(arguments)
                do {
                    try run(box.object, &reader)
                } catch is UndraCallbackTargetGone {
                } catch {
                    core?.report(error, operation: operation)
                }
            }
            return .async
        case .callInBackground(let run):
            guard portCallId != 0, let found = target(of: instance, lane: true), let lane = found.lane else {
                return .unavailable
            }
            let key = CallKey(instance: instance, portCallId: portCallId)
            queue(key)
            let box = TargetBox(found.object)
            lane.async { [weak self] in
                self?.startInBackground(key, operation: operation) { () async throws -> [UInt8] in
                    var reader = UndraReader(arguments)
                    return try await run(box.object, &reader)
                }
            }
            return .async
        }
    }

    /// The implementation of `instance` (and, for a background interface, its serial queue).
    private func target(of instance: UInt64, lane: Bool) -> (object: AnyObject, lane: DispatchQueue?)? {
        return state.withLock { (current: inout State) -> (object: AnyObject, lane: DispatchQueue?)? in
            guard var entry = current.entries[instance] else {
                return nil
            }
            if lane && entry.lane == nil {
                entry.lane = DispatchQueue(label: "dev.undra.callback.\(instance)")
                current.entries[instance] = entry
            }
            return (object: entry.object, lane: entry.lane)
        }
    }

    /// The core's `__release`: takes effect when the calls queued before it have been delivered.
    private func scheduleRelease(_ instance: UInt64, interface: UndraCallbackInterface, core: UndraCore) {
        if interface.isBackground {
            if let lane = target(of: instance, lane: true)?.lane {
                lane.async { [weak self] in
                    self?.release(instance)
                }
            } else {
                release(instance)
            }
            return
        }
        core.mirror.enqueue(MirrorInvocation(coalesce: nil, bytes: 8, isCallback: false) { [weak self] in
            self?.release(instance)
        })
    }

    // MARK: Asynchronous calls

    private func queue(_ key: CallKey) {
        state.withLock { (current: inout State) -> Void in
            current.calls[key] = .queued
        }
    }

    /// The core's `__cancel`: drops a queued call, or cancels the task running it. A late answer is
    /// not sent (the core discards it anyway).
    private func cancel(_ key: CallKey) {
        let running = state.withLock { (current: inout State) -> Task<Void, Never>? in
            let call = current.calls.removeValue(forKey: key)
            if case .running(let task)? = call {
                return task
            }
            return nil
        }
        // Not here: `Task.cancel()` runs the task's cancellation handlers (`withTaskCancellationHandler`'s
        // `onCancel`, app code) on the thread that cancels, and this is the core's port callback, which may hold
        // the core lock (SPEC 6, host contract 2 and 4). Kotlin cancels from another thread for the same reason.
        if let running {
            DispatchQueue.global().async {
                running.cancel()
            }
        }
    }

    /// Whether `key` is still waiting to start; a cancelled call is not.
    private func stillQueued(_ key: CallKey) -> Bool {
        return state.withLock { (current: inout State) -> Bool in
            if case .queued? = current.calls[key] {
                return true
            }
            return false
        }
    }

    @MainActor
    private func start(_ key: CallKey, operation: String, _ body: @escaping @MainActor @Sendable () async throws -> [UInt8]) {
        guard stillQueued(key) else {
            return
        }
        let task = Task { @MainActor [weak self] in
            let reply = await UndraCallbacks.answer(body)
            self?.finish(key, operation: operation, reply)
        }
        markRunning(key, task)
    }

    private func startInBackground(_ key: CallKey, operation: String, _ body: @escaping @Sendable () async throws -> [UInt8]) {
        guard stillQueued(key) else {
            return
        }
        let task = Task { [weak self] in
            let reply = await UndraCallbacks.answer(body)
            self?.finish(key, operation: operation, reply)
        }
        markRunning(key, task)
    }

    private func markRunning(_ key: CallKey, _ task: Task<Void, Never>) {
        let cancelled = state.withLock { (current: inout State) -> Bool in
            guard case .queued? = current.calls[key] else {
                // Finished already, or cancelled in between.
                return current.calls[key] == nil
            }
            current.calls[key] = .running(task)
            return false
        }
        if cancelled {
            task.cancel()
        }
    }

    /// How an asynchronous implementation ended.
    private enum Answer: Sendable {
        case value([UInt8])
        case typed([UInt8])
        case gone
        case failed(any Error)
    }

    private static func answer(_ body: () async throws -> [UInt8]) async -> Answer {
        do {
            return .value(try await body())
        } catch let error as UndraPortError {
            return .typed(error.body)
        } catch is UndraCallbackTargetGone {
            return .gone
        } catch {
            return .failed(error)
        }
    }

    /// Answers the core: status 0 with the value, 1 with the method's own error, 2 for anything else
    /// (reported, unless it was a weak wrapper's missing target). Nothing is sent for a cancelled call.
    private func finish(_ key: CallKey, operation: String, _ answer: Answer) {
        let found = state.withLock { (current: inout State) -> (wanted: Bool, core: UndraCore?) in
            let wanted = current.calls.removeValue(forKey: key) != nil
            return (wanted: wanted, core: current.core)
        }
        guard found.wanted, let core = found.core else {
            return
        }
        let reply: Wire.PortReply
        switch answer {
        case .value(let body):
            reply = Wire.PortReply(portCallId: key.portCallId, status: .ok, body: ArraySlice(body))
        case .typed(let body):
            reply = Wire.PortReply(portCallId: key.portCallId, status: .error, body: ArraySlice(body))
        case .gone:
            reply = Wire.PortReply(portCallId: key.portCallId, status: .unavailable)
        case .failed(let error):
            core.report(error, operation: operation)
            reply = Wire.PortReply(portCallId: key.portCallId, status: .unavailable)
        }
        if !core.isShutDown {
            core.transport.portReply(reply.encode())
        }
    }

    // MARK: Implementations

    /// The implementation of a main-thread interface a call goes to: `target`, or a weak wrapper's target.
    @MainActor
    static func mainImplementation<Implementation>(_ target: AnyObject, as type: Implementation.Type) throws -> Implementation {
        var resolved = target
        if let weak = target as? any UndraWeakMainCallback {
            guard let inner = weak.undraTarget else {
                throw UndraCallbackTargetGone()
            }
            resolved = inner
        }
        guard let implementation = resolved as? Implementation else {
            throw UndraCallError.malformed("a callback instance is not a \(Implementation.self)")
        }
        return implementation
    }

    /// The implementation of a background interface a call goes to: `target`, or a weak wrapper's target.
    static func implementation<Implementation>(_ target: AnyObject, as type: Implementation.Type) throws -> Implementation {
        var resolved = target
        if let weak = target as? any UndraWeakCallback {
            guard let inner = weak.undraTarget else {
                throw UndraCallbackTargetGone()
            }
            resolved = inner
        }
        guard let implementation = resolved as? Implementation else {
            throw UndraCallError.malformed("a callback instance is not a \(Implementation.self)")
        }
        return implementation
    }

    /// What a generated weak wrapper's asynchronous method does when app code calls it after its target is
    /// gone and its error type has no `Unavailable` variant to throw (typed throws can throw nothing else):
    /// there is no value to return and no error to throw, so it stops the process with a message instead of
    /// never returning (a task that waits for ever, unnoticed). The core never calls it in that state: the
    /// runtime resolves the target first and answers unavailable for a gone one.
    public static func targetGone<Value>() async -> Value {
        UndraLog.error("a weak callback wrapper was called after its target was gone (the core's calls are answered as unavailable without calling it)")
        fatalError("a weak callback wrapper's asynchronous method was called after its target was gone, and its error type has no `Unavailable` variant to throw; keep the target alive, or give the error type one")
    }
}

/// An implementation carried to the queue that runs it. The registry keeps it alive meanwhile; it is
/// only touched on the delivery's own executor.
private final class TargetBox: @unchecked Sendable {
    let object: AnyObject

    init(_ object: AnyObject) {
        self.object = object
    }
}

// MARK: - Mirror invocations

/// A host callback call queued with the change-sets (ADR-041 decision 6): the drain runs it in arrival
/// order, after the change-sets that came before it and never folded with them.
final class MirrorInvocation: Sendable {
    /// A coalescing method's calls of one instance: only the newest pending one is delivered.
    struct CoalesceKey: Hashable, Sendable {
        let instance: UInt64
        let method: UInt32
    }

    let coalesce: CoalesceKey?
    /// What the queue's byte bound counts for it: its arguments.
    let bytes: Int
    /// Whether it calls the app (counted in `MirrorStats.callbacksDelivered`), not just the registry.
    let isCallback: Bool
    private let run: @MainActor @Sendable () -> Void

    init(coalesce: CoalesceKey?, bytes: Int, isCallback: Bool = true, _ run: @escaping @MainActor @Sendable () -> Void) {
        self.coalesce = coalesce
        self.bytes = bytes
        self.isCallback = isCallback
        self.run = run
    }

    /// Runs it; `true` when it called the app.
    @MainActor
    func deliver() -> Bool {
        run()
        return isCallback
    }
}

// MARK: - Lent instances of a call

/// The callback instances one call's arguments carry: given back to the registry, once, when the call
/// never reaches the core or the core refuses it (status 5); otherwise the core owns them.
struct LentInstances: Sendable {
    private let instances: [UInt64]
    private let callbacks: UndraCallbacks?
    private let returned: Guarded<Bool>?

    init(_ lending: [UInt64?], to callbacks: UndraCallbacks) {
        let list = lending.compactMap { $0 }
        if list.isEmpty {
            self.instances = []
            self.callbacks = nil
            self.returned = nil
        } else {
            self.instances = list
            self.callbacks = callbacks
            self.returned = Guarded<Bool>(false)
        }
    }

    /// Gives the instances back (once).
    func giveBack() {
        guard let callbacks = callbacks, let returned = returned else {
            return
        }
        let first = returned.withLock { (done: inout Bool) -> Bool in
            if done {
                return false
            }
            done = true
            return true
        }
        if first {
            for instance in instances {
                callbacks.giveBack(instance)
            }
        }
    }

    /// Runs a step before the call is sent: if it throws, nothing reached the core.
    func unlessSent<Output>(_ body: () throws -> Output) rethrows -> Output {
        do {
            return try body()
        } catch {
            giveBack()
            throw error
        }
    }

    /// Runs a call: a refusal (status 5) transferred nothing.
    func givingBackIfRefused<Output>(_ body: () throws -> Output) rethrows -> Output {
        do {
            return try body()
        } catch let error as UndraReplyError where error.status == .badRequest {
            giveBack()
            throw error
        }
    }

    /// The asynchronous form of ``givingBackIfRefused(_:)``.
    func givingBackIfRefused<Output>(_ body: () async throws -> Output) async rethrows -> Output {
        do {
            return try await body()
        } catch let error as UndraReplyError where error.status == .badRequest {
            giveBack()
            throw error
        }
    }
}
