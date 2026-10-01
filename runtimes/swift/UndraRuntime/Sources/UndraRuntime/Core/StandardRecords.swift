// The records the standard ports exchange (docs/SPEC.md section 8), with their hand-written wire
// codecs: `HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `HttpError`, `FsError`,
// `NetKind` and `UndraAppState`.
//
// They are public API. Generated bindings refer to them where an app's own port, method or record
// mentions a standard type (`func upload(_ request: HttpRequest) async throws(HttpError) ->
// HttpResponse`) and declare none of them (ADR-024 and its amendment), and an app that
// implements `Http`, `Fs` or `Connectivity` itself builds and reads them. The names are the Rust
// ones, as in the Kotlin and TypeScript runtimes, and the shapes are what generated code for
// `undra-ports` would contain (a struct per record, `Codable` where it can be, an enum per error
// with the messages of the Rust `#[error]` attributes). Two spellings differ from the Rust:
// `NetKind.disconnected` is `NetKind::None` (a case called `none` would be ambiguous with
// `Optional.none` wherever the value is optional), and `AppState` is `UndraAppState`, the name it
// has had since v1 (an app's own `AppState` is the commonest type name there is).
//
// A module that declares a type with one of these names (an app record called `HttpRequest` with
// another shape) shadows the runtime's inside that module; code that imports both modules writes
// the module name where it means the app's own (`PlaygroundCore.HttpRequest`).
//
// This file needs no Foundation: the errors' `LocalizedError` conformances are in
// Wire/Foundation+Undra.swift.

// MARK: - Http records

/// The method of an ``HttpRequest``.
///
/// docs/SPEC.md section 8 names the type but not its variants; the indices below are the order
/// the runtimes agree on (see the package README, "Standard port records") and `undra-ports`
/// declares `HttpMethod` in the same order.
public enum HttpMethod: UInt16, UndraEnum, CaseIterable, Sendable, Codable {
    /// `GET`.
    case get = 0
    /// `POST`.
    case post = 1
    /// `PUT`.
    case put = 2
    /// `DELETE`.
    case delete = 3
    /// `PATCH`.
    case patch = 4
    /// `HEAD`.
    case head = 5
    /// `OPTIONS`.
    case options = 6

    /// The method as written in an HTTP request line: `"GET"`, `"POST"`, ...
    public var name: String {
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

    public static func undraDecode(_ r: inout UndraReader) throws -> HttpMethod {
        let at = r.position
        let tag = try r.readU16()
        guard let value = HttpMethod(rawValue: tag) else {
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "HttpMethod")
        }
        return value
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU16(rawValue)
    }
}

/// One HTTP header. Repeated header names are repeated entries.
///
/// `Header { name: String, value: String }`.
public struct Header: UndraRecord, Sendable, Hashable, Codable {
    /// The header name, as the sender spelled it.
    public var name: String
    /// The header value.
    public var value: String

    /// Creates a header.
    public init(name: String, value: String) {
        self.name = name
        self.value = value
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> Header {
        let name = try r.readString()
        let value = try r.readString()
        return Header(name: name, value: value)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeString(name)
        w.writeString(value)
    }
}

/// An HTTP request the core asks the platform to perform.
///
/// `HttpRequest { method, url, headers, body: Option<Bytes>, timeout_ms: Option<u32> }`.
public struct HttpRequest: UndraRecord, Sendable, Hashable, Codable {
    /// The HTTP method.
    public var method: HttpMethod
    /// The absolute URL.
    public var url: String
    /// Request headers, in order.
    public var headers: [Header]
    /// The request body, if any.
    public var body: [UInt8]?
    /// The timeout of the whole request in milliseconds; absent leaves it to the platform.
    public var timeoutMs: UInt32?

