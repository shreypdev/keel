import Foundation
import UndraRuntime

/// What a call that no recording answers does.
public enum Exhausted: Sendable {
    /// Answer a refused request naming the call (the default).
    case fail
    /// Answer again with the last recorded reply.
    case repeatLast
}

/// How a ``RecordedCore`` matches the calls the app makes to the recorded ones.
public struct ReplayOptions: Sendable {
    /// `.ignore` (default) matches a call on its target and method alone, in order; `.exact` also compares the encoded arguments.
    public var args: ArgsPolicy
    /// What a call does when the recording has no (more) reply for it.
    public var exhausted: Exhausted
    /// Where the playhead starts, in milliseconds of the session. Default (`nil`): the time of the first recorded change-set, so that observing a
    /// store shows the state the recording first saw.
    public var startAtMs: UInt64?

    public init(args: ArgsPolicy = .ignore, exhausted: Exhausted = .fail, startAtMs: UInt64? = nil) {
        self.args = args
        self.exhausted = exhausted
        self.startAtMs = startAtMs
    }
}

private struct RecordedCall {
    let args: [UInt8]
    let status: ReplyStatus
    let body: [UInt8]
    var items: [(StreamFlagName, [UInt8])]
}

private struct ChangeEvent {
    let t: UInt64
    let txn: UInt64
    let entries: [RecordedEntry]
}

private func key(_ t: RecordedTarget) -> String {
    switch t {
    case .function(let method): return "f:\(method)"
    case .method(let handle, let method): return "m:\(handle):\(method)"
    case .constructor(let type, let method): return "c:\(type):\(method)"
    case .page(let handle, let offset, let limit): return "p:\(handle):\(offset):\(limit)"
    }
}

private func key(_ t: CallTarget) -> String {
    switch t {
    case .freeFunction(let method): return "f:\(method)"
    case .objectMethod(let handle, let method): return "m:\(handle.rawValue):\(method)"
    case .constructor(let type, let method): return "c:\(type):\(method)"
    case .lazyListPage(let handle, let offset, let limit): return "p:\(handle.rawValue):\(offset):\(limit)"
    }
}

private func describe(_ t: CallTarget) -> String {
    switch t {
    case .freeFunction(let method): return "function \(method)"
    case .objectMethod(let handle, let method): return "method \(method) of \(handle)"
    case .constructor(let type, let method): return "constructor \(method) of type \(type)"
    case .lazyListPage(let handle, let offset, let limit): return "page \(offset)+\(limit) of \(handle)"
    }
}

/// A transport that plays a recording instead of reaching a core: replies come from the recorded replies (call ids rewritten), and the recorded
/// change-sets are released by a manual playhead. Under the unchanged `UndraCore`, mirror and generated stores. Synchronous, like an in-process
/// core: an `observe` has delivered the state up to the playhead before it returns.
final class ReplayTransport: UndraTransport, @unchecked Sendable {
    private struct State {
        var inbound: (any UndraInbound)?
        var calls: [String: [RecordedCall]] = [:]
        var last: [String: RecordedCall] = [:]
        var sets: [ChangeEvent] = []
        var observed: [UInt64: Set<UInt32>] = [:]
        var cursor = 0
        var playhead: UInt64 = 0
    }

    private let recording: Recording
    private let options: ReplayOptions
    private let state = Locked(State())

    init(_ recording: Recording, options: ReplayOptions) {
        self.recording = recording
        self.options = options
        var pending = [UInt32: (key: String, args: [UInt8])]()
        var byCall = [UInt32: (key: String, index: Int)]()
        var s = State()
        for event in recording.events {
            switch event.kind {
            case .call(let target, let call, let args):
                pending[call] = (key(target), args)
            case .reply(let call, let status, let body):
                guard let p = pending.removeValue(forKey: call) else {
                    continue
                }
                let recorded = RecordedCall(args: p.args, status: ReplyStatus(rawValue: status.code) ?? .badRequest, body: body, items: [])
                byCall[call] = (p.key, s.calls[p.key, default: []].count)
                s.calls[p.key, default: []].append(recorded)
            case .streamItem(let call, let flag, let body):
                if let at = byCall[call] {
                    s.calls[at.key]?[at.index].items.append((flag, body))
                }
            case .changeSet(let txn, let entries):
                s.sets.append(ChangeEvent(t: event.t, txn: txn, entries: entries))
            default:
                break
            }
        }
        s.playhead = options.startAtMs ?? s.sets.first?.t ?? 0
        // What the recording had said by then is history: an observe is answered with it.
        while s.cursor < s.sets.count && s.sets[s.cursor].t <= s.playhead {
            s.cursor += 1
        }
        state.withLock { $0 = s }
    }

