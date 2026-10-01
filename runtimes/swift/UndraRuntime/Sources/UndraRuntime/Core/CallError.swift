// What a generated Swift method throws when the call itself fails (ADR-032, docs/SPEC.md section
// 17.3), and the one function that maps the runtime's raw errors onto it.
//
// A generated method fails with exactly one of three things: its own typed error `E` (reply status
// 1), `CancellationError` (the calling task was cancelled), or an `UndraCallError` (everything
// else). Generated code never traps on the outcome of a call.

import Foundation

/// Why a call into the core did not produce its result, when the reason is neither the method's own
/// error type nor the cancellation of the calling task (ADR-032).
///
/// Generated methods throw their own error (`TodoError`), `CancellationError`, or this.
///
/// ```swift
/// do {
///     _ = try await todos.add(title: draft)
/// } catch TodoError.emptyTitle {
///     problem = "Enter a title"
/// } catch is CancellationError {
///     // the view went away; nothing to do
/// } catch {
///     problem = error.localizedDescription      // an UndraCallError: panicked, refused, ...
/// }
/// ```
public enum UndraCallError: Error, Sendable, Equatable {
    /// The core cancelled the call: a restore replaced or invalidated the object it ran on, or the
    /// core shut down while it ran (reply status 3). A cancelled Swift task throws
    /// `CancellationError` instead.
    case cancelledByCore
    /// The core panicked while running the call (reply status 2). The core caught the panic and keeps
    /// working.
    case panicked(message: String, backtrace: String)
    /// The core refused the call without running it (reply status 5): the object was closed or
    /// replaced by a restore, the call was made from inside one of the core's callbacks
    /// (`E_REENTRANT`), or the request could not be decoded. `reason` is the core's text.
    case refused(reason: String)
    /// The core cannot be reached: this `UndraCore` is shut down or not loaded, or the remote
    /// connection closed or timed out.
    case unavailable(UndraTransportError)
    /// The core answered with something the bindings cannot read. After a successful schema check
    /// this is a bug in Undra; please report it with the text.
    case malformed(String)
}

extension UndraCallError: CustomStringConvertible, LocalizedError {
    /// A one-line description that reads well in an alert: `error.localizedDescription` is the same text.
    public var description: String {
        switch self {
        case .cancelledByCore:
            return "the Undra core cancelled the call (a restore replaced its object, or the core shut down)"
        case .panicked(let message, _):
            return "the Undra core panicked: \(message)"
        case .refused(let reason):
            return "the Undra core refused the call: \(reason)"
        case .unavailable(let transport):
            return "the Undra core is unavailable: \(transport.description)"
        case .malformed(let text):
            return "the Undra core sent a reply the bindings cannot read: \(text)"
        }
    }

    /// The same text as `description`.
    public var errorDescription: String? {
        return description
    }
}

// MARK: - Mapping

extension UndraCallError {
    /// The error a generated method without an error type throws for `error`, a failure of
    /// `UndraCore.callSync`, `call` or `construct`, or of decoding their result: `CancellationError`
    /// stays itself, everything else becomes an `UndraCallError`.
    ///
    /// Generated calls only ever pass errors the runtime produces, so the last row of the mapping
    /// (an error of any other type is returned unchanged) is never taken by them.
    public static func mapped(_ error: any Error) -> any Error {
        if error is CancellationError {
            return error
        }
        if let call = error as? UndraCallError {
            return call
        }
        if let reply = error as? UndraReplyError {
            return fromReply(reply)
        }
        if let transport = error as? UndraTransportError {
            return UndraCallError.unavailable(transport)
        }
        if let protocolError = error as? UndraProtocolError {
            return UndraCallError.malformed(protocolError.description)
        }
        if let wire = error as? WireError {
            return UndraCallError.malformed("the reply does not decode: \(wire)")
        }
        if let mode = error as? UndraModeError {
            return UndraCallError.refused(reason: mode.description)
        }
        return error
    }

