import Foundation
import UndraRuntime

// MARK: - Recording the port traffic of real adapters

/// Records the port traffic of the adapters it wraps: for each call, the port, the method, the encoded arguments, the reply (or the typed
/// error, or "unavailable") and the time. Wrap the adapters you load a core with and write the result with ``toJSON()``.
///
/// The recording holds `port_call` and `port_reply` events only: `undra dev --record` captures the whole session.
public final class PortRecorder: @unchecked Sendable {
    private struct State {
        var events: [RecordedEvent] = []
        var next: UInt32 = 0
    }

    private let schemaHash: UInt64
    private let now: @Sendable () -> UInt64
    private let platform: String?
    private let source: String
    private let start: UInt64
    private let state = Locked(State())

    /// - Parameters:
    ///   - schemaHash: the schema hash of the core the traffic belongs to.
    ///   - now: milliseconds, any origin: the recording's times are relative to the first reading. Default the system's monotonic clock; pass a
    ///     manual clock for byte-for-byte reproducible recordings.
    ///   - platform: informational: the host platform, for example `"ios"`.
    ///   - source: informational: where the recording came from.
    public init(
        schemaHash: UInt64,
        now: @escaping @Sendable () -> UInt64 = { DispatchTime.now().uptimeNanoseconds / 1_000_000 },
        platform: String? = nil,
        source: String = "adapters"
    ) {
        self.schemaHash = schemaHash
        self.now = now
        self.platform = platform
        self.source = source
        self.start = now()
    }

    private func push(_ kind: RecordedKind) {
        let current = now()
        let t = current >= start ? current - start : 0
        state.withLock { (s: inout State) -> Void in
            let last = s.events.last?.t ?? 0
            s.events.append(RecordedEvent(t: max(last, t), kind: kind))
        }
    }

    private func nextCall() -> UInt32 {
        return state.withLock { (s: inout State) -> UInt32 in
            s.next += 1
            return s.next
        }
    }

    /// `impl` with every method recording its calls.
    public func wrap(portId: UInt32, _ impl: PortImpl) -> PortImpl {
        func recording(_ method: UInt32, _ body: @escaping AsyncPortMethod) -> AsyncPortMethod {
            return { [self] args in
                let call = nextCall()
                push(.portCall(port: portId, method: method, call: call, args: args))
                do {
                    let reply = try await body(args)
                    push(.portReply(call: call, status: .ok, body: reply))
                    return reply
                } catch let error as UndraPortError {
                    push(.portReply(call: call, status: .error, body: error.body))
                    throw error
                } catch {
                    push(.portReply(call: call, status: .unavailable, body: []))
                    throw error
                }
            }
        }
        switch impl {
        case .sync(let methods):
            var wrapped = [UInt32: SyncPortMethod]()
            for (method, body) in methods {
                wrapped[method] = { [self] args in
                    let call = nextCall()
                    push(.portCall(port: portId, method: method, call: call, args: args))
                    do {
                        let reply = try body(args)
                        push(.portReply(call: call, status: .ok, body: reply))
                        return reply
                    } catch let error as UndraPortError {
                        push(.portReply(call: call, status: .error, body: error.body))
                        throw error
                    } catch {
                        push(.portReply(call: call, status: .unavailable, body: []))
                        throw error
                    }
                }
            }
            return .sync(wrapped)
        case .async(let methods):
            var wrapped = [UInt32: AsyncPortMethod]()
            for (method, body) in methods {
                wrapped[method] = recording(method, body)
            }
            return .async(wrapped)
        }
    }

    /// `adapters` with every port recording its traffic: what to load the core with.
    public func wrap(_ adapters: Adapters) -> Adapters {
        return Adapters(adapters.all.map { RecordingAdapter(recorder: self, inner: $0) as any UndraAdapter })
    }

    /// What has been recorded.
    public func record() -> Recording {
        return Recording(schemaHash: schemaHash, source: source, platform: platform, events: state.withLock { $0.events })
    }

