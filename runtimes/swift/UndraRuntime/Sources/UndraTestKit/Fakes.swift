import Foundation
import UndraRuntime

// The deterministic fakes of the standard ports (docs/TESTING.md): the same behaviour as `undra::ports::fakes`, held to it by
// testkit/conformance/fakes.json, which the test suite replays against these classes. Each is an `UndraAdapter`, so a set of them goes
// straight into `LoadOptions.adapters`.

private let nsPerMs: Int64 = 1_000_000

// MARK: - Clock and Timer

/// Thrown by ``FakeClock/advance(ms:maxTimers:)`` and ``PreviewCore/advance(ms:maxTimers:)`` when one call fires more timers than its cap and another
/// is still due: a timer that re-arms itself at the same instant (or every millisecond across a long window) never lets time move on. The clock
/// stays at the last deadline that fired and the timers still armed stay armed.
public struct TimerStormError: Error, Equatable, Sendable, CustomStringConvertible {
    /// How many timers fired before the cap stopped the call.
    public let fired: Int
    /// The id of the timer that was due next.
    public let timerId: UInt32
    /// The monotonic reading of the clock when it stopped, in whole milliseconds.
    public let atMs: Int64

    public var description: String {
        return "advance fired \(fired) timers and timer \(timerId) is due again at \(atMs) ms: a timer that re-arms itself without time passing never ends "
            + "(raise maxTimers if the window really holds that many)"
    }
}

/// A deterministic `Clock` and `Timer`: time only moves when the test says so.
///
/// ``nowMs`` is a wall clock you can ``setNowMs(_:)``; ``monotonicNs`` starts at 0 and ``advance(ms:)`` moves both by the same amount.
/// ``set(timerId:delayMs:)`` arms a timer on the monotonic counter; ``advance(ms:)`` fires every timer that comes due in deadline order
/// (ties in arming order) with the clock reading exactly the deadline while each one fires. Firing calls ``onTimerFired`` (a
/// ``PreviewCore`` points it at the core's `timerFired`). Nothing here reads the system clock.
public final class FakeClock: @unchecked Sendable {
    /// The wall-clock reading of a new clock: 2023-11-14T22:13:20Z.
    public static let defaultNowMs: Int64 = 1_700_000_000_000
    /// The most timers one ``advance(ms:maxTimers:)`` fires by default.
    public static let maxTimersPerAdvance = 100_000

    private struct Armed {
        let deadlineNs: Int64
        let seq: Int64
        let id: UInt32
    }

    private struct State {
        var wallNs: Int64
        var monoNs: Int64 = 0
        var seq: Int64 = 0
        var timers: [Armed] = []
        var hook: (@Sendable (UInt32) -> Void)?
    }

    private let state: Locked<State>

    /// A clock reading `nowMs` on its wall clock, with the monotonic counter at 0.
    public init(nowMs: Int64 = FakeClock.defaultNowMs) {
        self.state = Locked(State(wallNs: nowMs * nsPerMs))
    }

    /// Called with the id of every timer that fires, on the thread that called ``advance(ms:)`` or ``fireNext(limitMs:)``, with no lock held.
    public var onTimerFired: (@Sendable (UInt32) -> Void)? {
        get { state.withLock { $0.hook } }
        set { state.withLock { $0.hook = newValue } }
    }

    /// The wall clock, milliseconds since the Unix epoch.
    public var nowMs: Int64 {
        return state.withLock { (s: inout State) -> Int64 in
            let q = s.wallNs / nsPerMs
            return s.wallNs % nsPerMs < 0 ? q - 1 : q
        }
    }

    /// The monotonic counter in nanoseconds (starts at 0).
    public var monotonicNs: UInt64 {
        return state.withLock { UInt64($0.monoNs) }
    }

    /// Sets the wall clock. The monotonic counter and the armed timers are not affected: a wall-clock jump is not the passage of time.
    public func setNowMs(_ nowMs: Int64) {
        state.withLock { $0.wallNs = nowMs * nsPerMs }
    }

