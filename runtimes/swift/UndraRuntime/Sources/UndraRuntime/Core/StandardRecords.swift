// The records the standard ports exchange (docs/SPEC.md section 8), with their hand-written wire
// codecs: `HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `HttpError`, `FsError`,
// `StorageError`, `NetKind`, `UndraAppState` and, since ADR-046, `UndraPanicFrame`,
// `UndraPanicReport` and `UndraBackgroundReport`.
//
// They are public API. Generated bindings refer to them where an app's own port, method or record
// mentions a standard type (`func upload(_ request: HttpRequest) async throws(HttpError) ->
// HttpResponse`) and declare none of them (ADR-024 and its amendment), and an app that
// implements `Http`, `Fs`, `Kv`, `SecureStore` or `Connectivity` itself builds and reads them. The names are the Rust
// ones, as in the Kotlin and TypeScript runtimes, and the shapes are what generated code for
// `undra-ports` would contain (a struct per record, `Codable` where it can be, an enum per error
// with the messages of the Rust `#[error]` attributes). Some spellings differ from the Rust:
// `NetKind.disconnected` is `NetKind::None` (a case called `none` would be ambiguous with
// `Optional.none` wherever the value is optional), and `AppState` is `UndraAppState`, the name it
// has had since v1 (an app's own `AppState` is the commonest type name there is). `PanicFrame`,
// `PanicReport` and `BackgroundReport` carry the `Undra` prefix too (`UndraPanicFrame`,
// `UndraPanicReport`, `UndraBackgroundReport`): they are what an app hands to its crash reporter and
// to the OS, where a bare `PanicReport` would collide with the app's own types.
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
/// `FsError { NotFound, Denied, Io(String), Full, Unavailable(String) }`. `Full` and `Unavailable`
/// were added by ADR-049 after the others, so their wire indices (3 and 4) follow `Io`.
public enum FsError: UndraError, Error, Sendable, Hashable {
    /// The file or directory does not exist.
    case notFound
    /// Access is not allowed: a permission error, or a path that leaves the file root.
    case denied
    /// Any other I/O failure; the text is the platform's.
    case io(String)
    /// The disk or the storage quota is exhausted (ADR-049).
    case full
    /// No file system in this context, or no adapter registered; the text says which (ADR-049).
    case unavailable(String)

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
        case 3:
            return .full
        case 4:
            let reason = try r.readString()
            return .unavailable(reason)
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
        case .full:
            w.writeU16(3)
        case .unavailable(let reason):
            w.writeU16(4)
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
        case .full:
            return "the disk is full"
        case .unavailable(let reason):
            return "the file system is unavailable: \(reason)"
        }
    }
}

// MARK: - Storage records

/// Why a `Kv` or `SecureStore` operation failed (ADR-049).
///
/// `StorageError { Unavailable(String), Full, Locked, Corrupt(String), Io(String) }`.
///
/// Every method of the two storage ports reports a failure as one of these instead of making the
/// call "unavailable", and the default adapters (``KvAdapter``, ``SecureStoreAdapter``) map their
/// platform failures onto it: an out-of-space write is ``full``, a Keychain or a data-protected
/// file that cannot be read before the device's first unlock is ``locked``, a stored entry that
/// cannot be read back is ``corrupt(_:)``, anything else is ``io(_:)`` with the platform's message.
/// An app that implements `Kv` or `SecureStore` itself answers a failure by throwing
/// `UndraPortError(body: error.undraEncoded())` (generated port adapters do that for a method
/// that throws `StorageError`).
public enum StorageError: UndraError, Error, Sendable, Hashable {
    /// No adapter is registered, or the platform has no backend in this context; the text says
    /// which.
    case unavailable(String)
    /// The quota or the disk is exhausted.
    case full
    /// Protected data cannot be read now (before the device's first unlock, or a key that needs
    /// the user to authenticate).
    case locked
    /// The stored bytes (or ciphertext) cannot be read back; the key is still there.
    case corrupt(String)
    /// Any other failure; the text is the platform's.
    case io(String)

    public static func undraDecode(_ r: inout UndraReader) throws -> StorageError {
        let at = r.position
        let tag = try r.readU16()
        switch tag {
        case 0:
            let reason = try r.readString()
            return .unavailable(reason)
        case 1:
            return .full
        case 2:
            return .locked
        case 3:
            let reason = try r.readString()
            return .corrupt(reason)
        case 4:
            let reason = try r.readString()
            return .io(reason)
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "StorageError")
        }
    }

    public func undraEncode(_ w: inout UndraWriter) {
        switch self {
        case .unavailable(let reason):
            w.writeU16(0)
            w.writeString(reason)
        case .full:
            w.writeU16(1)
        case .locked:
            w.writeU16(2)
        case .corrupt(let reason):
            w.writeU16(3)
            w.writeString(reason)
        case .io(let reason):
            w.writeU16(4)
            w.writeString(reason)
        }
    }
}

extension StorageError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .unavailable(let reason):
            return "storage is unavailable: \(reason)"
        case .full:
            return "the storage is full"
        case .locked:
            return "the storage is locked"
        case .corrupt(let reason):
            return "stored data is corrupt: \(reason)"
        case .io(let reason):
            return "storage I/O error: \(reason)"
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

// MARK: - Diagnostics and background records (ADR-046)

