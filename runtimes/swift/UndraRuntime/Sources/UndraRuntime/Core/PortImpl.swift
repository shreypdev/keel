// The host side of ports (docs/SPEC.md sections 5.7, 6.3 and 8).
//
// Generated code adapts an implementation of a port protocol to a `PortImpl`
// (`httpPortImpl(_:)`, `clockPortImpl(_:)`, ...); the app hands it to
// `UndraCore.registerPort(_:_:)`. The core calls a port with the method id and the encoded
// arguments and expects the encoded result back.

/// A synchronous port method: encoded arguments in, encoded result out.
public typealias SyncPortMethod = @Sendable ([UInt8]) throws -> [UInt8]

/// An asynchronous port method: encoded arguments in, encoded result out.
public typealias AsyncPortMethod = @Sendable ([UInt8]) async throws -> [UInt8]

/// The method table of one port, keyed by method id (`fnv1a32("<Trait>.<method>")`).
///
/// * `sync` ports are answered inline, on whatever thread the core is running the call on
///   (often while the core lock is held). Their methods must be quick and must never call back
///   into the core or wait for another thread that might.
/// * `async` ports are answered later: the runtime runs the method in a task and sends the
///   reply with `undra_port_reply` when it finishes.
///
/// A method throws `UndraPortError(body:)` to answer with a typed error (port status 1): the
/// encoded `E` of the method's `Result<T, E>`, for example a `StorageError` from `Kv.get`. Any
/// other thrown error, an unknown method id and an undecodable argument list all answer with
/// "unavailable" (port status 2), which the core reports as `PortError::Unavailable` (or, for a
/// method with an error channel, as that error's "unavailable" variant). Since that is never how
/// a correct adapter reports a failure (ADR-049), an untyped throw is also logged at ERROR level,
/// naming the port, the method and the adapter.
public enum PortImpl: Sendable {
    /// Methods answered synchronously.
    case sync([UInt32: SyncPortMethod])
    /// Methods answered asynchronously.
    case async([UInt32: AsyncPortMethod])
}