    /// `Timer.set`: arms timer `timerId` to fire after `delayMs` of fake time.
    public func set(timerId: UInt32, delayMs: UInt64) {
        state.withLock { (s: inout State) -> Void in
            s.seq += 1
            let delay = delayMs > UInt64(Int64.max / nsPerMs / 2) ? Int64.max / 2 : Int64(delayMs) * nsPerMs
            s.timers.append(Armed(deadlineNs: s.monoNs + delay, seq: s.seq, id: timerId))
            s.timers.sort { $0.deadlineNs == $1.deadlineNs ? $0.seq < $1.seq : $0.deadlineNs < $1.deadlineNs }
        }
    }

    /// How many timers are armed and have not fired.
    public var pendingTimers: Int {
        return state.withLock { $0.timers.count }
    }

    /// The ids of the armed timers, in the order they will fire.
    public func pendingTimerIds() -> [UInt32] {
        return state.withLock { $0.timers.map { $0.id } }
    }

    /// Milliseconds until the next armed timer is due, or `nil` when none is armed.
    public func nextDueInMs() -> Int64? {
        return state.withLock { (s: inout State) -> Int64? in
            guard let next = s.timers.first else {
                return nil
            }
            return (next.deadlineNs - s.monoNs + nsPerMs - 1) / nsPerMs
        }
    }

    /// Fires the next armed timer if it is due within `limitMs` from now, moving the clock to its deadline first. Returns its id, or `nil` when
    /// none is due in the window (the clock does not move). ``advance(ms:)`` is a loop over this; a harness that has to wait for the core
    /// between timers (``PreviewCore/advance(ms:)``) drives it itself.
    @discardableResult
    public func fireNext(limitMs: Int64) -> UInt32? {
        let fired = state.withLock { (s: inout State) -> (UInt32, (@Sendable (UInt32) -> Void)?)? in
            guard let next = s.timers.first, next.deadlineNs <= s.monoNs + max(0, limitMs) * nsPerMs else {
                return nil
            }
            s.timers.removeFirst()
            let moved = max(0, next.deadlineNs - s.monoNs)
            s.monoNs += moved
            s.wallNs += moved
            return (next.id, s.hook)
        }
        guard let (id, hook) = fired else {
            return nil
        }
        hook?(id)
        return id
    }

    /// Moves the clock by `ms` without firing anything: what is left of a window after its last timer.
    public func moveBy(ms: Int64) {
        state.withLock { (s: inout State) -> Void in
            let by = max(0, ms) * nsPerMs
            s.monoNs += by
            s.wallNs += by
        }
    }

    /// Moves time forward by `ms` and fires the timers that come due, in order; returns their ids. The hook runs synchronously, and may arm timers
    /// that fall inside the window (they fire in the same call).
    ///
    /// - Parameter maxTimers: the most timers one call may fire.
    /// - Throws: ``TimerStormError`` when `maxTimers` fired and another is still due: a timer that re-arms itself without time passing never ends.
    ///   The clock stays at the last deadline that fired and the timers still armed stay armed.
    @discardableResult
    public func advance(ms: Int64, maxTimers: Int = FakeClock.maxTimersPerAdvance) throws -> [UInt32] {
        var fired = [UInt32]()
        var left = max(0, ms) * nsPerMs
        while true {
            let step = state.withLock { (s: inout State) -> (step: Int64, id: UInt32, atMs: Int64)? in
                guard let next = s.timers.first, next.deadlineNs <= s.monoNs + left else {
                    return nil
                }
                return (max(0, next.deadlineNs - s.monoNs), next.id, s.monoNs / nsPerMs)
            }
            guard let (step, nextId, atMs) = step else {
                break
            }
            if fired.count >= maxTimers {
                throw TimerStormError(fired: fired.count, timerId: nextId, atMs: atMs)
            }
            left -= step
            guard let id = fireNext(limitMs: (step + nsPerMs - 1) / nsPerMs) else {
                break
            }
            fired.append(id)
        }
        state.withLock { (s: inout State) -> Void in
            s.monoNs += left
            s.wallNs += left
        }
        return fired
    }

    /// The `Clock` port over this clock.
    public var clockAdapter: any UndraAdapter {
        return ClockPort(clock: self)
    }

    /// The `Timer` port over this clock.
    public var timerAdapter: any UndraAdapter {
        return TimerPort(clock: self)
    }

    private struct ClockPort: UndraAdapter {
        let clock: FakeClock
        var portId: UInt32 { undraPortId("Clock") }

