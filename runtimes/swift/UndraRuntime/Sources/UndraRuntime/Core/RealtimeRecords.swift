// The records of the opt-in `WebSocket` and `Sse` ports (ADR-047), with their hand-written wire
// codecs: `WsOpened`, `WsMessage`, `WsError`, `SseEvent` and `SseError`.
//
// They are public API, like the records of StandardRecords.swift: generated bindings of a core
// built with the `websocket` / `sse` features refer to them (`func read(count:) async throws ->
// [WsMessage]`, `UndraCallError.mapped(error, domain: WsError.self)`) and declare none of them, and
// an app that implements `WebSocketAdapter` or `SseAdapter` itself builds and reads them. The
// shapes, names and messages are what `undra-bindgen` generates for `undra-ports` (the errors'
// descriptions are the Rust `#[error]` texts); every runtime exports all of them whatever a core
// enables.
//
// This file needs no Foundation: the errors' `LocalizedError` conformances are in
// Wire/Foundation+Undra.swift.

// MARK: - WebSocket

/// What `WebSocket.connect` answers: the connection's id and the subprotocol the server chose.
///
/// `WsOpened { conn: u32, protocol: String }`.
public struct WsOpened: UndraRecord, Sendable, Hashable, Codable {
    /// The connection's id, chosen by the binding; never reused by it.
    public var conn: UInt32
    /// The subprotocol the server selected, or `""` when none was negotiated.
    public var `protocol`: String

    /// Creates the answer of a connect.
    public init(conn: UInt32, `protocol`: String) {
        self.conn = conn
        self.`protocol` = `protocol`
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> WsOpened {
        let conn = try r.readU32()
        let negotiated = try r.readString()
        return WsOpened(conn: conn, protocol: negotiated)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU32(conn)
        w.writeString(`protocol`)
    }
}

/// One WebSocket message. Control frames (ping, pong, close) never surface as messages.
///
/// `WsMessage { Text(String) = 0, Binary(Bytes) = 1 }`. `Codable` (synthesized, `{"text":{"_0":..}}`)
/// because generated records that hold a message derive `Codable`.
public enum WsMessage: UndraEnum, Sendable, Hashable, Codable {
    /// A text message (valid UTF-8 by RFC 6455).
    case text(String)
    /// A binary message.
    case binary([UInt8])

    /// The message's payload as bytes (the UTF-8 of a text message).
    public var bytes: [UInt8] {
        switch self {
        case .text(let text):
            return Array(text.utf8)
        case .binary(let bytes):
            return bytes
        }
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> WsMessage {
        let at = r.position
        let tag = try r.readU16()
        switch tag {
        case 0:
            return .text(try r.readString())
        case 1:
            return .binary(try r.readBytes())
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "WsMessage")
        }
    }

    public func undraEncode(_ w: inout UndraWriter) {
        switch self {
        case .text(let text):
            w.writeU16(0)
            w.writeString(text)
        case .binary(let bytes):
            w.writeU16(1)
            w.writeBytes(bytes)
        }
    }
}

/// Why a WebSocket could not be opened, or how it ended (ADR-047 §5).
///
/// | What happened | Case |
/// |---|---|
/// | the upgrade was answered non-101, the URL is unusable, headers the platform cannot send | `refused` |
/// | the connection dropped without a close frame (DNS, reset, TLS, timeout) | `network` |
/// | the peer broke RFC 6455 (a text frame that is not UTF-8), or a reply did not decode | `protocol` |
/// | the peer sent a close frame (1000 included), or the adapter closed past its backlog limit (1008) | `closed` |
///
/// `WsError { Refused { status: Option<u16>, message: String } = 0, Network(String) = 1,
/// Protocol(String) = 2, Closed { code: u16, reason: String } = 3 }`.
public enum WsError: UndraError, Error, Sendable, Hashable {
    /// The connection was not established. `status` is the HTTP status of the refused upgrade
    /// where the platform reports it (URLSession does).
    case refused(status: UInt16?, message: String)
    /// The connection failed or dropped without a closing handshake; the text is the platform's.
    case network(String)
    /// The peer broke the protocol (or a port reply did not decode); the text says how.
    case `protocol`(String)
    /// The connection was closed with a close frame: `code` and `reason` are the frame's.
    case closed(code: UInt16, reason: String)

    public static func undraDecode(_ r: inout UndraReader) throws -> WsError {
        let at = r.position
        let tag = try r.readU16()
        switch tag {
        case 0:
            let status = try Optional<UInt16>.undraDecode(&r)
            let message = try r.readString()
            return .refused(status: status, message: message)
        case 1:
            return .network(try r.readString())
        case 2:
            return .protocol(try r.readString())
        case 3:
            let code = try r.readU16()
            let reason = try r.readString()
            return .closed(code: code, reason: reason)
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "WsError")
        }
    }