/// One frame of the Rust call stack at a panic, innermost first.
///
/// `PanicFrame { address: u64, symbol: Option<String>, file: Option<String>, line: Option<u32> }`
/// (type id `0x19a497d1`). `address` is an offset into the core's image (the instruction address
/// minus the image's load address, pointing into the call instruction), so a symbolicator holding
/// the build's symbol files (``UndraPanicReport/imageId`` names them) resolves it. A debug build
/// also names the frame (`symbol`, `file`, `line`); a release build has addresses only, and `0`
/// means the address is not known.
public struct UndraPanicFrame: UndraRecord, Sendable, Hashable, Codable {
    /// The offset of the instruction in the core's image, or `0` when it is not known.
    public var address: UInt64
    /// The function name, when the image still has it (debug builds).
    public var symbol: String?
    /// The source file, when the image still has it (debug builds).
    public var file: String?
    /// The source line, when the image still has it (debug builds).
    public var line: UInt32?

    /// Creates a frame; without arguments for the rest it carries an address only.
    public init(address: UInt64, symbol: String? = nil, file: String? = nil, line: UInt32? = nil) {
        self.address = address
        self.symbol = symbol
        self.file = file
        self.line = line
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraPanicFrame {
        let address = try r.readU64()
        let symbol = try Optional<String>.undraDecode(&r)
        let file = try Optional<String>.undraDecode(&r)
        let line = try Optional<UInt32>.undraDecode(&r)
        return UndraPanicFrame(address: address, symbol: symbol, file: file, line: line)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU64(address)
        symbol.undraEncode(&w)
        file.undraEncode(&w)
        line.undraEncode(&w)
    }
}

/// What the core knows about a panic it contained: the value ``LoadOptions/onPanic`` receives, once
/// per panic, and what an app hands to its crash reporter (ADR-046).
///
/// `PanicReport { message, location, operation, thread, frames: Vec<PanicFrame>, namespace,
/// core_version, schema_hash: u64, image_id }` (type id `0xd08d5436`). The call that panicked still
/// fails as before (``UndraCallError/panicked(message:backtrace:)``); this is the structured twin of
/// that error and the only report of a panic in a task nobody awaits.
public struct UndraPanicReport: UndraRecord, Sendable, Hashable, Codable {
    /// The panic message.
    public var message: String
    /// Where it panicked, `file:line:column` (the path is remapped in release builds).
    public var location: String
    /// What the core was running: `"Todos.add"`, `"explode"`, `"task"`, `"computed Todos.visible"`,
    /// `"observe Todos"`, `"snapshot Todos"`, `"init hook x"`, and so on.
    public var operation: String
    /// The name of the Rust thread that panicked (`"undra-core"`, ...).
    public var thread: String
    /// The Rust stack, innermost frame first; empty when the core could not capture it.
    public var frames: [UndraPanicFrame]
    /// The core's namespace (`[core] namespace` in its undra.toml).
    public var namespace: String
    /// The version of the core crate (`export_core!`).
    public var coreVersion: String
    /// The schema hash of the core.
    public var schemaHash: UInt64
    /// The identity of the image that holds the core, lowercase hex: the Mach-O `LC_UUID` or the ELF
    /// GNU build id. It is what a symbolicator uses to find the symbol file of this exact build; empty
    /// when the core could not read it.
    public var imageId: String

    /// Creates a report.
    public init(
        message: String,
        location: String,
        operation: String,
        thread: String,
        frames: [UndraPanicFrame] = [],
        namespace: String,
        coreVersion: String,
        schemaHash: UInt64,
        imageId: String
    ) {
        self.message = message
        self.location = location
        self.operation = operation
        self.thread = thread
        self.frames = frames
        self.namespace = namespace
        self.coreVersion = coreVersion
        self.schemaHash = schemaHash
        self.imageId = imageId
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraPanicReport {
        let message = try r.readString()
        let location = try r.readString()
        let operation = try r.readString()
        let thread = try r.readString()
        let frames = try [UndraPanicFrame].undraDecode(&r)
        let namespace = try r.readString()
        let coreVersion = try r.readString()
        let schemaHash = try r.readU64()
        let imageId = try r.readString()
        return UndraPanicReport(
            message: message,
            location: location,
            operation: operation,
            thread: thread,
            frames: frames,
            namespace: namespace,
            coreVersion: coreVersion,
            schemaHash: schemaHash,
            imageId: imageId
        )
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeString(message)
        w.writeString(location)
        w.writeString(operation)
        w.writeString(thread)
        frames.undraEncode(&w)
        w.writeString(namespace)
        w.writeString(coreVersion)
        w.writeU64(schemaHash)
        w.writeString(imageId)
    }
}

/// What a background run (``UndraCore/runInBackground(deadline:)``) did (ADR-046).
///
/// `BackgroundReport { finished: bool, replayed: u32, refetched: u32, still_pending: u32 }` (type id
/// `0x5dbea5f3`).
public struct UndraBackgroundReport: UndraRecord, Sendable, Hashable, Codable {
    /// Whether every background task finished and nothing is left to do. `false` when the deadline
    /// came first or something is still pending: ask the OS for another window.
    public var finished: Bool
    /// Offline mutations the run replayed to the server.
    public var replayed: UInt32
    /// Stale persisted or observed queries the run refetched.
    public var refetched: UInt32
    /// Items still waiting for a window or for the network after the run: queued offline mutations,
    /// stale persisted queries, unflushed persistence.
    public var stillPending: UInt32

    /// Creates a report.
    public init(finished: Bool, replayed: UInt32 = 0, refetched: UInt32 = 0, stillPending: UInt32 = 0) {
        self.finished = finished
        self.replayed = replayed
        self.refetched = refetched
        self.stillPending = stillPending
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraBackgroundReport {
        let finished = try r.readBool()
        let replayed = try r.readU32()
        let refetched = try r.readU32()
        let stillPending = try r.readU32()
        return UndraBackgroundReport(finished: finished, replayed: replayed, refetched: refetched, stillPending: stillPending)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeBool(finished)
        w.writeU32(replayed)
        w.writeU32(refetched)
        w.writeU32(stillPending)
    }
}