        func makePortImpl(core: UndraCore) -> PortImpl? {
            let clock = self.clock
            return .sync([
                undraMethodId("Clock", "now_ms"): { _ in clock.nowMs.undraEncoded() },
                undraMethodId("Clock", "monotonic_ns"): { _ in clock.monotonicNs.undraEncoded() },
            ])
        }
    }

    private struct TimerPort: UndraAdapter {
        let clock: FakeClock
        var portId: UInt32 { undraPortId("Timer") }

        func makePortImpl(core: UndraCore) -> PortImpl? {
            let clock = self.clock
            return .sync([
                undraMethodId("Timer", "set"): { args in
                    var reader = UndraReader(args)
                    let id = try reader.readU32()
                    let delay = try reader.readU64()
                    try reader.finish()
                    clock.set(timerId: id, delayMs: delay)
                    return []
                },
            ])
        }
    }
}

// MARK: - Rng

private let zeroSeedReplacement: UInt64 = 0x9E37_79B9_7F4A_7C15
private let rngMultiplier: UInt64 = 0x2545_F491_4F6C_DD1D

/// A deterministic `Rng`: xorshift64*, the same seed always yields the same bytes, and the same bytes as `SeededRng` in Rust, Kotlin and
/// TypeScript. Not cryptographically secure, on purpose. A seed of 0 is replaced by a fixed constant; `fill(n)` consumes `ceil(n / 8)`
/// outputs, little-endian, and drops the unused tail; a fill never returns more than 16 MiB.
public final class SeededRng: UndraAdapter, @unchecked Sendable {
    /// The most bytes one fill returns: 16 MiB.
    public static let maxFill: UInt32 = 1 << 24
    /// The seed of a generator made without one.
    public static let defaultSeed: UInt64 = 0x4B45_454C_5F52_4E47

    private let state: Locked<UInt64>

    /// A generator started from `seed`.
    public init(seed: UInt64 = SeededRng.defaultSeed) {
        self.state = Locked(seed == 0 ? zeroSeedReplacement : seed)
    }

    /// Restarts the sequence from `seed`.
    public func reseed(_ seed: UInt64) {
        state.withLock { $0 = seed == 0 ? zeroSeedReplacement : seed }
    }

    private static func next(_ state: inout UInt64) -> UInt64 {
        var x = state
        x ^= x >> 12
        x ^= x << 25
        x ^= x >> 27
        state = x
        return x &* rngMultiplier
    }

    /// The next 64-bit output.
    public func nextU64() -> UInt64 {
        return state.withLock { SeededRng.next(&$0) }
    }

    /// `len` bytes from the sequence (what `Rng.fill(len)` answers).
    public func fill(_ len: UInt32) -> [UInt8] {
        let n = Int(min(len, SeededRng.maxFill))
        return state.withLock { (s: inout UInt64) -> [UInt8] in
            var out = [UInt8]()
            out.reserveCapacity(n)
            while out.count < n {
                var word = SeededRng.next(&s)
                var i = 0
                while i < 8 && out.count < n {
                    out.append(UInt8(truncatingIfNeeded: word))
                    word >>= 8
                    i += 1
                }
            }
            return out
        }
    }

    public var portId: UInt32 { undraPortId("Rng") }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return .sync([
            undraMethodId("Rng", "fill"): { [self] args in
                var reader = UndraReader(args)
                let len = try reader.readU32()
                try reader.finish()
                return UndraBytes(fill(len)).undraEncoded()
            },
        ])
    }
}

// MARK: - Http

/// Decides whether a scripted reply applies to a request.
public struct HttpMatcher: Sendable {
    private let predicate: @Sendable (HttpRequest) -> Bool

    /// A matcher that asks `predicate`.
    public init(_ predicate: @escaping @Sendable (HttpRequest) -> Bool) {
        self.predicate = predicate
    }

    /// Whether `request` matches.
    public func matches(_ request: HttpRequest) -> Bool {
        return predicate(request)
    }

    /// Every request.
    public static var any: HttpMatcher { HttpMatcher { _ in true } }

    /// Requests to exactly `url`.
    public static func url(_ url: String) -> HttpMatcher { HttpMatcher { $0.url == url } }

    /// Requests whose URL starts with `prefix`.
    public static func urlPrefix(_ prefix: String) -> HttpMatcher { HttpMatcher { $0.url.hasPrefix(prefix) } }

