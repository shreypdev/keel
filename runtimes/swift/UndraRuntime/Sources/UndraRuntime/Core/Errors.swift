// The runtime's typed errors (docs/SPEC.md sections 11 and 17.3).
//
// * `UndraReplyError`            a core call finished with a status other than ok (what the raw entry points throw)
// * `UndraCallError`             what a generated method throws when the call itself fails (CallError.swift)
// * `UndraUnhandledError`        a failure of a generated command or a store's change, given to `LoadOptions.onError`
// * `UndraPortError`             thrown by a port implementation to answer with a typed error
// * `UndraModeError`             an operation the current transport mode cannot perform
// * `UndraSchemaMismatchError`   the core and the bindings were built from different schemas
// * `UndraLoadError`             `UndraCore.load` could not attach to a core
// * `UndraTransportError`        the connection to the core failed or timed out after loading
// * `UndraProtocolError`         the core sent something the protocol does not allow
// * `UndraRestoreError`          the core refused a snapshot

// MARK: - UndraReplyError

/// A call that did not finish with status `ok`.
///
/// This is what the raw entry points (`UndraCore.callSync`, `call`, `stream`, `construct`) throw. A
/// reply with `status == .error` carries the encoded `E` of a `Result<T, E>` in `body`; the other
/// statuses (`panic`, `cancelled`, `badRequest`, a stream that opened where a plain reply was
/// expected) are not part of a method's signature. Generated methods do not expose it: they throw
/// their own `E`, `CancellationError`, or ``UndraCallError`` (ADR-032).
///
/// A stream ends with one too: its own typed error item as `.error` with the encoded `E`, and a
/// failed item (`Wire.StreamFailure`, ADR-036) as the reply it stands for, with that item's status
/// and the body a reply of that status carries.
public struct UndraReplyError: Error, Sendable, Equatable {
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
        var reader = UndraReader(body)
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

extension UndraReplyError: CustomStringConvertible {
    public var description: String {
        switch status {
        case .ok:
            return "Undra call replied ok (unexpected here)"
        case .error:
            return "Undra call failed with a typed error (\(body.count) byte(s) of encoded error)"
        case .panic:
            return "Undra core panicked: \(message ?? "<undecodable panic report>")"
        case .cancelled:
            return "Undra call was cancelled"
        case .streamOpened:
            return "Undra call opened a stream where a single reply was expected"
        case .badRequest:
            return "Undra core rejected the request: \(message ?? "<undecodable reason>")"
        }
    }
}

// MARK: - UndraPortError

/// Thrown by a port implementation (typically by generated `PortImpl` adapters) to answer a port
/// call with a typed error: the runtime replies to the core with port status 1 (`error`) and
/// `body` as the reply body. `body` is the encoded `E` of the port method's `Result<T, E>`.
public struct UndraPortError: Error, Sendable, Equatable {
    /// The encoded error value.
    public let body: [UInt8]

    /// Creates a port error carrying an already-encoded error value.
    public init(body: [UInt8]) {
        self.body = body
    }
}

// MARK: - UndraMode and UndraModeError

/// How an `UndraCore` reaches the Rust core.
public enum UndraMode: String, Sendable, Equatable {
    /// The core is linked into the app and called through the C ABI.
    case inproc
    /// The core runs elsewhere (`undra dev`) and is reached over a WebSocket.
    case remote
}

/// An operation that the core's transport mode cannot perform.
public struct UndraModeError: Error, Sendable, Equatable {
    /// The operation, for example `"snapshot"`.
    public let operation: String
    /// The mode that cannot perform it.
    public let mode: UndraMode

    /// Creates a mode error.
    public init(operation: String, mode: UndraMode) {
        self.operation = operation
        self.mode = mode
    }
}

extension UndraModeError: CustomStringConvertible {
    public var description: String {
        return "Undra operation `\(operation)` is not available in \(mode.rawValue) mode"
    }
}

// MARK: - UndraSchemaMismatchError

/// The core reports a schema hash other than the one the bindings were generated for
/// (constitution rule R7). Rebuild the core and regenerate the bindings, or load the matching
/// core.
public struct UndraSchemaMismatchError: Error, Sendable, Equatable {
    /// The hash the bindings were generated for (`UndraIds.schemaHash`).
    public let expected: UInt64
    /// The hash the core reports.
    public let got: UInt64