    /// The playhead: milliseconds of the recording released so far.
    var playhead: UInt64 {
        return state.withLock { $0.playhead }
    }

    /// The time of the last recorded change-set.
    var durationMs: UInt64 {
        return state.withLock { $0.sets.last?.t ?? 0 }
    }

    package var mode: UndraMode { .inproc }

    package var supportsDirectSync: Bool { true }

    package func start(inbound: any UndraInbound, options: TransportStartOptions) throws -> TransportInfo {
        state.withLock { $0.inbound = inbound }
        return TransportInfo(schemaHash: recording.schemaHash)
    }

    private static func entry(_ e: RecordedEntry) -> Wire.ChangeEntry {
        let op: ChangeOp
        switch e.op {
        case .full: op = .fullValue
        case .patch: op = .keyedPatch
        case .lazyInvalidated: op = .lazyListInvalidated
        }
        return Wire.ChangeEntry(handle: UndraHandle(rawValue: e.handle), signalId: e.signal, op: op, value: e.value[...])
    }

    private func releaseUntil(_ t: UInt64) {
        let (inbound, payloads) = state.withLock { (s: inout State) -> ((any UndraInbound)?, [[UInt8]]) in
            var out = [[UInt8]]()
            while s.cursor < s.sets.count && s.sets[s.cursor].t <= t {
                let set = s.sets[s.cursor]
                s.cursor += 1
                let entries = set.entries.filter { e in
                    guard let signals = s.observed[e.handle] else {
                        return false
                    }
                    return signals.contains(Observe.allSignals) || signals.contains(e.signal)
                }.map(ReplayTransport.entry)
                if !entries.isEmpty {
                    out.append(Wire.ChangeSet(txnId: set.txn, entries: entries).encode())
                }
            }
            return (s.inbound, out)
        }
        for payload in payloads {
            inbound?.onChangeSet(payload)
        }
    }

    /// Moves the playhead forward by `ms` and releases the change-sets it passes.
    func advance(ms: UInt64) {
        let to = state.withLock { (s: inout State) -> UInt64 in
            s.playhead += ms
            return s.playhead
        }
        releaseUntil(to)
    }

    /// Releases everything that is left.
    func playAll() {
        let to = state.withLock { (s: inout State) -> UInt64 in
            s.playhead = max(s.playhead, s.sets.last?.t ?? 0)
            return s.playhead
        }
        releaseUntil(to)
    }

    package func observe(handle: UndraHandle, signal: UInt32, on: Bool) {
        let (inbound, initial) = state.withLock { (s: inout State) -> ((any UndraInbound)?, [UInt8]?) in
            if !on {
                if signal == Observe.allSignals {
                    s.observed[handle.rawValue] = nil
                } else {
                    s.observed[handle.rawValue]?.remove(signal)
                }
                return (s.inbound, nil)
            }
            s.observed[handle.rawValue, default: []].insert(signal)
            // The core answers an observe with what the signals hold: here, everything recorded up to the playhead, in order.
            var entries = [Wire.ChangeEntry]()
            for i in 0..<s.cursor {
                for e in s.sets[i].entries where e.handle == handle.rawValue && (signal == Observe.allSignals || e.signal == signal) {
                    entries.append(ReplayTransport.entry(e))
                }
            }
            return (s.inbound, entries.isEmpty ? nil : Wire.ChangeSet(txnId: 0, entries: entries).encode())
        }
        if let initial = initial {
            inbound?.onChangeSet(initial)
        }
    }

    /// Finds the recorded reply for `call`, or the reason to refuse it.
    private func find(_ call: Wire.Call) -> (RecordedCall?, String?) {
        return state.withLock { (s: inout State) -> (RecordedCall?, String?) in
            let k = key(call.target)
            var found: RecordedCall?
            if var queue = s.calls[k], !queue.isEmpty {
                var at = 0
                if options.args == .exact, case .lazyListPage = call.target {
                    at = 0
                } else if options.args == .exact {
                    at = queue.firstIndex { $0.args == Array(call.args) } ?? -1
                }
                if at >= 0 {
                    found = queue.remove(at: at)
                    s.calls[k] = queue
                    s.last[k] = found
                }
            }
            if found == nil && options.exhausted == .repeatLast {
                found = s.last[k]
            }
            if let found = found {
                return (found, nil)
            }
            let exact = options.args == .exact ? " with these arguments" : ""
            return (nil, "RecordedCore: the recording has no reply for \(describe(call.target))\(exact)")
        }
    }

