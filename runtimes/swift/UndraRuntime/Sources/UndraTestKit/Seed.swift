import Foundation
import UndraRuntime

/// The `version` of the seed document this kit reads.
public let seedVersion = 1

/// Why a seed could not be read or applied: `path` says where (`http[1].status`).
public struct SeedError: Error, CustomStringConvertible, Equatable, Sendable {
    /// Where in the document.
    public let path: String
    /// What is wrong.
    public let problem: String

    public var description: String { "seed \(path): \(problem)" }
}

/// One scripted HTTP rule of a ``Seed``.
public struct SeedHttpRule: Sendable {
    public var url: String?
    public var urlPrefix: String?
    public var method: HttpMethod?
    public var reply: HttpReply

    public init(url: String? = nil, urlPrefix: String? = nil, method: HttpMethod? = nil, reply: HttpReply) {
        self.url = url
        self.urlPrefix = urlPrefix
        self.method = method
        self.reply = reply
    }
}

/// The starting state of the fakes: the same document seeds `undra::ports::fakes` in Rust and the fakes of the Kotlin and TypeScript kits
/// (`testkit/fixtures/seed.json` is an example). Every field is optional. Build one with its initializer or read one with
/// ``init(json:)``, then ``apply(to:)`` it to ``Fakes``.
public struct Seed: Sendable {
    public var nowMs: Int64?
    public var rngSeed: UInt64?
    public var kv: [(String, [UInt8])]
    public var secureStore: [(String, [UInt8])]
    public var fs: [(String, [UInt8])]
    public var http: [SeedHttpRule]
    public var connectivity: (online: Bool, kind: NetKind)?
    public var lifecycle: UndraAppState?

    public init(
        nowMs: Int64? = nil,
        rngSeed: UInt64? = nil,
        kv: [(String, [UInt8])] = [],
        secureStore: [(String, [UInt8])] = [],
        fs: [(String, [UInt8])] = [],
        http: [SeedHttpRule] = [],
        connectivity: (online: Bool, kind: NetKind)? = nil,
        lifecycle: UndraAppState? = nil
    ) {
        self.nowMs = nowMs
        self.rngSeed = rngSeed
        self.kv = kv
        self.secureStore = secureStore
        self.fs = fs
        self.http = http
        self.connectivity = connectivity
        self.lifecycle = lifecycle
    }

    /// Puts the seed into `fakes`.
    ///
    /// - Throws: ``SeedError`` for an `fs` path the fake refuses (an empty path, `..`, a file in the way).
    public func apply(to fakes: Fakes) throws {
        if let nowMs = nowMs {
            fakes.clock.setNowMs(nowMs)
        }
        if let rngSeed = rngSeed {
            fakes.rng.reseed(rngSeed)
        }
        for (key, value) in kv {
            fakes.kv.insert(key, value)
        }
        for (key, value) in secureStore {
            fakes.secureStore.insert(key, value)
        }
        for (path, contents) in fs {
            do {
                try fakes.fs.seed(path, contents)
            } catch {
                throw SeedError(path: "fs.\(path)", problem: "\(error)")
            }
        }
        for rule in http {
            var matcher = HttpMatcher.any
            if let url = rule.url {
                matcher = matcher.and(.url(url))
            }
            if let prefix = rule.urlPrefix {
                matcher = matcher.and(.urlPrefix(prefix))
            }
            if let method = rule.method {
                matcher = matcher.and(.method(method))
            }
            fakes.http.respond(matcher, rule.reply)
        }
        if let connectivity = connectivity {
            fakes.connectivity.set(online: connectivity.online, kind: connectivity.kind)
        }
        if let lifecycle = lifecycle {
            fakes.lifecycle.set(lifecycle)
        }
    }

    /// Reads a seed document.
    ///
    /// - Throws: ``SeedError`` naming the path of the first malformed value.
    public init(json text: String) throws {
        self = try readSeed(text)
    }
}

private func bytesOf(_ path: String, _ v: JSON?) throws -> [UInt8] {
    if case .string(let text)? = v {
        return Array(text.utf8)
    }
    if case .object(let fields)? = v, case .string(let hex)? = fields["hex"], let bytes = hex.fromHex() {
        return bytes
    }
    throw SeedError(path: path, problem: "must be a string or {\"hex\": \"..\"}")
}

private func entries(_ doc: [String: JSON], _ key: String) throws -> [(String, [UInt8])] {
    guard let v = doc[key] else {
        return []
    }
    guard let map = v.object else {
        throw SeedError(path: key, problem: "must be an object")
    }
    return try map.keys.sorted().map { ($0, try bytesOf("\(key).\($0)", map[$0])) }
}

private func named<T>(_ table: [String: T], _ path: String, _ v: JSON?) throws -> T {
    guard let name = v?.string else {
        throw SeedError(path: path, problem: "must be a string")
    }
    guard let value = table[name] else {
        throw SeedError(path: path, problem: "is not one of \(table.keys.sorted().joined(separator: ", "))")
    }
    return value
}