    /// The recording as canonical JSON.
    public func toJSON() -> String {
        return record().toJSON()
    }

    private struct RecordingAdapter: UndraAdapter {
        let recorder: PortRecorder
        let inner: any UndraAdapter

        var portId: UInt32 { inner.portId }

        func makePortImpl(core: UndraCore) -> PortImpl? {
            return inner.makePortImpl(core: core).map { recorder.wrap(portId: inner.portId, $0) }
        }

        func attach(to core: UndraCore) {
            inner.attach(to: core)
        }

        func detach() {
            inner.detach()
        }
    }
}

// MARK: - Replaying it

/// Where a replay left the recording.
public enum ReplayError: Error, Equatable, Sendable, CustomStringConvertible {
    /// The core called a different method (or the same method with other arguments) than the recording's next call of this port.
    case mismatch(port: UInt32, called: String, expected: String, argsDiffer: Bool, nth: Int)
    /// The core called a port more often than the recording did.
    case exhausted(port: UInt32, called: String, recorded: Int)
    /// The core made the recorded call, but the recording holds no reply for it (the session ended while it was in flight, or the file was cut): it
    /// was answered "unavailable".
    case unanswered(port: UInt32, called: String, nth: Int)
    /// The replay ended with recorded calls the core never made.
    case unconsumed(port: UInt32, next: String, remaining: Int)

    public var description: String {
        switch self {
        case .mismatch(_, let called, let expected, let argsDiffer, let nth):
            return argsDiffer
                ? "replay: call \(nth) of the port was \(called) with other arguments than the recording's"
                : "replay: call \(nth) of the port was \(called), the recording has \(expected) next"
        case .exhausted(_, let called, let recorded):
            return "replay: \(called) was called after the recording's \(recorded) call(s) of the port were used up"
        case .unanswered(_, let called, let nth):
            return "replay: call \(nth) of the port, \(called), has no reply in the recording; it was answered unavailable"
        case .unconsumed(_, let next, let remaining):
            return "replay: \(remaining) recorded call(s) were never made, the next is \(next)"
        }
    }
}

/// Thrown by ``Replayer/finish()`` when the replay deviated; `errors` has every deviation.
public struct ReplayFailure: Error, CustomStringConvertible, Sendable {
    public let errors: [ReplayError]

    public var description: String { errors.map { $0.description }.joined(separator: "\n") }
}

/// How strictly a replayed call must match the recorded one.
public enum ArgsPolicy: Sendable {
    /// The encoded arguments must be byte for byte the recorded ones (the default).
    case exact
    /// Only the port and method must match: for calls whose arguments carry something that changes between runs.
    case ignore
}

private func label(_ port: UInt32, _ method: UInt32) -> String {
    return undraStandardName(port: port, method: method) ?? "port \(port) method \(method)"
}

/// Answers a core's port calls from a recording, in order: the recording's `port_call` events, per port, are the script. A call that is the next
/// recorded one of its port (same method and, by default, the same arguments) gets the recorded reply; anything else is a typed ``ReplayError``,
/// kept for ``finish()``, and the core is answered "unavailable". A deviation consumes nothing, so one wrong call does not shift every later
/// answer. Time is not replayed: answers are immediate.
///
/// A port's method table is a fixed dictionary in Swift, so a call is checked for the method ids the recording holds for the port and, for a
/// standard port, all of its methods; another method id of a port is answered "unavailable" without being counted.
public final class Replayer: @unchecked Sendable {
    private struct Expected {
        let method: UInt32
        let args: [UInt8]
        var reply: (PortStatusName, [UInt8])?
    }

    private struct State {
        var queues: [UInt32: [Expected]] = [:]
        var order: [UInt32] = []
        var recorded: [UInt32: Int] = [:]
        var answered: [UInt32: Int] = [:]
        var methods: [UInt32: Set<UInt32>] = [:]
        var deviations: [ReplayError] = []
    }

