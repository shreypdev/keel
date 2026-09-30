// The runtime's typed errors (docs/SPEC.md sections 11 and 17.3).
//
// * `KeelReplyError`            a core call finished with a status other than ok
// * `KeelPortError`             thrown by a port implementation to answer with a typed error
// * `KeelModeError`             an operation the current transport mode cannot perform
// * `KeelSchemaMismatchError`   the core and the bindings were built from different schemas
// * `KeelLoadError`             `KeelCore.load` could not attach to a core
// * `KeelTransportError`        the connection to the core failed or timed out after loading
// * `KeelProtocolError`         the core sent something the protocol does not allow
// * `KeelRestoreError`          the core refused a snapshot

// MARK: - KeelReplyError

/// A call that did not finish with status `ok`.
///
/// Generated code turns the ones it expects into typed errors: a reply with `status == .error`
/// carries the encoded `E` of a `Result<T, E>` in `body`. Everything else (`panic`, `cancelled`,
/// `badRequest`, a stream that opened where a plain reply was expected) is not part of a method's
/// signature and reaches the caller as this error.
public struct KeelReplyError: Error, Sendable, Equatable {
    /// The reply status.
    public let status: ReplyStatus
    /// The status-dependent body, undecoded: the encoded `E` for `.error`, `String message,
    /// String backtrace` for `.panic`, a `String` reason for `.badRequest`.
    public let body: [UInt8]

    /// Creates an error from a reply's status and body.
    public init(status: ReplyStatus, body: [UInt8]) {
        self.status = status
        self.body = body
    }

    /// The panic message of a `.panic` reply or the reason of a `.badRequest` reply, when the
    /// body decodes as the documented layout; `nil` for every other status.
    public var message: String? {
        var reader = KeelReader(body)
        switch status {
        case .panic:
            return try? reader.readString()
        case .badRequest:
            return try? reader.readString()
        case .ok, .error, .cancelled, .streamOpened:
            return nil
        }
    }
}

extension KeelReplyError: CustomStringConvertible {
    public var description: String {
        switch status {
        case .ok:
            return "Keel call replied ok (unexpected here)"
        case .error:
            return "Keel call failed with a typed error (\(body.count) byte(s) of encoded error)"
        case .panic:
            return "Keel core panicked: \(message ?? "<undecodable panic report>")"
        case .cancelled:
            return "Keel call was cancelled"
        case .streamOpened:
            return "Keel call opened a stream where a single reply was expected"
        case .badRequest:
            return "Keel core rejected the request: \(message ?? "<undecodable reason>")"
        }
    }
}

// MARK: - KeelPortError

/// Thrown by a port implementation (typically by generated `PortImpl` adapters) to answer a port
/// call with a typed error: the runtime replies to the core with port status 1 (`error`) and
/// `body` as the reply body. `body` is the encoded `E` of the port method's `Result<T, E>`.
public struct KeelPortError: Error, Sendable, Equatable {
    /// The encoded error value.
    public let body: [UInt8]

    /// Creates a port error carrying an already-encoded error value.
    public init(body: [UInt8]) {
        self.body = body
    }
}

// MARK: - KeelMode and KeelModeError

/// How a `KeelCore` reaches the Rust core.
public enum KeelMode: String, Sendable, Equatable {
    /// The core is linked into the app and called through the C ABI.
    case inproc
    /// The core runs elsewhere (`keel dev`) and is reached over a WebSocket.
    case remote
}

/// An operation that the core's transport mode cannot perform.
public struct KeelModeError: Error, Sendable, Equatable {
    /// The operation, for example `"snapshot"`.
    public let operation: String
    /// The mode that cannot perform it.
    public let mode: KeelMode

    /// Creates a mode error.
    public init(operation: String, mode: KeelMode) {
        self.operation = operation
        self.mode = mode
    }
}

extension KeelModeError: CustomStringConvertible {
    public var description: String {
        return "Keel operation `\(operation)` is not available in \(mode.rawValue) mode"
    }
}

// MARK: - KeelSchemaMismatchError

/// The core reports a schema hash other than the one the bindings were generated for
/// (constitution rule R7). Rebuild the core and regenerate the bindings, or load the matching
/// core.
public struct KeelSchemaMismatchError: Error, Sendable, Equatable {
    /// The hash the bindings were generated for (`KeelIds.schemaHash`).
    public let expected: UInt64
    /// The hash the core reports.
    public let got: UInt64

    /// Creates a mismatch error.
    public init(expected: UInt64, got: UInt64) {
        self.expected = expected
        self.got = got
    }
}

