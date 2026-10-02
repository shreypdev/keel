import Foundation

/// The `format` of a recording.
public let recordingFormat = "undra.recording"

/// The `version` this kit reads and writes.
public let recordingVersion = 1

/// Why a recording could not be read: names the event (`event` is `nil` for the document header) and the `field`.
public struct RecordingError: Error, CustomStringConvertible, Equatable, Sendable {
    /// What is wrong, in a sentence.
    public let message: String
    /// The index of the event, or `nil` for the document header.
    public let event: Int?
    /// The field that is missing or malformed.
    public let field: String?

    public var description: String { message }
}

/// Which call a `call` event is (docs/SPEC.md section 3.3).
public enum RecordedTarget: Sendable, Equatable {
    /// A free function.
    case function(method: UInt32)
    /// A method of the object `handle`.
    case method(handle: UInt64, method: UInt32)
    /// A constructor of the object type `type`.
    case constructor(type: UInt32, method: UInt32)
    /// A page of a lazy list.
    case page(handle: UInt64, offset: UInt32, limit: UInt32)
}

/// The status of a recorded `reply`; the raw value is its name in the file.
public enum ReplyStatusName: String, Sendable, CaseIterable {
    case ok, error, panic, cancelled
    case streamOpened = "stream_opened"
    case badRequest = "bad_request"

    var code: UInt8 {
        switch self {
        case .ok: return 0
        case .error: return 1
        case .panic: return 2
        case .cancelled: return 3
        case .streamOpened: return 4
        case .badRequest: return 5
        }
    }
}

/// The status of a recorded `port_reply`.
public enum PortStatusName: String, Sendable, CaseIterable {
    case ok, error, unavailable
}

/// What a recorded `stream_item` carries.
public enum StreamFlagName: String, Sendable, CaseIterable {
    case item, end, error, failed

    var code: UInt8 {
        switch self {
        case .item: return 0
        case .end: return 1
        case .error: return 2
        case .failed: return 3
        }
    }
}

/// How a recorded change-set entry's value reads.
public enum ChangeOpName: String, Sendable, CaseIterable {
    case full, patch
    case lazyInvalidated = "lazy_invalidated"

    var code: UInt8 {
        switch self {
        case .full: return 0
        case .patch: return 1
        case .lazyInvalidated: return 2
        }
    }
}

/// One signal update of a recorded change-set.
public struct RecordedEntry: Sendable, Equatable {
    public var handle: UInt64
    public var signal: UInt32
    public var op: ChangeOpName
    public var value: [UInt8]

    public init(handle: UInt64, signal: UInt32, op: ChangeOpName, value: [UInt8]) {
        self.handle = handle
        self.signal = signal
        self.op = op
        self.value = value
    }
}

/// What happened.
public enum RecordedKind: Sendable, Equatable {
    /// Host to core: a call.
    case call(target: RecordedTarget, call: UInt32, args: [UInt8])
    /// Core to host: a call's reply; `body` is the SPEC 3.4 body of `status`.
    case reply(call: UInt32, status: ReplyStatusName, body: [UInt8])
    /// Core to host: one transaction's updates.
    case changeSet(txn: UInt64, entries: [RecordedEntry])
    /// Core to host: an item, end or failure of a stream.
    case streamItem(call: UInt32, flag: StreamFlagName, body: [UInt8])
    /// Core to host: the core calls a port.
    case portCall(port: UInt32, method: UInt32, call: UInt32, args: [UInt8])
    /// Host to core: the answer to a port call.
    case portReply(call: UInt32, status: PortStatusName, body: [UInt8])
    /// Host to core: an event of an event port.
    case portEvent(port: UInt32, method: UInt32, payload: [UInt8])
    /// Host to core: a platform timer fired.
    case timerFired(timer: UInt32)
    /// Host to core: start or stop observing a signal (`UInt32.max` is every signal).
    case observe(handle: UInt64, signal: UInt32, on: Bool)
    /// Host to core: an object was released.
    case release(handle: UInt64)
    /// Host to core: a call or stream was cancelled.
    case cancel(call: UInt32)
}