    /// Requests with `method`.
    public static func method(_ method: HttpMethod) -> HttpMatcher { HttpMatcher { $0.method == method } }

    /// Requests both matchers match.
    public func and(_ other: HttpMatcher) -> HttpMatcher {
        let first = self
        return HttpMatcher { first.matches($0) && other.matches($0) }
    }
}

/// What a scripted rule answers: a response, or an error to fail with.
public enum HttpReply: Sendable {
    /// Answer with a response (any status is a response).
    case response(HttpResponse)
    /// Fail with an error.
    case failure(HttpError)
}

/// Builds a response: `body` as UTF-8, `headers` as given.
public func httpResponse(status: UInt16, body: String = "", headers: [(String, String)] = []) -> HttpResponse {
    return HttpResponse(status: status, headers: headers.map { Header(name: $0.0, value: $0.1) }, body: Array(body.utf8))
}

/// An `Http` fake that answers from a script and remembers every request. Rules are tried in the order they were added and the first that
/// matches wins. A request nothing matches fails with `HttpError.network` naming the request, and is still recorded.
public final class FakeHttp: UndraAdapter, @unchecked Sendable {
    private enum Script {
        case fixed(HttpReply)
        case sequence([HttpReply])
        case handler(@Sendable (HttpRequest) async -> HttpReply)
    }

    private struct State {
        var rules: [(HttpMatcher, Script)] = []
        var calls: [HttpRequest] = []
    }

    private let state = Locked(State())

    /// A fake with no rules.
    public init() {}

    /// Answers every request `matcher` matches with `reply`.
    @discardableResult
    public func respond(_ matcher: HttpMatcher, _ reply: HttpReply) -> FakeHttp {
        state.withLock { $0.rules.append((matcher, .fixed(reply))) }
        return self
    }

    /// Answers every request `matcher` matches with `response`.
    @discardableResult
    public func respond(_ matcher: HttpMatcher, _ response: HttpResponse) -> FakeHttp {
        return respond(matcher, .response(response))
    }

    /// Answers every request to exactly `url` with `response`.
    @discardableResult
    public func respond(url: String, _ response: HttpResponse) -> FakeHttp {
        return respond(.url(url), .response(response))
    }

    /// Fails every request `matcher` matches with `error`.
    @discardableResult
    public func fail(_ matcher: HttpMatcher, _ error: HttpError) -> FakeHttp {
        return respond(matcher, .failure(error))
    }

    /// Answers the requests `matcher` matches with `replies`, one each, in order; once used up the rule no longer matches.
    @discardableResult
    public func respondSequence(_ matcher: HttpMatcher, _ replies: [HttpReply]) -> FakeHttp {
        state.withLock { $0.rules.append((matcher, .sequence(replies))) }
        return self
    }

    /// Answers every request `matcher` matches by calling `handler`.
    @discardableResult
    public func respondWith(_ matcher: HttpMatcher, _ handler: @escaping @Sendable (HttpRequest) async -> HttpReply) -> FakeHttp {
        state.withLock { $0.rules.append((matcher, .handler(handler))) }
        return self
    }

    /// Every request received so far, oldest first (unmatched ones included).
    public var calls: [HttpRequest] {
        return state.withLock { $0.calls }
    }

    /// Forgets every rule and every recorded request.
    public func reset() {
        state.withLock { (s: inout State) -> Void in
            s.rules.removeAll()
            s.calls.removeAll()
        }
    }

    /// Performs `request` against the script: throws ``HttpError`` for a scripted or unmatched failure.
    public func request(_ request: HttpRequest) async throws -> HttpResponse {
        enum Found {
            case reply(HttpReply)
            case handler(@Sendable (HttpRequest) async -> HttpReply)
            case none
        }
        let found = state.withLock { (s: inout State) -> Found in
            s.calls.append(request)
            for (index, rule) in s.rules.enumerated() where rule.0.matches(request) {
                switch rule.1 {
                case .fixed(let reply):
                    return .reply(reply)
                case .sequence(var replies):
                    if replies.isEmpty {
                        continue
                    }
                    let next = replies.removeFirst()
                    s.rules[index].1 = .sequence(replies)
                    return .reply(next)
                case .handler(let handler):
                    return .handler(handler)
                }
            }
            return .none
        }
        let reply: HttpReply
        switch found {
        case .reply(let r): reply = r
        case .handler(let h): reply = await h(request)
        case .none: throw HttpError.network("FakeHttp: no scripted response for \(request.method.name) \(request.url)")
        }
        switch reply {
        case .response(let response): return response
        case .failure(let error): throw error
        }
    }

