import Foundation
import XCTest
@testable import UndraRuntime

// The fixtures of the lazy-list tests: a page server standing in for the core, a hand-driven "turn" queue (the coalescing seam of
// `UndraLazyListEngine`), and a rig that wires them to a fake core.

/// Records what `LoadOptions.onError` receives, from any thread.
final class LazyErrors: @unchecked Sendable {
    private let items = Guarded<[UndraUnhandledError]>([])

    var all: [UndraUnhandledError] {
        return items.withLock { (current: inout [UndraUnhandledError]) -> [UndraUnhandledError] in
            return current
        }
    }

    var count: Int {
        return all.count
    }

    func append(_ item: UndraUnhandledError) {
        items.withLock { (current: inout [UndraUnhandledError]) -> Void in
            current.append(item)
        }
    }
}

/// The main-actor turns of a list, run by hand: `schedule` is what the engine's flush is handed, `run()` is the next turn.
@MainActor
final class LazyTurns {
    private(set) var pending: [@MainActor @Sendable () -> Void] = []
    /// How many flushes were scheduled in all.
    private(set) var scheduled = 0

    func schedule(_ work: @escaping @MainActor @Sendable () -> Void) {
        pending.append(work)
        scheduled += 1
    }

    /// Runs the flushes scheduled so far (not the ones they schedule).
    func run() {
        let due = pending
        pending = []
        for work in due {
            work()
        }
    }

    /// Runs turns until nothing is scheduled (a stale reply schedules another), at most `limit` of them.
    func runUntilQuiet(limit: Int = 20) {
        var turns = 0
        while !pending.isEmpty && turns < limit {
            run()
            turns += 1
        }
    }
}

/// A page server in place of the core: `rows` at a `version`, answering page calls, recording them, and able to hold the replies of
/// asynchronous calls until the test lets them through.
final class LazyServer: @unchecked Sendable {
    struct State {
        var rows: [Int32]
        var version: UInt64
        var offsets: [UInt32] = []
        var limits: [UInt32] = []
        var targets: [UndraHandle] = []
        var held: [(callId: UInt32, body: [UInt8])] = []
        var holds = false
        var respond: (@Sendable (_ offset: UInt32, _ limit: UInt32) -> [UInt8])?
        var afterReply: (@Sendable () -> Void)?
    }

    let handle: UndraHandle
    private let state: Guarded<State>

    init(rows: Int, handle: UndraHandle = UndraHandle(index: 5, generation: 1), version: UInt64 = 1) {
        self.handle = handle
        self.state = Guarded<State>(State(rows: (0 ..< rows).map { Int32($0) }, version: version))
    }

    // MARK: The list

    var version: UInt64 {
        return state.withLock { (current: inout State) -> UInt64 in
            return current.version
        }
    }

    var rows: [Int32] {
        return state.withLock { (current: inout State) -> [Int32] in
            return current.rows
        }
    }

    /// The signal's value: the handle, the length and the version.
    func value(handle override: UndraHandle? = nil) -> [UInt8] {
        let (len, version) = state.withLock { (current: inout State) -> (UInt32, UInt64) in
            return (UInt32(current.rows.count), current.version)
        }
        return UndraLazyValue(handle: override ?? handle, len: len, version: version).undraEncoded()
    }

    /// The invalidation the core sends after a change.
    func invalidated() -> [UInt8] {
        let (len, version) = state.withLock { (current: inout State) -> (UInt32, UInt64) in
            return (UInt32(current.rows.count), current.version)
        }
        return UndraLazyInvalidated(len: len, version: version).undraEncoded()
    }

    /// Changes the list (and so bumps its version).
    func mutate(_ change: (inout [Int32]) -> Void) {
        state.withLock { (current: inout State) -> Void in
            change(&current.rows)
            current.version += 1
        }
    }

    // MARK: What the host asked

    /// The offsets of the page calls, in order.
    var offsets: [UInt32] {
        return state.withLock { (current: inout State) -> [UInt32] in
            return current.offsets
        }
    }

    /// The pages (offset / `size`) of the page calls, in order.
    func pages(size: Int = 50) -> [Int] {
        return offsets.map { Int($0) / size }
    }

    /// The `limit` of every page call.
    var limits: [UInt32] {
        return state.withLock { (current: inout State) -> [UInt32] in
            return current.limits
        }
    }

    /// The handle each page call named.
    var targets: [UndraHandle] {
        return state.withLock { (current: inout State) -> [UndraHandle] in
            return current.targets
        }
    }

    func forgetCalls() {
        state.withLock { (current: inout State) -> Void in
            current.offsets = []
            current.limits = []
            current.targets = []
        }
    }

    // MARK: Scripting

    /// Replaces the replies of page calls (nil restores the honest ones).
    func respond(_ body: (@Sendable (_ offset: UInt32, _ limit: UInt32) -> [UInt8])?) {
        state.withLock { (current: inout State) -> Void in
            current.respond = body
        }
    }

    /// Runs after a page call has been answered, before the reply is handed back (a change-set the core produced meanwhile).
    func afterReply(_ body: (@Sendable () -> Void)?) {
        state.withLock { (current: inout State) -> Void in
            current.afterReply = body
        }
    }

    /// Holds the replies of asynchronous page calls (read at the time of the call) until `release(on:)`.
    func hold(_ on: Bool) {
        state.withLock { (current: inout State) -> Void in
            current.holds = on
        }
    }

    var heldCount: Int {
        return state.withLock { (current: inout State) -> Int in
            return current.held.count
        }
    }

    /// Lets the held replies through, in the order of their calls.
    func release(on transport: FakeTransport) {
        let held = state.withLock { (current: inout State) -> [(callId: UInt32, body: [UInt8])] in
            let all = current.held
            current.held = []
            return all
        }
        for reply in held {
            transport.replyOk(reply.callId, reply.body)
        }
    }