    /// Creates a mismatch error.
    public init(expected: UInt64, got: UInt64) {
        self.expected = expected
        self.got = got
    }
}

extension UndraSchemaMismatchError: CustomStringConvertible {
    public var description: String {
        return "Undra schema mismatch: the bindings were generated for schema 0x\(hex16(expected)) "
            + "but the core reports 0x\(hex16(got)). Rebuild the core and regenerate the bindings "
            + "with `undra bindgen`."
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

// MARK: - UndraLoadError

/// Why `UndraCore.load` could not attach to a core.
public enum UndraLoadError: Error, Sendable, Equatable {
    /// This core (an in-process core of the same namespace, ADR-044) is already loaded in this
    /// process, or its generated entry already holds an open core; call `shutdown()` on that core
    /// first. Cores of different namespaces load side by side.
    case alreadyLoaded
    /// The core's table (`<namespace>_undra_api()`) is of another C ABI version than this runtime
    /// (``UndraCore``'s, 2): the core and the runtime come from different Undra releases. Nothing
    /// after the version was read and the core was not started.
    case abiMismatch(expected: UInt32, got: UInt32)
    /// The core's table is not one this runtime can use: smaller than a version 2 `UndraApi`,
    /// without a namespace, or with a null entry. The core was not started.
    case invalidCoreTable(String)
    /// An in-process load was given no core table (``LoadOptions/api``): load the core through its
    /// generated entry, `Undra<Namespace>.load()`, which passes it.
    case missingCoreTable
    /// A load was given no ``LoadOptions/expectedSchemaHash``: load the core through its generated
    /// entry, `Undra<Namespace>.load()`, which passes the hash of its bindings.
    case missingSchemaHash
    /// The table's `init` returned a non-zero code.
    case coreInitFailed(code: UInt32)
    /// The core's namespace (``LoadOptions/namespace``, or the in-process table's) is not one: the default
    /// stores are kept in a directory of that name, so it is a lowercase letter followed by lowercase
    /// letters, digits and `_`, at most 32 bytes, as `undra.toml` has it (`..`, `a/b` and an empty
    /// string are refused). The payload says what is wrong. Nothing was started.
    case invalidNamespace(String)
    /// The remote URL is not a valid `ws://` or `wss://` URL.
    case invalidURL(String)
    /// The WebSocket could not be opened or broke during the handshake.
    case connectionFailed(String)
    /// The remote core did not answer the `Hello` within `seconds`.
    case handshakeTimedOut(seconds: Double)
    /// The remote core answered the handshake with something other than a well-formed `Hello`.
    case handshakeRejected(String)
}

extension UndraLoadError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .alreadyLoaded:
            return "this Undra core (its namespace) is already loaded in this process; call shutdown() on it before loading it again"
        case .abiMismatch(let expected, let got):
            return "the Undra core's table is of C ABI version \(got), this runtime needs \(expected): build the core and the app with the same Undra release"
        case .invalidCoreTable(let reason):
            return "not a usable Undra core table: \(reason)"
        case .missingCoreTable:
            return "LoadOptions.inproc has no core table (api): load the core through its generated entry, Undra<Namespace>.load()"
        case .missingSchemaHash:
            return "LoadOptions has no expectedSchemaHash: load the core through its generated entry, Undra<Namespace>.load()"
        case .coreInitFailed(let code):
            return "the Undra core's init failed with code \(code)"
        case .invalidNamespace(let reason):
            return "not an Undra core namespace: \(reason)"
        case .invalidURL(let url):
            return "not a valid ws:// or wss:// URL: \(url)"
        case .connectionFailed(let reason):
            return "could not connect to the Undra dev core: \(reason)"
        case .handshakeTimedOut(let seconds):
            return "the Undra dev core did not answer the Hello within \(seconds) s"
        case .handshakeRejected(let reason):
            return "the Undra dev core's Hello was rejected: \(reason)"
        }
    }
}