    public var portId: UInt32 { undraPortId("Http") }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return .async([
            undraMethodId("Http", "request"): { [self] args in
                let decoded = try HttpRequest.undraDecoded(from: args)
                do {
                    return try await request(decoded).undraEncoded()
                } catch let error as HttpError {
                    throw UndraPortError(body: error.undraEncoded())
                }
            },
        ])
    }
}

// MARK: - Kv and SecureStore

/// One operation a ``MemStore`` served through its port, for asserting on persistence behaviour.
public struct StoreOp: Sendable, Equatable, CustomStringConvertible {
    /// `get`, `set`, `delete` or `list`.
    public let op: String
    /// The key, or the prefix of a `list`.
    public let key: String

    public var description: String { "\(op)(\(key))" }
}

/// An in-memory key-value store: keys ordered by UTF-8 bytes, operations recorded. ``MemKv`` and ``MemSecureStore`` are its two ports.
public class MemStore: UndraAdapter, @unchecked Sendable {
    private struct State {
        var map: [String: [UInt8]] = [:]
        var ops: [StoreOp] = []
    }

    private let trait: String
    private let state = Locked(State())

    fileprivate init(trait: String) {
        self.trait = trait
    }

    /// Puts `value` under `key` without recording an operation: seeds a test.
    public func insert(_ key: String, _ value: [UInt8]) {
        state.withLock { $0.map[key] = value }
    }

    /// The value under `key`, read directly (not an operation).
    public func value(_ key: String) -> [UInt8]? {
        return state.withLock { $0.map[key] }
    }

    /// The keys, ascending by UTF-8 bytes.
    public func keys() -> [String] {
        return state.withLock { $0.map.keys.sorted { $0.utf8Precedes($1) } }
    }

    /// How many entries.
    public var count: Int {
        return state.withLock { $0.map.count }
    }

    /// The operations served through the port, oldest first.
    public var ops: [StoreOp] {
        return state.withLock { $0.ops }
    }

    private func record(_ op: String, _ key: String) {
        state.withLock { $0.ops.append(StoreOp(op: op, key: key)) }
    }

    /// `get`.
    public func get(_ key: String) async -> [UInt8]? {
        record("get", key)
        return value(key)
    }

    /// `set`.
    public func set(_ key: String, _ value: [UInt8]) async {
        record("set", key)
        insert(key, value)
    }

    /// `delete` (a missing key is not an error).
    public func delete(_ key: String) async {
        record("delete", key)
        _ = state.withLock { $0.map.removeValue(forKey: key) }
    }

    /// `list`: the keys that start with `prefix`, ascending.
    public func list(_ prefix: String) async -> [String] {
        record("list", prefix)
        return keys().filter { $0.hasPrefix(prefix) }
    }

    public var portId: UInt32 { undraPortId(trait) }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        let trait = self.trait
        return .async([
            undraMethodId(trait, "get"): { [self] args in
                var reader = UndraReader(args)
                let key = try reader.readString()
                try reader.finish()
                return (await get(key)).map(UndraBytes.init).undraEncoded()
            },
            undraMethodId(trait, "set"): { [self] args in
                var reader = UndraReader(args)
                let key = try reader.readString()
                let value = try reader.readBytes()
                try reader.finish()
                await set(key, value)
                return []
            },
            undraMethodId(trait, "delete"): { [self] args in
                var reader = UndraReader(args)
                let key = try reader.readString()
                try reader.finish()
                await delete(key)
                return []
            },
            undraMethodId(trait, "list"): { [self] args in
                var reader = UndraReader(args)
                let prefix = try reader.readString()
                try reader.finish()
                return (await list(prefix)).undraEncoded()
            },
        ])
    }
}

/// The `Kv` fake.
public final class MemKv: MemStore, @unchecked Sendable {
    /// An empty store.
    public init() {
        super.init(trait: "Kv")
    }
}

/// The `SecureStore` fake: the same store under its own port id.
public final class MemSecureStore: MemStore, @unchecked Sendable {
    /// An empty store.
    public init() {
        super.init(trait: "SecureStore")
    }
}

