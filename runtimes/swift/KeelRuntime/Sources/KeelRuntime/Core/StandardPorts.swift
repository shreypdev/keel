// The standard ports (docs/SPEC.md section 8): their ids and the hand-written wire codecs of the
// records their methods exchange.
//
// This file needs no Foundation; the adapters that implement the ports import what they need.
// The record types are internal on purpose: generated modules declare their own `HttpRequest`,
// `HttpError`, `NetKind`, ... and a public twin here would make those names ambiguous in every
// app that imports both modules.

// MARK: - Ids

/// `fnv1a32("port.<Trait>")` and `fnv1a32("<Trait>.<method>")` for the standard ports.
enum StandardPorts {
    enum Clock {
        static let portId: UInt32 = fnv1a32("port.Clock")
        static let nowMs: UInt32 = fnv1a32("Clock.now_ms")
        static let monotonicNs: UInt32 = fnv1a32("Clock.monotonic_ns")
    }

    enum Rng {
        static let portId: UInt32 = fnv1a32("port.Rng")
        static let fill: UInt32 = fnv1a32("Rng.fill")
    }

    enum Log {
        static let portId: UInt32 = fnv1a32("port.Log")
        static let log: UInt32 = fnv1a32("Log.log")
    }

    enum Http {
        static let portId: UInt32 = fnv1a32("port.Http")
        static let request: UInt32 = fnv1a32("Http.request")
    }

    enum Kv {
        static let portId: UInt32 = fnv1a32("port.Kv")
        static let get: UInt32 = fnv1a32("Kv.get")
        static let set: UInt32 = fnv1a32("Kv.set")
        static let delete: UInt32 = fnv1a32("Kv.delete")
        static let list: UInt32 = fnv1a32("Kv.list")
    }

    enum SecureStore {
        static let portId: UInt32 = fnv1a32("port.SecureStore")
        static let get: UInt32 = fnv1a32("SecureStore.get")
        static let set: UInt32 = fnv1a32("SecureStore.set")
        static let delete: UInt32 = fnv1a32("SecureStore.delete")
        static let list: UInt32 = fnv1a32("SecureStore.list")
    }

    enum Fs {
        static let portId: UInt32 = fnv1a32("port.Fs")
        static let read: UInt32 = fnv1a32("Fs.read")
        static let write: UInt32 = fnv1a32("Fs.write")
        static let delete: UInt32 = fnv1a32("Fs.delete")
        static let list: UInt32 = fnv1a32("Fs.list")
    }

    enum Timer {
        static let portId: UInt32 = fnv1a32("port.Timer")
        static let set: UInt32 = fnv1a32("Timer.set")
    }

    enum Connectivity {
        static let portId: UInt32 = fnv1a32("port.Connectivity")
        static let changed: UInt32 = fnv1a32("Connectivity.changed")
    }

    enum Lifecycle {
        static let portId: UInt32 = fnv1a32("port.Lifecycle")
        static let changed: UInt32 = fnv1a32("Lifecycle.changed")
    }
}

// MARK: - Http records

/// `HttpMethod`. The specification names the type but not its variants; the indices below are
/// the order the runtimes agree on (see the package README, "Standard port records").
enum PortHttpMethod: UInt16, KeelCodec, Sendable, Hashable, CaseIterable {
    case get = 0
    case post = 1
    case put = 2
    case delete = 3
    case patch = 4
    case head = 5
    case options = 6

    /// The method's name in an HTTP request line.
    var name: String {
        switch self {
        case .get: return "GET"
        case .post: return "POST"
        case .put: return "PUT"
        case .delete: return "DELETE"
        case .patch: return "PATCH"
        case .head: return "HEAD"
        case .options: return "OPTIONS"
        }
    }

    static func keelDecode(_ r: inout KeelReader) throws -> PortHttpMethod {
        let at = r.position
        let tag = try r.readU16()
        guard let value = PortHttpMethod(rawValue: tag) else {
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "HttpMethod")
        }
        return value
    }

    func keelEncode(_ w: inout KeelWriter) {
        w.writeU16(rawValue)
    }
}

/// `Header { name: String, value: String }`.
struct PortHeader: KeelCodec, Sendable, Hashable {
    var name: String
    var value: String

    static func keelDecode(_ r: inout KeelReader) throws -> PortHeader {
        let name = try r.readString()
        let value = try r.readString()
        return PortHeader(name: name, value: value)
    }

    func keelEncode(_ w: inout KeelWriter) {
        w.writeString(name)
        w.writeString(value)
    }
}

/// `HttpRequest { method, url, headers, body: Option<Bytes>, timeout_ms: Option<u32> }`.
struct PortHttpRequest: KeelCodec, Sendable, Hashable {
    var method: PortHttpMethod
    var url: String
    var headers: [PortHeader]
    var body: [UInt8]?
    var timeoutMs: UInt32?

    static func keelDecode(_ r: inout KeelReader) throws -> PortHttpRequest {
        let method = try PortHttpMethod.keelDecode(&r)
        let url = try r.readString()
        let headers = try [PortHeader].keelDecode(&r)
        let body = try Optional<KeelBytes>.keelDecode(&r)?.bytes
        let timeoutMs = try Optional<UInt32>.keelDecode(&r)
        return PortHttpRequest(method: method, url: url, headers: headers, body: body, timeoutMs: timeoutMs)
    }