/// One event and when it happened: whole milliseconds since the session started.
public struct RecordedEvent: Sendable, Equatable {
    public var t: UInt64
    public var kind: RecordedKind

    public init(t: UInt64, kind: RecordedKind) {
        self.t = t
        self.kind = kind
    }

    /// The event as one line of canonical JSON.
    public func jsonLine() -> String {
        return eventLine(self)
    }
}

/// A recorded session (`undra.recording`, version 1): the events of one core's life in order, with the schema hash of that core.
/// `toJSON()` writes the canonical text (the same bytes the Rust, Kotlin and TypeScript writers produce); `Recording(json:)` reads it.
public struct Recording: Sendable, Equatable {
    /// The schema hash of the core the bytes belong to: a replay on another schema is refused.
    public var schemaHash: UInt64
    /// Where it came from, informational: `"dev-server"`, `"adapters"`, `"test"`, `"hand"`.
    public var source: String
    /// The host platform, informational.
    public var platform: String?
    /// The events, in the order they happened.
    public var events: [RecordedEvent]

    public init(schemaHash: UInt64, source: String, platform: String? = nil, events: [RecordedEvent] = []) {
        self.schemaHash = schemaHash
        self.source = source
        self.platform = platform
        self.events = events
    }

    /// Reads a recording.
    ///
    /// - Throws: ``RecordingError`` for text that is not JSON, another `format`, a `version` this kit does not read, or a field that is missing
    ///   or malformed (named, with the event's index).
    public init(json text: String) throws {
        self = try readRecording(text)
    }

    /// The canonical JSON text, ending with a newline: equal recordings are equal bytes.
    public func toJSON() -> String {
        var out = "{\n  \"format\": \"\(recordingFormat)\",\n  \"version\": \(recordingVersion),\n"
        out += "  \"schema_hash\": \"\(hex64(schemaHash))\",\n  \"source\": \(jsonString(source)),\n"
        if let platform = platform {
            out += "  \"platform\": \(jsonString(platform)),\n"
        }
        if events.isEmpty {
            return out + "  \"events\": []\n}\n"
        }
        out += "  \"events\": [\n"
        for (index, event) in events.enumerated() {
            out += "    " + event.jsonLine() + (index + 1 < events.count ? ",\n" : "\n")
        }
        return out + "  ]\n}\n"
    }
}

private func nameField(_ port: UInt32, _ method: UInt32) -> String {
    if let name = undraStandardName(port: port, method: method) {
        return ",\"name\":\(jsonString(name))"
    }
    return ""
}