    private func refusal(_ callId: UInt32, _ reason: String) -> Wire.Reply {
        var writer = UndraWriter()
        writer.writeString(reason)
        return Wire.Reply(callId: callId, status: .badRequest, body: writer.finishSlice())
    }

    package func send(call payload: [UInt8]) -> Bool {
        guard let call = try? Wire.Call.decode(payload) else {
            return false
        }
        let inbound = state.withLock { $0.inbound }
        let (found, why) = find(call)
        guard let recorded = found else {
            inbound?.onReply(callId: call.callId, payload: refusal(call.callId, why ?? "no reply").encode())
            return true
        }
        inbound?.onReply(callId: call.callId, payload: Wire.Reply(callId: call.callId, status: recorded.status, body: recorded.body[...]).encode())
        for (flag, body) in recorded.items {
            let wireFlag = Wire.StreamFlag(rawValue: flag.code) ?? .failed
            inbound?.onStreamItem(callId: call.callId, payload: Wire.StreamItem(callId: call.callId, flag: wireFlag, body: body[...]).encode())
        }
        return true
    }

    package func callSync(_ payload: [UInt8]) throws -> [UInt8] {
        let call = try Wire.Call.decode(payload)
        let (found, why) = find(call)
        guard let recorded = found else {
            return refusal(call.callId, why ?? "no reply").encode()
        }
        return Wire.Reply(callId: call.callId, status: recorded.status, body: recorded.body[...]).encode()
    }

    package func cancel(callId: UInt32) {}

    package func streamCredit(callId: UInt32, credit: UInt32) {}

    package func release(handle: UndraHandle) {}

    package func registerPort(_ portId: UInt32) {}

    package func portReply(_ payload: [UInt8]) {}

    package func event(portId: UInt32, methodId: UInt32, payload: [UInt8]) {}

    package func timerFired(_ timerId: UInt32) {}

    package func snapshot() throws -> [UInt8] {
        throw UndraModeError(operation: "snapshot", mode: .inproc)
    }

    package func restore(_ payload: [UInt8]) throws {
        throw UndraModeError(operation: "restore", mode: .inproc)
    }

    package func statsJSON() -> String? {
        return nil
    }

    package func shutdown() {}
}

/// A core that replays a recording: the generated stores run on the recorded replies and change-sets with no core behind them, for previews of
/// states that are expensive to reach. Interaction the recording does not hold fails with a typed refusal naming the call; for an app that has to
/// react, use ``PreviewCore``, the real core with fakes.
///
/// ```swift
/// let recorded = try RecordedCore.load(Recording(json: text), expectedSchemaHash: UndraIds.schemaHash)
/// let todos = try Todos(core: recorded.core)      // the recorded constructor reply
/// await recorded.advance(ms: 300)                 // the change-sets up to 300 ms into the session
/// ```
public final class RecordedCore: @unchecked Sendable {
    /// The core: pass it to the generated stores as `ctx`.
    public let core: UndraCore
    private let transport: ReplayTransport

    private init(core: UndraCore, transport: ReplayTransport) {
        self.core = core
        self.transport = transport
    }

    /// Loads `recording`.
    ///
    /// - Parameters:
    ///   - expectedSchemaHash: the schema hash of the bindings (`UndraIds.schemaHash`).
    ///   - onError: where failures that reach nobody are reported, as for `UndraCore.load`.
    /// - Throws: `UndraSchemaMismatchError` when the recording belongs to another schema.
    public static func load(
        _ recording: Recording,
        expectedSchemaHash: UInt64,
        options: ReplayOptions = ReplayOptions(),
        onError: (@Sendable (UndraUnhandledError) -> Void)? = nil
    ) throws -> RecordedCore {
        let transport = ReplayTransport(recording, options: options)
        // No adapter is installed: nothing touches the platform.
        let load = LoadOptions(mode: .inproc, adapters: .none, expectedSchemaHash: expectedSchemaHash, onError: onError)
        return RecordedCore(core: try UndraCore.attach(transport: transport, options: load), transport: transport)
    }

    /// The playhead, in milliseconds.
    public var playhead: UInt64 {
        return transport.playhead
    }

    /// The time of the last recorded change-set.
    public var durationMs: UInt64 {
        return transport.durationMs
    }

    /// Moves the playhead forward by `ms`, releases the recorded change-sets it passes for the signals that are observed, and returns once the
    /// mirror has applied them to the stores.
    @MainActor
    public func advance(ms: UInt64) {
        transport.advance(ms: ms)
        core.mirror.flush()
    }

    /// Plays the recording to its end.
    @MainActor
    public func playAll() {
        transport.playAll()
        core.mirror.flush()
    }

    /// Shuts the core down.
    public func close() {
        core.shutdown()
    }
}