    // MARK: Pages

    /// The honest reply for a page at the list's current version.
    func page(offset: UInt32, limit: UInt32) -> [UInt8] {
        let (rows, version) = state.withLock { (current: inout State) -> ([Int32], UInt64) in
            return (current.rows, current.version)
        }
        let start = min(Int(offset), rows.count)
        let end = min(start + Int(limit), rows.count)
        var writer = UndraWriter()
        UndraLazyPageHeader(version: version, total: UInt32(rows.count), count: UInt32(end - start)).undraEncode(&writer)
        for row in rows[start ..< end] {
            row.undraEncode(&writer)
        }
        return writer.finish()
    }

    /// Records a call and builds the reply for it.
    private func answer(_ call: Wire.Call) -> (body: [UInt8], hold: Bool, after: (@Sendable () -> Void)?)? {
        guard case .lazyListPage(let target, let offset, let limit) = call.target else {
            return nil
        }
        let script = state.withLock { (current: inout State) -> (respond: (@Sendable (UInt32, UInt32) -> [UInt8])?, holds: Bool, after: (@Sendable () -> Void)?) in
            current.offsets.append(offset)
            current.limits.append(limit)
            current.targets.append(target)
            return (current.respond, current.holds, current.afterReply)
        }
        let body = script.respond?(offset, limit) ?? page(offset: offset, limit: limit)
        return (body, script.holds, script.after)
    }

    /// Makes `transport` serve page calls from this list.
    func install(on transport: FakeTransport) {
        transport.onCallSync = { [self] call in
            guard let reply = answer(call) else {
                return Wire.Reply(callId: call.callId, status: .ok).encode()
            }
            reply.after?()
            return Wire.Reply(callId: call.callId, status: .ok, body: ArraySlice(reply.body)).encode()
        }
        transport.onCall = { [self] call, fake in
            guard let reply = answer(call) else {
                return true
            }
            if reply.hold {
                state.withLock { (current: inout State) -> Void in
                    current.held.append((callId: call.callId, body: reply.body))
                }
            } else {
                fake.replyOk(call.callId, reply.body)
            }
            return true
        }
    }
}

/// A list over a fake core and a page server.
@MainActor
final class LazyRig {
    let transport: FakeTransport
    let core: UndraCore
    let server: LazyServer
    let turns = LazyTurns()
    let errors = LazyErrors()
    let list: UndraLazyList<Int32>

    /// A list of `rows` rows at version 1, already applied (the signal's first value).
    init(rows: Int, direct: Bool = true, hold: Bool = false, pageSize: Int = 50, maxCachedPages: Int = 24) throws {
        transport = FakeTransport(directSync: direct)
        server = LazyServer(rows: rows)
        server.install(on: transport)
        server.hold(hold)
        var options = LoadOptions.inproc(adapters: Adapters.none, expectedSchemaHash: 0x1234)
        let errors = self.errors
        options.onError = { errors.append($0) }
        core = try UndraCore.connect(transport: transport, options: options, frameScheduler: nil)
        let turns = self.turns
        list = UndraLazyList<Int32>(core: core, schedule: { work in
            turns.schedule(work)
        })
        list.pageSize = pageSize
        list.maxCachedPages = maxCachedPages
        try applyValue(server.value())
    }

    func applyValue(_ bytes: [UInt8]) throws {
        var reader = UndraReader(bytes)
        try list.applyFull(&reader)
    }

    func applyInvalidated(_ bytes: [UInt8]) throws {
        var reader = UndraReader(bytes)
        try list.applyInvalidated(&reader)
    }

    /// The core changed the list and says so.
    func change(_ change: (inout [Int32]) -> Void) throws {
        server.mutate(change)
        try applyInvalidated(server.invalidated())
    }

    /// Reads rows and runs the turn their requests are sent in.
    @discardableResult
    func read(_ indexes: Int...) -> [Int32?] {
        let values = indexes.map { list[$0] }
        turns.runUntilQuiet()
        return values
    }
}

/// A store as generated code writes it: one `UndraLazyList` signal (id 3) fed by the entries of its change-sets.
@MainActor
final class LazyLibraryStore: UndraStore, @unchecked Sendable {
    let books: UndraLazyList<Int32>

    init(core: UndraCore, handle: UndraHandle, schedule: @escaping UndraLazyListSchedule) {
        books = UndraLazyList<Int32>(core: core, schedule: schedule)
        super.init(core: core, handle: handle)
    }

    override func apply(signal: UInt32, op: ChangeOp, reader: inout UndraReader) {
        do {
            switch signal {
            case 3:
                switch op {
                case .fullValue:
                    try books.applyFull(&reader)
                case .keyedPatch:
                    break
                case .lazyListInvalidated:
                    try books.applyInvalidated(&reader)
                }
            default:
                break
            }
        } catch {
            core.report(error, operation: "Library.apply(signal: \(signal))")
        }
    }
}

/// A page body: a header and the given rows (for hostile replies).
func lazyPageBody(version: UInt64, total: UInt32, count: UInt32, rows: [Int32], trailing: [UInt8] = []) -> [UInt8] {
    var writer = UndraWriter()
    UndraLazyPageHeader(version: version, total: total, count: count).undraEncode(&writer)
    for row in rows {
        row.undraEncode(&writer)
    }
    writer.writeRaw(trailing)
    return writer.finish()
}

/// Counts what an observation reports.
final class LazyCounter: @unchecked Sendable {
    private let value = Guarded<Int>(0)

    var count: Int {
        return value.withLock { (current: inout Int) -> Int in
            return current
        }
    }

    func bump() {
        value.withLock { (current: inout Int) -> Void in
            current += 1
        }
    }
}