private func eventLine(_ e: RecordedEvent) -> String {
    let head = "{\"t\":\(e.t)"
    switch e.kind {
    case .call(let target, let call, let args):
        let t: String
        switch target {
        case .function(let method):
            t = "\"target\":\"function\",\"method\":\(method)"
        case .method(let handle, let method):
            t = "\"target\":\"method\",\"handle\":\"\(hex64(handle))\",\"method\":\(method)"
        case .constructor(let type, let method):
            t = "\"target\":\"constructor\",\"type\":\(type),\"method\":\(method)"
        case .page(let handle, let offset, let limit):
            t = "\"target\":\"page\",\"handle\":\"\(hex64(handle))\",\"offset\":\(offset),\"limit\":\(limit)"
        }
        return "\(head),\"kind\":\"call\",\(t),\"call\":\(call),\"args\":\"\(args.hex)\"}"
    case .reply(let call, let status, let body):
        return "\(head),\"kind\":\"reply\",\"call\":\(call),\"status\":\"\(status.rawValue)\",\"body\":\"\(body.hex)\"}"
    case .changeSet(let txn, let entries):
        let list = entries.map {
            "{\"handle\":\"\(hex64($0.handle))\",\"signal\":\($0.signal),\"op\":\"\($0.op.rawValue)\",\"value\":\"\($0.value.hex)\"}"
        }.joined(separator: ",")
        return "\(head),\"kind\":\"change_set\",\"txn\":\(txn),\"entries\":[\(list)]}"
    case .streamItem(let call, let flag, let body):
        return "\(head),\"kind\":\"stream_item\",\"call\":\(call),\"flag\":\"\(flag.rawValue)\",\"body\":\"\(body.hex)\"}"
    case .portCall(let port, let method, let call, let args):
        return "\(head),\"kind\":\"port_call\",\"port\":\(port),\"method\":\(method),\"call\":\(call),\"args\":\"\(args.hex)\"\(nameField(port, method))}"
    case .portReply(let call, let status, let body):
        return "\(head),\"kind\":\"port_reply\",\"call\":\(call),\"status\":\"\(status.rawValue)\",\"body\":\"\(body.hex)\"}"
    case .portEvent(let port, let method, let payload):
        return "\(head),\"kind\":\"event\",\"port\":\(port),\"method\":\(method),\"payload\":\"\(payload.hex)\"\(nameField(port, method))}"
    case .timerFired(let timer):
        return "\(head),\"kind\":\"timer_fired\",\"timer\":\(timer)}"
    case .observe(let handle, let signal, let on):
        return "\(head),\"kind\":\"observe\",\"handle\":\"\(hex64(handle))\",\"signal\":\(signal),\"on\":\(on)}"
    case .release(let handle):
        return "\(head),\"kind\":\"release\",\"handle\":\"\(hex64(handle))\"}"
    case .cancel(let call):
        return "\(head),\"kind\":\"cancel\",\"call\":\(call)}"
    }
}

/// A typed view of one JSON object, reporting the first bad field.
private struct Fields {
    let index: Int?
    let obj: [String: JSON]

    func bad(_ field: String, _ problem: String) -> RecordingError {
        let where_ = index.map { "event \($0): " } ?? ""
        return RecordingError(message: "\(where_)\"\(field)\" \(problem)", event: index, field: field)
    }

    func u64(_ field: String) throws -> UInt64 {
        guard case .number(let raw)? = obj[field], let n = UInt64(raw) else {
            throw bad(field, "must be a non-negative integer")
        }
        return n
    }

    func u32(_ field: String) throws -> UInt32 {
        let n = try u64(field)
        guard n <= UInt64(UInt32.max) else {
            throw bad(field, "does not fit a u32")
        }
        return UInt32(n)
    }

    func str(_ field: String) throws -> String {
        guard let s = obj[field]?.string else {
            throw bad(field, "must be a string")
        }
        return s
    }

    func handle(_ field: String) throws -> UInt64 {
        guard let h = parseHex64(try str(field)) else {
            throw bad(field, "must be a \"0x..\" string")
        }
        return h
    }

    func bytes(_ field: String) throws -> [UInt8] {
        guard let b = try str(field).fromHex() else {
            throw bad(field, "must be hex")
        }
        return b
    }

    func bool(_ field: String) throws -> Bool {
        guard let b = obj[field]?.bool else {
            throw bad(field, "must be true or false")
        }
        return b
    }

    func oneOf<T: RawRepresentable>(_ field: String, _ type: T.Type, _ what: String) throws -> T where T.RawValue == String {
        guard let v = T(rawValue: try str(field)) else {
            throw bad(field, "is not \(what)")
        }
        return v
    }
}