    /// The same for a method whose Rust signature returns `Result<_, E>`: a typed reply becomes `E`.
    public static func mapped<E: UndraError>(_ error: any Error, domain: E.Type) -> any Error {
        if let reply = error as? UndraReplyError, reply.status == .error {
            do {
                return try E.undraDecoded(from: reply.body)
            } catch {
                return UndraCallError.malformed("a \(E.self) that does not decode (\(reply.body.count) bytes)")
            }
        }
        return mapped(error)
    }

    /// The error a generated stream method ends with, for a failure of `UndraCore.stream`.
    ///
    /// A stream's error item carries a `String` when the core ended the stream itself (a restore or
    /// a shutdown: `"cancelled: ..."`, or a panic: the message) and the encoded `E` for a typed
    /// error, so a stream without an error type reads the body as that `String`.
    public static func mapped(streamFailure error: any Error) -> any Error {
        if let reply = error as? UndraReplyError, reply.status == .error {
            return fromStreamItem(reply)
        }
        return mapped(error)
    }

    /// The same for a stream whose Rust signature carries an error type `E`: the body is tried as
    /// `E` first, then as the core's `String`.
    ///
    /// The two encodings can overlap for an `E` whose bytes also read as one `String` (an enum whose
    /// first variant carries a `UInt16`, for instance, and an empty message); `E` wins. A distinct
    /// wire flag for core-ended streams is a v2 item (ADR-032, Risks).
    public static func mapped<E: UndraError>(streamFailure error: any Error, domain: E.Type) -> any Error {
        if let reply = error as? UndraReplyError, reply.status == .error {
            if let typed = try? E.undraDecoded(from: reply.body) {
                return typed
            }
            return fromStreamItem(reply)
        }
        return mapped(error)
    }

    // MARK: Pieces

    /// The mapping of a reply status that is not the method's own error.
    private static func fromReply(_ reply: UndraReplyError) -> UndraCallError {
        switch reply.status {
        case .cancelled:
            return .cancelledByCore
        case .panic:
            var reader = UndraReader(reply.body)
            let message = try? reader.readString()
            let backtrace = try? reader.readString()
            guard let message = message else {
                return .panicked(message: "<undecodable panic report>", backtrace: "")
            }
            return .panicked(message: message, backtrace: backtrace ?? "")
        case .badRequest:
            return .refused(reason: reply.message ?? "<undecodable reason>")
        case .error:
            return .malformed("the core answered with a typed error, but this method has none (\(reply.body.count) bytes)")
        case .streamOpened:
            return .malformed("the core opened a stream where a single reply was expected")
        case .ok:
            return .malformed("the core answered ok as a failure")
        }
    }

    /// A stream error item (flag 2) read as the `String` the core writes when it ends a stream
    /// itself.
    private static func fromStreamItem(_ reply: UndraReplyError) -> UndraCallError {
        var reader = UndraReader(reply.body)
        guard let text = try? reader.readString(), (try? reader.finish()) != nil else {
            return .malformed("a stream error item that does not decode (\(reply.body.count) bytes)")
        }
        if text.hasPrefix("cancelled: ") {
            return .cancelledByCore
        }
        return .panicked(message: text, backtrace: "")
    }
}

// MARK: - UndraUnhandledError

/// A failure no caller could see: a generated command (a synchronous method that returns nothing and
/// has no error type) or a store's change could not be applied. Delivered to `LoadOptions.onError`.
public struct UndraUnhandledError: Error, Sendable, Equatable, CustomStringConvertible {
    /// What failed, as Swift spells it: `"Todos.toggle"`, `"configureRemote"`, `"Todos.apply(signal: 2)"`.
    public let operation: String
    /// Why.
    public let error: UndraCallError

    /// Creates the report of a failed `operation`.
    public init(operation: String, error: UndraCallError) {
        self.operation = operation
        self.error = error
    }

    /// `"<operation> failed: <error>"`.
    public var description: String {
        return "\(operation) failed: \(error.description)"
    }
}