private func httpError(_ path: String, _ v: JSON?) throws -> HttpError {
    if case .string("timeout")? = v {
        return .timeout
    }
    if case .string("cancelled")? = v {
        return .cancelled
    }
    if case .object(let fields)? = v {
        if case .string(let message)? = fields["network"] {
            return .network(message)
        }
        if case .string(let url)? = fields["invalid_url"] {
            return .invalidUrl(url)
        }
    }
    throw SeedError(path: path, problem: "must be \"timeout\", \"cancelled\", {\"network\": ..} or {\"invalid_url\": ..}")
}

private func httpRule(_ i: Int, _ v: JSON) throws -> SeedHttpRule {
    func at(_ field: String) -> String { "http[\(i)].\(field)" }
    guard let obj = v.object else {
        throw SeedError(path: "http[\(i)]", problem: "must be an object")
    }
    func text(_ field: String) throws -> String? {
        guard let x = obj[field] else {
            return nil
        }
        guard let s = x.string else {
            throw SeedError(path: at(field), problem: "must be a string")
        }
        return s
    }
    let methods = Dictionary(uniqueKeysWithValues: HttpMethod.allCases.map { ($0.name.lowercased(), $0) })
    let method = try obj["method"].map { try named(methods, at("method"), $0) }
    let reply: HttpReply
    if let error = obj["error"] {
        reply = .failure(try httpError(at("error"), error))
    } else {
        var status: UInt16 = 200
        if let s = obj["status"] {
            guard case .number(let raw) = s, let n = UInt16(raw) else {
                throw SeedError(path: at("status"), problem: "must be a status code")
            }
            status = n
        }
        let body = try obj["body"].map { try bytesOf(at("body"), $0) } ?? []
        var headers = [Header]()
        if let h = obj["headers"] {
            guard let list = h.array else {
                throw SeedError(path: at("headers"), problem: "must be a list of [name, value]")
            }
            for pair in list {
                guard let items = pair.array, items.count == 2, let name = items[0].string, let value = items[1].string else {
                    throw SeedError(path: at("headers"), problem: "must be a list of [name, value]")
                }
                headers.append(Header(name: name, value: value))
            }
        }
        reply = .response(HttpResponse(status: status, headers: headers, body: body))
    }
    return SeedHttpRule(url: try text("url"), urlPrefix: try text("url_prefix"), method: method, reply: reply)
}

private func readSeed(_ text: String) throws -> Seed {
    let doc: JSON
    do {
        doc = try parseJSON(text)
    } catch let error as JSONError {
        throw SeedError(path: "$", problem: "is not valid JSON: \(error.message)")
    }
    guard let obj = doc.object else {
        throw SeedError(path: "$", problem: "must be an object")
    }
    if let v = obj["version"] {
        guard case .number(let raw) = v, raw == String(seedVersion) else {
            throw SeedError(path: "version", problem: "is not supported (this reader knows \(seedVersion))")
        }
    }
    var nowMs: Int64?
    if let n = obj["now_ms"] {
        guard let value = n.int else {
            throw SeedError(path: "now_ms", problem: "must be an integer")
        }
        nowMs = value
    }
    var rngSeed: UInt64?
    switch obj["rng_seed"] {
    case nil:
        rngSeed = nil
    case .number(let raw)?:
        guard let n = UInt64(raw) else {
            throw SeedError(path: "rng_seed", problem: "must be a non-negative integer")
        }
        rngSeed = n
    case .string(let hex)?:
        guard let n = parseHex64(hex) else {
            throw SeedError(path: "rng_seed", problem: "a string seed must be \"0x..\"")
        }
        rngSeed = n
    default:
        throw SeedError(path: "rng_seed", problem: "must be an integer or a \"0x..\" string")
    }
    var http = [SeedHttpRule]()
    if let h = obj["http"] {
        guard let list = h.array else {
            throw SeedError(path: "http", problem: "must be a list")
        }
        for (i, r) in list.enumerated() {
            http.append(try httpRule(i, r))
        }
    }
    var connectivity: (online: Bool, kind: NetKind)?
    if let c = obj["connectivity"] {
        guard let fields = c.object, let online = fields["online"]?.bool else {
            throw SeedError(path: "connectivity.online", problem: "must be true or false")
        }
        let kinds: [String: NetKind] = ["wifi": .wifi, "cellular": .cellular, "wired": .wired, "unknown": .unknown, "none": .disconnected]
        connectivity = (online, try named(kinds, "connectivity.kind", fields["kind"]))
    }
    let states: [String: UndraAppState] = ["active": .active, "inactive": .inactive, "background": .background]
    let lifecycle = try obj["lifecycle"].map { try named(states, "lifecycle", $0) }
    return Seed(
        nowMs: nowMs, rngSeed: rngSeed, kv: try entries(obj, "kv"), secureStore: try entries(obj, "secure_store"), fs: try entries(obj, "fs"),
        http: http, connectivity: connectivity, lifecycle: lifecycle
    )
}