private func parseEvent(_ index: Int, _ raw: JSON) throws -> RecordedEvent {
    guard let obj = raw.object else {
        throw RecordingError(message: "event \(index) must be an object", event: index, field: nil)
    }
    let f = Fields(index: index, obj: obj)
    let t = try f.u64("t")
    let kind: RecordedKind
    switch try f.str("kind") {
    case "call":
        let target: RecordedTarget
        switch try f.str("target") {
        case "function": target = .function(method: try f.u32("method"))
        case "method": target = .method(handle: try f.handle("handle"), method: try f.u32("method"))
        case "constructor": target = .constructor(type: try f.u32("type"), method: try f.u32("method"))
        case "page": target = .page(handle: try f.handle("handle"), offset: try f.u32("offset"), limit: try f.u32("limit"))
        default: throw f.bad("target", "is not function, method, constructor or page")
        }
        kind = .call(target: target, call: try f.u32("call"), args: try f.bytes("args"))
    case "reply":
        kind = .reply(call: try f.u32("call"), status: try f.oneOf("status", ReplyStatusName.self, "a reply status"), body: try f.bytes("body"))
    case "change_set":
        guard let list = obj["entries"]?.array else {
            throw f.bad("entries", "must be an array")
        }
        var entries = [RecordedEntry]()
        for e in list {
            guard let eo = e.object else {
                throw f.bad("entries", "must hold objects")
            }
            let ef = Fields(index: index, obj: eo)
            entries.append(RecordedEntry(
                handle: try ef.handle("handle"),
                signal: try ef.u32("signal"),
                op: try ef.oneOf("op", ChangeOpName.self, "full, patch or lazy_invalidated"),
                value: try ef.bytes("value")
            ))
        }
        kind = .changeSet(txn: try f.u64("txn"), entries: entries)
    case "stream_item":
        kind = .streamItem(call: try f.u32("call"), flag: try f.oneOf("flag", StreamFlagName.self, "item, end, error or failed"), body: try f.bytes("body"))
    case "port_call":
        kind = .portCall(port: try f.u32("port"), method: try f.u32("method"), call: try f.u32("call"), args: try f.bytes("args"))
    case "port_reply":
        kind = .portReply(call: try f.u32("call"), status: try f.oneOf("status", PortStatusName.self, "ok, error or unavailable"), body: try f.bytes("body"))
    case "event":
        kind = .portEvent(port: try f.u32("port"), method: try f.u32("method"), payload: try f.bytes("payload"))
    case "timer_fired":
        kind = .timerFired(timer: try f.u32("timer"))
    case "observe":
        kind = .observe(handle: try f.handle("handle"), signal: try f.u32("signal"), on: try f.bool("on"))
    case "release":
        kind = .release(handle: try f.handle("handle"))
    case "cancel":
        kind = .cancel(call: try f.u32("call"))
    default:
        throw f.bad("kind", "is not a known event kind")
    }
    return RecordedEvent(t: t, kind: kind)
}

private func readRecording(_ text: String) throws -> Recording {
    let doc: JSON
    do {
        doc = try parseJSON(text)
    } catch let error as JSONError {
        throw RecordingError(message: "the recording is not valid JSON: \(error.message)", event: nil, field: nil)
    }
    guard let obj = doc.object else {
        throw RecordingError(message: "the recording must be a JSON object", event: nil, field: nil)
    }
    let format = obj["format"]?.string ?? ""
    if format != recordingFormat {
        throw RecordingError(message: "not a recording: format is \"\(format)\", expected \"\(recordingFormat)\"", event: nil, field: "format")
    }
    var version = "(none)"
    if case .number(let raw)? = obj["version"] {
        version = raw
    }
    if version != String(recordingVersion) {
        throw RecordingError(message: "recording version \(version) is not supported (this reader knows version \(recordingVersion))", event: nil, field: "version")
    }
    let head = Fields(index: nil, obj: obj)
    guard let schemaHash = parseHex64(try head.str("schema_hash")) else {
        throw head.bad("schema_hash", "must be a \"0x..\" string")
    }
    let source = try head.str("source")
    var platform: String?
    switch obj["platform"] {
    case nil, .null?:
        platform = nil
    case .string(let p)?:
        platform = p
    default:
        throw head.bad("platform", "must be a string")
    }
    guard let events = obj["events"]?.array else {
        throw head.bad("events", "must be an array")
    }
    var parsed = [RecordedEvent]()
    for (i, e) in events.enumerated() {
        parsed.append(try parseEvent(i, e))
    }
    return Recording(schemaHash: schemaHash, source: source, platform: platform, events: parsed)
}