    func keelEncode(_ w: inout KeelWriter) {
        method.keelEncode(&w)
        w.writeString(url)
        headers.keelEncode(&w)
        body.map(KeelBytes.init).keelEncode(&w)
        timeoutMs.keelEncode(&w)
    }
}

/// `HttpResponse { status: u16, headers: Vec<Header>, body: Bytes }`.
struct PortHttpResponse: KeelCodec, Sendable, Hashable {
    var status: UInt16
    var headers: [PortHeader]
    var body: [UInt8]

    static func keelDecode(_ r: inout KeelReader) throws -> PortHttpResponse {
        let status = try r.readU16()
        let headers = try [PortHeader].keelDecode(&r)
        let body = try r.readBytes()
        return PortHttpResponse(status: status, headers: headers, body: body)
    }

    func keelEncode(_ w: inout KeelWriter) {
        w.writeU16(status)
        headers.keelEncode(&w)
        w.writeBytes(body)
    }
}

/// `HttpError { Network(String), Timeout, Cancelled, InvalidUrl(String) }`.
enum PortHttpError: KeelCodec, Sendable, Hashable, Error {
    case network(String)
    case timeout
    case cancelled
    case invalidUrl(String)

    static func keelDecode(_ r: inout KeelReader) throws -> PortHttpError {
        let at = r.position
        let tag = try r.readU16()
        switch tag {
        case 0:
            let reason = try r.readString()
            return .network(reason)
        case 1:
            return .timeout
        case 2:
            return .cancelled
        case 3:
            let url = try r.readString()
            return .invalidUrl(url)
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "HttpError")
        }
    }

    func keelEncode(_ w: inout KeelWriter) {
        switch self {
        case .network(let reason):
            w.writeU16(0)
            w.writeString(reason)
        case .timeout:
            w.writeU16(1)
        case .cancelled:
            w.writeU16(2)
        case .invalidUrl(let url):
            w.writeU16(3)
            w.writeString(url)
        }
    }
}

// MARK: - Fs records

/// `FsError { NotFound, Denied, Io(String) }`.
enum PortFsError: KeelCodec, Sendable, Hashable, Error {
    case notFound
    case denied
    case io(String)

    static func keelDecode(_ r: inout KeelReader) throws -> PortFsError {
        let at = r.position
        let tag = try r.readU16()
        switch tag {
        case 0:
            return .notFound
        case 1:
            return .denied
        case 2:
            let reason = try r.readString()
            return .io(reason)
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "FsError")
        }
    }

    func keelEncode(_ w: inout KeelWriter) {
        switch self {
        case .notFound:
            w.writeU16(0)
        case .denied:
            w.writeU16(1)
        case .io(let reason):
            w.writeU16(2)
            w.writeString(reason)
        }
    }
}

// MARK: - Event records

/// `NetKind { Wifi, Cellular, Wired, Unknown, None }`; `None` is spelled `disconnected` here so
/// that `PortNetKind?` has no ambiguous `.none`.
enum PortNetKind: UInt16, KeelCodec, Sendable, Hashable, CaseIterable {
    case wifi = 0
    case cellular = 1
    case wired = 2
    case unknown = 3
    case disconnected = 4

    static func keelDecode(_ r: inout KeelReader) throws -> PortNetKind {
        let at = r.position
        let tag = try r.readU16()
        guard let value = PortNetKind(rawValue: tag) else {
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "NetKind")
        }
        return value
    }

    func keelEncode(_ w: inout KeelWriter) {
        w.writeU16(rawValue)
    }
}

/// The lifecycle state an app reports to the core (`AppState { Active, Inactive, Background }`).
///
/// The name carries the `Keel` prefix because it is public and generated modules declare their
/// own `AppState`.
public enum KeelAppState: UInt16, KeelCodec, Sendable, Hashable, CaseIterable {
    /// The app is in the foreground and receiving events.
    case active = 0
    /// The app is in the foreground but not receiving events (an interruption is in progress).
    case inactive = 1
    /// The app is in the background.
    case background = 2

    public static func keelDecode(_ r: inout KeelReader) throws -> KeelAppState {
        let at = r.position
        let tag = try r.readU16()
        guard let value = KeelAppState(rawValue: tag) else {
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "AppState")
        }
        return value
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeU16(rawValue)
    }
}

// MARK: - RuntimeConfig

/// The `RuntimeConfig` record passed to `keel_init` (docs/SPEC.md section 6):
/// `platform String, mode String, core_threads u8, blocking_threads u8, log_level u8`.
struct RuntimeConfigRecord: KeelCodec, Sendable, Equatable {
    var platform: String
    var mode: String
    var coreThreads: UInt8
    var blockingThreads: UInt8
    var logLevel: UInt8

    static func keelDecode(_ r: inout KeelReader) throws -> RuntimeConfigRecord {
        let platform = try r.readString()
        let mode = try r.readString()
        let coreThreads = try r.readU8()
        let blockingThreads = try r.readU8()
        let logLevel = try r.readU8()
        return RuntimeConfigRecord(
            platform: platform,
            mode: mode,
            coreThreads: coreThreads,
            blockingThreads: blockingThreads,
            logLevel: logLevel
        )
    }

    func keelEncode(_ w: inout KeelWriter) {
        w.writeString(platform)
        w.writeString(mode)
        w.writeU8(coreThreads)
        w.writeU8(blockingThreads)
        w.writeU8(logLevel)
    }
}