    public func undraEncode(_ w: inout UndraWriter) {
        switch self {
        case .refused(let status, let message):
            w.writeU16(0)
            status.undraEncode(&w)
            w.writeString(message)
        case .network(let message):
            w.writeU16(1)
            w.writeString(message)
        case .protocol(let message):
            w.writeU16(2)
            w.writeString(message)
        case .closed(let code, let reason):
            w.writeU16(3)
            w.writeU16(code)
            w.writeString(reason)
        }
    }
}

extension WsError: CustomStringConvertible {
    /// The message of the Rust `#[error]` attribute.
    public var description: String {
        switch self {
        case .refused(_, let message):
            return "the WebSocket was refused: \(message)"
        case .network(let message):
            return "WebSocket network error: \(message)"
        case .protocol(let message):
            return "WebSocket protocol error: \(message)"
        case .closed(let code, let reason):
            return "the WebSocket was closed (\(code)): \(reason)"
        }
    }
}

// MARK: - Server-sent events

/// One server-sent event.
///
/// `SseEvent { id: Option<String>, event: String, data: String, retry_ms: Option<u32> }`.
public struct SseEvent: UndraRecord, Sendable, Hashable, Codable {
    /// The event's `id` field, if the event (or an earlier one, per the standard's last-event-id
    /// buffer) set one. Send it back as `lastEventId` to resume.
    public var id: String?
    /// The event type: the `event` field, `"message"` when absent.
    public var event: String
    /// The `data` lines, joined with `\n`.
    public var data: String
    /// The `retry` field in milliseconds, when this event carried one: how long the server asks
    /// clients to wait before reconnecting.
    public var retryMs: UInt32?

    /// Creates an event; without arguments for the rest it is a `"message"` with no id and no
    /// retry.
    public init(id: String? = nil, event: String = "message", data: String, retryMs: UInt32? = nil) {
        self.id = id
        self.event = event
        self.data = data
        self.retryMs = retryMs
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> SseEvent {
        let id = try Optional<String>.undraDecode(&r)
        let event = try r.readString()
        let data = try r.readString()
        let retryMs = try Optional<UInt32>.undraDecode(&r)
        return SseEvent(id: id, event: event, data: data, retryMs: retryMs)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        id.undraEncode(&w)
        w.writeString(event)
        w.writeString(data)
        retryMs.undraEncode(&w)
    }
}

/// Why a server-sent event stream could not be opened, or how it ended (ADR-047 §5).
///
/// `SseError { Refused { status: Option<u16>, message: String } = 0, Network(String) = 1,
/// Protocol(String) = 2, Ended = 3 }`.
public enum SseError: UndraError, Error, Sendable, Hashable {
    /// The request was answered with a status other than 2xx (a 204 means "stop"), or the URL
    /// is unusable. `status` is absent when there was no HTTP answer.
    case refused(status: UInt16?, message: String)
    /// The connection failed or dropped; the text is the platform's.
    case network(String)
    /// The answer was not `text/event-stream`, was not UTF-8, or a port reply did not decode.
    case `protocol`(String)
    /// The server ended the response. Reconnect with the last event id to resume.
    case ended

    public static func undraDecode(_ r: inout UndraReader) throws -> SseError {
        let at = r.position
        let tag = try r.readU16()
        switch tag {
        case 0:
            let status = try Optional<UInt16>.undraDecode(&r)
            let message = try r.readString()
            return .refused(status: status, message: message)
        case 1:
            return .network(try r.readString())
        case 2:
            return .protocol(try r.readString())
        case 3:
            return .ended
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "SseError")
        }
    }

    public func undraEncode(_ w: inout UndraWriter) {
        switch self {
        case .refused(let status, let message):
            w.writeU16(0)
            status.undraEncode(&w)
            w.writeString(message)
        case .network(let message):
            w.writeU16(1)
            w.writeString(message)
        case .protocol(let message):
            w.writeU16(2)
            w.writeString(message)
        case .ended:
            w.writeU16(3)
        }
    }
}

extension SseError: CustomStringConvertible {
    /// The message of the Rust `#[error]` attribute.
    public var description: String {
        switch self {
        case .refused(_, let message):
            return "the event stream was refused: \(message)"
        case .network(let message):
            return "event stream network error: \(message)"
        case .protocol(let message):
            return "event stream protocol error: \(message)"
        case .ended:
            return "the server ended the event stream"
        }
    }
}