    /// Creates a request; without arguments for the rest it has no headers, no body and no
    /// timeout.
    public init(
        method: HttpMethod,
        url: String,
        headers: [Header] = [],
        body: [UInt8]? = nil,
        timeoutMs: UInt32? = nil
    ) {
        self.method = method
        self.url = url
        self.headers = headers
        self.body = body
        self.timeoutMs = timeoutMs
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> HttpRequest {
        let method = try HttpMethod.undraDecode(&r)
        let url = try r.readString()
        let headers = try [Header].undraDecode(&r)
        let body = try Optional<UndraBytes>.undraDecode(&r)?.bytes
        let timeoutMs = try Optional<UInt32>.undraDecode(&r)
        return HttpRequest(method: method, url: url, headers: headers, body: body, timeoutMs: timeoutMs)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        method.undraEncode(&w)
        w.writeString(url)
        headers.undraEncode(&w)
        body.map(UndraBytes.init).undraEncode(&w)
        timeoutMs.undraEncode(&w)
    }
}

/// An HTTP response. Any status, including 4xx and 5xx, is a response; only failures before a
/// response exists are an ``HttpError``.
///
/// `HttpResponse { status: u16, headers: Vec<Header>, body: Bytes }`.
public struct HttpResponse: UndraRecord, Sendable, Hashable, Codable {
    /// The HTTP status code.
    public var status: UInt16
    /// Response headers, in the order the platform reports them.
    public var headers: [Header]
    /// The response body.
    public var body: [UInt8]

    /// Creates a response.
    public init(status: UInt16, headers: [Header] = [], body: [UInt8] = []) {
        self.status = status
        self.headers = headers
        self.body = body
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> HttpResponse {
        let status = try r.readU16()
        let headers = try [Header].undraDecode(&r)
        let body = try r.readBytes()
        return HttpResponse(status: status, headers: headers, body: body)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU16(status)
        headers.undraEncode(&w)
        w.writeBytes(body)
    }
}

/// Why an HTTP request failed before a response existed.
///
/// `HttpError { Network(String), Timeout, Cancelled, InvalidUrl(String) }`.
public enum HttpError: UndraError, Error, Sendable, Hashable {
    /// The connection failed (DNS, refused, reset, TLS, ...); the text is the platform's.
    case network(String)
    /// The request took longer than its timeout.
    case timeout
    /// The request was cancelled before it finished.
    case cancelled
    /// The URL (or a header) could not be used; the text names it.
    case invalidUrl(String)

    public static func undraDecode(_ r: inout UndraReader) throws -> HttpError {
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

    public func undraEncode(_ w: inout UndraWriter) {
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

extension HttpError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .network(let reason):
            return "network error: \(reason)"
        case .timeout:
            return "the request timed out"
        case .cancelled:
            return "the request was cancelled"
        case .invalidUrl(let url):
            return "invalid URL: \(url)"
        }
    }
}

// MARK: - Fs records

/// Why a file operation failed.
///
/// `FsError { NotFound, Denied, Io(String) }`.
public enum FsError: UndraError, Error, Sendable, Hashable {
    /// The file or directory does not exist.
    case notFound
    /// Access is not allowed: a permission error, or a path that leaves the file root.
    case denied
    /// Any other I/O failure; the text is the platform's.
    case io(String)

    public static func undraDecode(_ r: inout UndraReader) throws -> FsError {
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

    public func undraEncode(_ w: inout UndraWriter) {
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

extension FsError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .notFound:
            return "not found"
        case .denied:
            return "access denied"
        case .io(let reason):
            return "I/O error: \(reason)"
        }
    }
}

// MARK: - Event records

/// What kind of network the device is on.
///
/// `NetKind { Wifi, Cellular, Wired, Unknown, None }`; `None` is spelled `disconnected` here so
/// that `NetKind?` has no ambiguous `.none`.
public enum NetKind: UInt16, UndraEnum, CaseIterable, Sendable, Codable {
    /// Wi-Fi.
    case wifi = 0
    /// Mobile data.
    case cellular = 1
    /// Ethernet or another wired link.
    case wired = 2
    /// Connected, but the kind is not known.
    case unknown = 3
    /// No network (`None` in Rust).
    case disconnected = 4

    public static func undraDecode(_ r: inout UndraReader) throws -> NetKind {
        let at = r.position
        let tag = try r.readU16()
        guard let value = NetKind(rawValue: tag) else {
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "NetKind")
        }
        return value
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU16(rawValue)
    }
}

/// The lifecycle state an app reports to the core (`AppState { Active, Inactive, Background }`).
///
/// The name carries the `Undra` prefix because an app's own `AppState` is among the commonest
/// type names there are, and this one has been public under it since v1.
public enum UndraAppState: UInt16, UndraEnum, CaseIterable, Sendable, Codable {
    /// The app is in the foreground and receiving events.
    case active = 0
    /// The app is in the foreground but not receiving events (an interruption is in progress).
    case inactive = 1
    /// The app is in the background.
    case background = 2

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraAppState {
        let at = r.position
        let tag = try r.readU16()
        guard let value = UndraAppState(rawValue: tag) else {
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "AppState")
        }
        return value
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU16(rawValue)
    }
}