extension KeelSchemaMismatchError: CustomStringConvertible {
    public var description: String {
        return "Keel schema mismatch: the bindings were generated for schema 0x\(hex16(expected)) "
            + "but the core reports 0x\(hex16(got)). Rebuild the core and regenerate the bindings "
            + "with `keel bindgen`."
    }
}

/// Sixteen lowercase hex digits.
func hex16(_ value: UInt64) -> String {
    let digits = String(value, radix: 16)
    if digits.count >= 16 {
        return digits
    }
    return String(repeating: "0", count: 16 - digits.count) + digits
}

// MARK: - KeelLoadError

/// Why `KeelCore.load` could not attach to a core.
public enum KeelLoadError: Error, Sendable, Equatable {
    /// An in-process core is already loaded in this process; call `KeelCore.shutdown()` on it
    /// first.
    case alreadyLoaded
    /// The linked native library speaks another C ABI version. Version 0 is the stub that is
    /// compiled when the real core is not linked (see the package README, `KEEL_LINK_CORE`).
    case abiMismatch(expected: UInt32, got: UInt32)
    /// `keel_init` returned a non-zero code.
    case coreInitFailed(code: UInt32)
    /// The remote URL is not a valid `ws://` or `wss://` URL.
    case invalidURL(String)
    /// The WebSocket could not be opened or broke during the handshake.
    case connectionFailed(String)
    /// The remote core did not answer the `Hello` within `seconds`.
    case handshakeTimedOut(seconds: Double)
    /// The remote core answered the handshake with something other than a well-formed `Hello`.
    case handshakeRejected(String)
}

extension KeelLoadError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .alreadyLoaded:
            return "a Keel core is already loaded in this process; call shutdown() on it before loading another"
        case .abiMismatch(let expected, let got):
            return "the linked Keel library speaks C ABI version \(got), this runtime needs \(expected)"
                + (got == 0 ? " (version 0 is the link-time stub: link the real core, see KEEL_LINK_CORE)" : "")
        case .coreInitFailed(let code):
            return "keel_init failed with code \(code)"
        case .invalidURL(let url):
            return "not a valid ws:// or wss:// URL: \(url)"
        case .connectionFailed(let reason):
            return "could not connect to the Keel dev core: \(reason)"
        case .handshakeTimedOut(let seconds):
            return "the Keel dev core did not answer the Hello within \(seconds) s"
        case .handshakeRejected(let reason):
            return "the Keel dev core's Hello was rejected: \(reason)"
        }
    }
}

// MARK: - KeelTransportError

/// The connection to the core failed after loading.
public enum KeelTransportError: Error, Sendable, Equatable {
    /// The core was shut down, or the connection to it closed, before the operation finished.
    case closed
    /// A blocking operation (a `callSync` over the remote transport) did not finish in time.
    case timedOut(operation: String)
    /// The connection failed; `reason` is the underlying error's description.
    case connectionLost(reason: String)
}

extension KeelTransportError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .closed:
            return "the Keel core is shut down or disconnected"
        case .timedOut(let operation):
            return "Keel operation `\(operation)` timed out"
        case .connectionLost(let reason):
            return "the connection to the Keel core was lost: \(reason)"
        }
    }
}

// MARK: - KeelRestoreError

/// `KeelCore.restore(_:)` was refused: the core rejected the snapshot and is unchanged.
public struct KeelRestoreError: Error, Sendable, Equatable {
    /// The non-zero code `keel_restore` returned.
    public let code: UInt32

    /// Creates a restore error.
    public init(code: UInt32) {
        self.code = code
    }
}

extension KeelRestoreError: CustomStringConvertible {
    public var description: String {
        return "the Keel core rejected the snapshot (code \(code)); a rejected restore leaves the core unchanged"
    }
}

// MARK: - KeelProtocolError

/// The core sent something the protocol does not allow.
public enum KeelProtocolError: Error, Sendable, Equatable {
    /// A message could not be decoded.
    case malformedMessage(context: String, error: WireError)
    /// `stream` was used on a method that replied with a plain result.
    case notAStream(callId: UInt32)
    /// A constructor replied with the null handle.
    case nullHandle
}

extension KeelProtocolError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .malformedMessage(let context, let error):
            return "malformed \(context) from the Keel core: \(error)"
        case .notAStream(let callId):
            return "call \(callId) was opened as a stream but the core replied with a plain result"
        case .nullHandle:
            return "the Keel core returned the null handle from a constructor"
        }
    }
}