// MARK: - Fs

/// Runs `body`, turning an ``FsError`` into the port's typed error reply.
private func fsTyped<T>(_ body: () throws -> T) throws -> T {
    do {
        return try body()
    } catch let error as FsError {
        throw UndraPortError(body: error.undraEncoded())
    }
}

/// An in-memory `Fs` with the semantics the platform adapters share: paths are `/`-separated, empty and `.` segments are ignored and `..`
/// is `denied`; `write` creates missing directories and replaces a file; `read` of a missing path is `notFound`, of a directory `io`;
/// `delete` removes a file or a directory with everything under it; `list` answers the names directly inside a directory, ascending.
public final class MemFs: UndraAdapter, @unchecked Sendable {
    private struct State {
        var files: [String: [UInt8]] = [:]
        var dirs: Set<String> = []
    }

    private let state = Locked(State())

    /// An empty file system.
    public init() {}

    private static func segments(_ path: String) throws -> [String] {
        var parts = [String]()
        for part in path.split(separator: "/", omittingEmptySubsequences: false) {
            if part.isEmpty || part == "." {
                continue
            }
            if part == ".." {
                throw FsError.denied
            }
            parts.append(String(part))
        }
        return parts
    }

    /// Creates the file at `path` without going through the port: seeds a test. Throws ``FsError`` like a write would.
    public func seed(_ path: String, _ contents: [UInt8]) throws {
        try writeFile(path, contents)
    }

    /// The contents of the file at `path`, read directly.
    public func contents(_ path: String) -> [UInt8]? {
        guard let parts = try? MemFs.segments(path) else {
            return nil
        }
        return state.withLock { $0.files[parts.joined(separator: "/")] }
    }

    /// The path of every file, ascending.
    public func filePaths() -> [String] {
        return state.withLock { $0.files.keys.sorted { $0.utf8Precedes($1) } }
    }

    /// `read`.
    public func read(_ path: String) throws -> [UInt8] {
        let parts = try MemFs.segments(path)
        if parts.isEmpty {
            throw FsError.io("the path is empty")
        }
        let key = parts.joined(separator: "/")
        return try state.withLock { (s: inout State) throws -> [UInt8] in
            if let file = s.files[key] {
                return file
            }
            if s.dirs.contains(key) {
                throw FsError.io("is a directory")
            }
            throw FsError.notFound
        }
    }

    /// `write`.
    public func write(_ path: String, _ data: [UInt8]) throws {
        try writeFile(path, data)
    }

    private func writeFile(_ path: String, _ data: [UInt8]) throws {
        let parts = try MemFs.segments(path)
        if parts.isEmpty {
            throw FsError.io("the path is empty")
        }
        let key = parts.joined(separator: "/")
        let parents = parts.dropLast()
        try state.withLock { (s: inout State) throws -> Void in
            if s.dirs.contains(key) {
                throw FsError.io("is a directory")
            }
            var prefix = ""
            for parent in parents {
                prefix = prefix.isEmpty ? parent : prefix + "/" + parent
                if s.files[prefix] != nil {
                    throw FsError.io("not a directory")
                }
            }
            prefix = ""
            for parent in parents {
                prefix = prefix.isEmpty ? parent : prefix + "/" + parent
                s.dirs.insert(prefix)
            }
            s.files[key] = data
        }
    }

    /// `delete`.
    public func delete(_ path: String) throws {
        let parts = try MemFs.segments(path)
        if parts.isEmpty {
            throw FsError.io("the path is empty")
        }
        let key = parts.joined(separator: "/")
        try state.withLock { (s: inout State) throws -> Void in
            if s.files.removeValue(forKey: key) != nil {
                return
            }
            if s.dirs.remove(key) == nil {
                throw FsError.notFound
            }
            let below = key + "/"
            s.files = s.files.filter { !$0.key.hasPrefix(below) }
            s.dirs = s.dirs.filter { !$0.hasPrefix(below) }
        }
    }