// MARK: - UndraTransportError

/// The connection to the core failed after loading.
public enum UndraTransportError: Error, Sendable, Equatable {
    /// The core was shut down, or the connection to it closed, before the operation finished.
    case closed
    /// A blocking operation (a `callSync` over the remote transport) did not finish in time.
    case timedOut(operation: String)
    /// The connection failed; `reason` is the underlying error's description.
    case connectionLost(reason: String)
}

extension UndraTransportError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .closed:
            return "the Undra core is shut down or disconnected"
        case .timedOut(let operation):
            return "Undra operation `\(operation)` timed out"
        case .connectionLost(let reason):
            return "the connection to the Undra core was lost: \(reason)"
        }
    }
}

// MARK: - UndraRestoreError

/// `UndraCore.restore(_:)` was refused: the core rejected the snapshot and is unchanged.
///
/// `code` is what `undra_restore` returned (`restore_code` in `crates/undra-ffi`); the four the
/// core uses have named values, so a refusal can be matched with `==`:
///
/// ```swift
/// do {
///     try core.restore(saved)
/// } catch let error as UndraRestoreError where error == .incompatible {
///     // The saved state was written by a build whose types this one cannot read: start fresh.
/// }
/// ```
public struct UndraRestoreError: Error, Sendable, Equatable {
    /// The non-zero code `undra_restore` returned.
    public let code: UInt32

    /// Creates a restore error.
    public init(code: UInt32) {
        self.code = code
    }

    /// Code 2: a store's restore function panicked (the panic was contained).
    public static let panicked = UndraRestoreError(code: 2)

    /// Code 5: the snapshot is malformed (it does not decode, for example a snapshot in the layout
    /// before ADR-037), names an unknown store type, has a null or duplicate handle, or a store
    /// rejected its values.
    public static let badSnapshot = UndraRestoreError(code: 5)

    /// Code 6: there is no running core, it is shut down, or `restore` was called from inside a
    /// core callback.
    public static let unavailable = UndraRestoreError(code: 6)

    /// Code 7: a store's persisted values cannot become this build's types: they neither migrate
    /// structurally (by name, losslessly) nor through a `#[undra::migrate]` hook (ADR-037,
    /// `RestoreError::Incompatible`). The core's ERROR log record, which the `Log` port receives,
    /// names the store, the signal and the reason.
    public static let incompatible = UndraRestoreError(code: 7)
}

extension UndraRestoreError: CustomStringConvertible {
    public var description: String {
        let meaning: String
        switch code {
        case UndraRestoreError.panicked.code:
            meaning = "a store's restore panicked"
        case UndraRestoreError.badSnapshot.code:
            meaning = "the snapshot is malformed or names an unknown store"
        case UndraRestoreError.unavailable.code:
            meaning = "the core is not running"
        case UndraRestoreError.incompatible.code:
            meaning = "a store's saved values cannot become this build's types; the core's error log says which"
        default:
            meaning = "unknown code"
        }
        return "the Undra core rejected the snapshot (code \(code): \(meaning)); a rejected restore leaves the core unchanged"
    }
}

// MARK: - UndraProtocolError

/// The core sent something the protocol does not allow.
public enum UndraProtocolError: Error, Sendable, Equatable {
    /// A message could not be decoded.
    case malformedMessage(context: String, error: WireError)
    /// `stream` was used on a method that replied with a plain result.
    case notAStream(callId: UInt32)
    /// A constructor replied with the null handle.
    case nullHandle
}

extension UndraProtocolError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .malformedMessage(let context, let error):
            return "malformed \(context) from the Undra core: \(error)"
        case .notAStream(let callId):
            return "call \(callId) was opened as a stream but the core replied with a plain result"
        case .nullHandle:
            return "the Undra core returned the null handle from a constructor"
        }
    }
}