    private let policy: ArgsPolicy
    private let state = Locked(State())

    /// A replayer over the port calls of `recording`.
    public init(_ recording: Recording, policy: ArgsPolicy = .exact) {
        self.policy = policy
        var open = [UInt32: (port: UInt32, index: Int)]()
        state.withLock { (s: inout State) -> Void in
            for event in recording.events {
                switch event.kind {
                case .portCall(let port, let method, let call, let args):
                    if s.queues[port] == nil {
                        s.order.append(port)
                    }
                    open[call] = (port, s.queues[port, default: []].count)
                    s.queues[port, default: []].append(Expected(method: method, args: args, reply: nil))
                    s.recorded[port, default: 0] += 1
                    s.methods[port, default: Set(standardMethodIds(port: port))].insert(method)
                case .portReply(let call, let status, let body):
                    if let at = open.removeValue(forKey: call) {
                        s.queues[at.port]?[at.index].reply = (status, body)
                    }
                default:
                    break
                }
            }
        }
    }

    private func answer(port: UInt32, method: UInt32, args: [UInt8]) throws -> [UInt8] {
        let outcome = state.withLock { (s: inout State) -> Result<(PortStatusName, [UInt8])?, ReplayError> in
            let nth = s.answered[port] ?? 0
            guard let next = s.queues[port]?.first else {
                let error = ReplayError.exhausted(port: port, called: label(port, method), recorded: s.recorded[port] ?? 0)
                s.deviations.append(error)
                return .failure(error)
            }
            let sameMethod = next.method == method
            if !(sameMethod && (policy == .ignore || next.args == args)) {
                let error = ReplayError.mismatch(port: port, called: label(port, method), expected: label(port, next.method), argsDiffer: sameMethod, nth: nth)
                s.deviations.append(error)
                return .failure(error)
            }
            s.queues[port]?.removeFirst()
            s.answered[port] = nth + 1
            if next.reply == nil {
                s.deviations.append(.unanswered(port: port, called: label(port, method), nth: nth))
            }
            return .success(next.reply)
        }
        switch outcome {
        case .failure(let error):
            throw error
        case .success(let reply):
            guard let reply = reply, reply.0 != .unavailable else {
                throw ReplayUnavailable()
            }
            if reply.0 == .error {
                throw UndraPortError(body: reply.1)
            }
            return reply.1
        }
    }

    private struct ReplayUnavailable: Error {}

    /// One adapter per recorded port, ready to load a core with.
    public func adapters() -> Adapters {
        let (ports, methods) = state.withLock { ($0.order, $0.methods) }
        return Adapters(ports.map { port -> any UndraAdapter in
            var table = [UInt32: AsyncPortMethod]()
            for method in methods[port] ?? [] {
                table[method] = { [self] args in try answer(port: port, method: method, args: args) }
            }
            return PortImplAdapter(portId: port, impl: .async(table))
        })
    }

    /// The deviations seen so far.
    public func errors() -> [ReplayError] {
        return state.withLock { $0.deviations }
    }

    /// How many recorded calls are still waiting to be made.
    public var remaining: Int {
        return state.withLock { $0.queues.values.reduce(0) { $0 + $1.count } }
    }

    /// The deviations so far followed by one ``ReplayError/unconsumed(port:next:remaining:)`` per port with calls left; empty when the replay
    /// was clean.
    public func problems() -> [ReplayError] {
        return state.withLock { (s: inout State) -> [ReplayError] in
            var out = s.deviations
            for port in s.queues.keys.sorted() {
                if let queue = s.queues[port], let next = queue.first {
                    out.append(.unconsumed(port: port, next: label(port, next.method), remaining: queue.count))
                }
            }
            return out
        }
    }

    /// Ends the replay.
    ///
    /// - Throws: ``ReplayFailure`` when a call deviated or recorded calls were never made.
    public func finish() throws {
        let all = problems()
        if !all.isEmpty {
            throw ReplayFailure(errors: all)
        }
    }
}