    /// `list`.
    public func list(_ dir: String) throws -> [String] {
        let parts = try MemFs.segments(dir)
        let key = parts.joined(separator: "/")
        return try state.withLock { (s: inout State) throws -> [String] in
            if !parts.isEmpty {
                if s.files[key] != nil {
                    throw FsError.io("not a directory")
                }
                if !s.dirs.contains(key) {
                    throw FsError.notFound
                }
            }
            let prefix = parts.isEmpty ? "" : key + "/"
            var names = Set<String>()
            for path in Array(s.files.keys) + Array(s.dirs) where path.hasPrefix(prefix) {
                let name = path.dropFirst(prefix.count).split(separator: "/", omittingEmptySubsequences: false).first.map(String.init) ?? ""
                if !name.isEmpty {
                    names.insert(name)
                }
            }
            return names.sorted { $0.utf8Precedes($1) }
        }
    }

    public var portId: UInt32 { undraPortId("Fs") }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return .async([
            undraMethodId("Fs", "read"): { [self] args in
                var reader = UndraReader(args)
                let path = try reader.readString()
                try reader.finish()
                return try fsTyped { UndraBytes(try read(path)).undraEncoded() }
            },
            undraMethodId("Fs", "write"): { [self] args in
                var reader = UndraReader(args)
                let path = try reader.readString()
                let data = try reader.readBytes()
                try reader.finish()
                try fsTyped { try write(path, data) }
                return []
            },
            undraMethodId("Fs", "delete"): { [self] args in
                var reader = UndraReader(args)
                let path = try reader.readString()
                try reader.finish()
                try fsTyped { try delete(path) }
                return []
            },
            undraMethodId("Fs", "list"): { [self] args in
                var reader = UndraReader(args)
                let dir = try reader.readString()
                try reader.finish()
                return try fsTyped { try list(dir).undraEncoded() }
            },
        ])
    }
}

// MARK: - Log

/// One record the core logged.
public struct LogEntry: Sendable, Equatable {
    public let level: UInt8
    public let target: String
    public let message: String
}

/// A `Log` that keeps every record.
public final class CaptureLog: UndraAdapter, @unchecked Sendable {
    private let recorded = Locked<[LogEntry]>([])

    /// An empty log.
    public init() {}

    /// The records so far.
    public var entries: [LogEntry] {
        return recorded.withLock { $0 }
    }

    /// The messages so far.
    public func messages() -> [String] {
        return entries.map { $0.message }
    }

    /// Whether any message contains `needle`.
    public func contains(_ needle: String) -> Bool {
        return entries.contains { $0.message.contains(needle) }
    }

    /// Forgets the records.
    public func clear() {
        recorded.withLock { $0.removeAll() }
    }

    public var portId: UInt32 { undraPortId("Log") }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return .sync([
            undraMethodId("Log", "log"): { [self] args in
                var reader = UndraReader(args)
                let level = try reader.readU8()
                let target = try reader.readString()
                let message = try reader.readString()
                try reader.finish()
                recorded.withLock { $0.append(LogEntry(level: level, target: target, message: message)) }
                return []
            },
        ])
    }
}

// MARK: - Diagnostics

/// A `Diagnostics` that keeps every panic report the core hands it (ADR-046): what a test asserts on instead of a crash reporter.
///
/// It records on the thread the core reported from, at once, in order; unlike the runtime's default adapter it does not hop to the main
/// thread and does not call `LoadOptions.onPanic`. Reports that do not decode are dropped, as the default adapter drops them.
///
/// ```swift
/// let fakes = Fakes()
/// let preview = try PreviewCore.load(UndraPlaygroundCore.load, fakes: fakes)
/// _ = try? explode(reason: "boom", ctx: preview.core)
/// XCTAssertEqual(fakes.diagnostics.reports.first?.operation, "explode")
/// ```
public final class CaptureDiagnostics: UndraAdapter, @unchecked Sendable {
    private let recorded = Locked<[UndraPanicReport]>([])

    /// An adapter with no reports.
    public init() {}

    /// The reports so far, oldest first.
    public var reports: [UndraPanicReport] {
        return recorded.withLock { $0 }
    }

    /// Whether any report's message contains `needle`.
    public func contains(_ needle: String) -> Bool {
        return reports.contains { $0.message.contains(needle) }
    }

    /// Forgets the reports.
    public func clear() {
        recorded.withLock { $0.removeAll() }
    }

    public var portId: UInt32 { undraPortId("Diagnostics") }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return .sync([
            undraMethodId("Diagnostics", "panicked"): { [self] args in
                if let report = try? UndraPanicReport.undraDecoded(from: args) {
                    recorded.withLock { $0.append(report) }
                }
                return []
            },
        ])
    }
}

// MARK: - Connectivity and Lifecycle

/// A source of connectivity changes a test or a preview drives: ``set(online:kind:)`` reports the new state to the core it is attached to.
public final class ScriptedConnectivity: UndraAdapter, @unchecked Sendable {
    private struct State {
        var online = true
        var kind = NetKind.wifi
        weak var core: UndraCore?
    }

    private let state = Locked(State())

    /// A source that starts online on Wi-Fi.
    public init() {}

    /// The state the app is in.
    public var current: (online: Bool, kind: NetKind) {
        return state.withLock { (online: $0.online, kind: $0.kind) }
    }

    public var portId: UInt32 { undraPortId("Connectivity") }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return nil
    }

    /// Reports the current state to `core` (like a platform monitor does when it starts) and every change from now on.
    public func attach(to core: UndraCore) {
        state.withLock { $0.core = core }
        emit()
    }

    /// Changes the state and tells the core, if attached.
    public func set(online: Bool, kind: NetKind) {
        state.withLock { (s: inout State) -> Void in
            s.online = online
            s.kind = kind
        }
        emit()
    }

    /// Goes offline.
    public func goOffline() {
        set(online: false, kind: .disconnected)
    }

    /// Comes back online on `kind`.
    public func goOnline(kind: NetKind = .wifi) {
        set(online: true, kind: kind)
    }

    private func emit() {
        let (core, online, kind) = state.withLock { ($0.core, $0.online, $0.kind) }
        var writer = UndraWriter()
        writer.writeBool(online)
        kind.undraEncode(&writer)
        core?.event(port: undraPortId("Connectivity"), method: undraMethodId("Connectivity", "changed"), payload: writer.finish())
    }
}

/// A source of lifecycle changes a test or a preview drives.
public final class ScriptedLifecycle: UndraAdapter, @unchecked Sendable {
    private struct State {
        var value = UndraAppState.active
        weak var core: UndraCore?
    }

    private let state = Locked(State())

    /// A source that starts active.
    public init() {}

    /// The state the app is in.
    public var current: UndraAppState {
        return state.withLock { $0.value }
    }

    public var portId: UInt32 { undraPortId("Lifecycle") }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return nil
    }

    /// Reports the current state to `core` and every change from now on.
    public func attach(to core: UndraCore) {
        state.withLock { $0.core = core }
        emit()
    }

    /// Changes the state and tells the core, if attached.
    public func set(_ value: UndraAppState) {
        state.withLock { $0.value = value }
        emit()
    }

    private func emit() {
        let (core, value) = state.withLock { ($0.core, $0.value) }
        core?.event(port: undraPortId("Lifecycle"), method: undraMethodId("Lifecycle", "changed"), payload: value.undraEncoded())
    }
}

// MARK: - The bundle

/// One of each fake in the documented default state: the default time and seed, empty stores, no scripted replies, no panic reports, online on Wi-Fi, active.
public final class Fakes: @unchecked Sendable {
    /// The `Clock` and the `Timer`.
    public let clock = FakeClock()
    /// The `Rng`.
    public let rng = SeededRng()
    /// The `Log`.
    public let log = CaptureLog()
    /// The `Diagnostics`: the panic reports of the core (ADR-046).
    public let diagnostics = CaptureDiagnostics()
    /// The `Http`.
    public let http = FakeHttp()
    /// The `Kv`.
    public let kv = MemKv()
    /// The `SecureStore`.
    public let secureStore = MemSecureStore()
    /// The `Fs`.
    public let fs = MemFs()
    /// The source of `Connectivity` events.
    public let connectivity = ScriptedConnectivity()
    /// The source of `Lifecycle` events.
    public let lifecycle = ScriptedLifecycle()

    /// Fresh fakes.
    public init() {}

    /// Every fake as the adapters of a core: what `LoadOptions.adapters` takes.
    ///
    /// A port is registered after the core started, and the core's start-up work (the query cache is read from the `Kv` at once) runs
    /// concurrently with the registrations (docs/SPEC.md section 6), so the stores come first, then the rest.
    public func adapters() -> Adapters {
        return Adapters([
            kv, secureStore, fs, http, clock.clockAdapter, clock.timerAdapter, rng, log, diagnostics, connectivity, lifecycle,
        ])
    }
}
