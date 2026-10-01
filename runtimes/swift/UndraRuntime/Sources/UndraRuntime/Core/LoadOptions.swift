/// How to attach an `UndraCore` to a Rust core (docs/SPEC.md sections 11 and 17.3).
///
/// ```swift
/// // In process: the core is linked into the app.
/// let core = try UndraCore.load(.inproc(expectedSchemaHash: UndraIds.schemaHash))
///
/// // Dev loop: an `undra dev` core on the same machine.
/// let dev = try UndraCore.load(.remote(url: "ws://127.0.0.1:7878", expectedSchemaHash: UndraIds.schemaHash))
/// ```
public struct LoadOptions: Sendable {
    /// Where the core lives.
    public enum Mode: Sendable, Equatable {
        /// Linked into this process; called through the C ABI.
        case inproc
        /// Running elsewhere (`undra dev`); reached over the WebSocket at `url`.
        case remote(url: String)
    }

    /// Where the core lives.
    public var mode: Mode
    /// The port implementations to register.
    public var adapters: Adapters
    /// The schema hash the generated bindings were built for (`UndraIds.schemaHash`). `load`
    /// throws `UndraSchemaMismatchError` if the core reports another one.
    public var expectedSchemaHash: UInt64
    /// The lowest level of core log records forwarded to the Log port (0 trace ... 5 fatal).
    public var logLevel: UInt8
    /// Seconds to wait for the remote handshake (`remote` only).
    public var connectTimeout: Double
    /// Seconds a blocking `callSync` or `construct` waits for the remote core (`remote` only).
    public var blockingCallTimeout: Double
    /// Called when a failure reaches nobody: a generated command (a synchronous method that returns
    /// nothing and has no error type, such as `todos.toggle(id:)`) could not run, or a store could
    /// not apply a change from the core (ADR-032). Calls that can throw never reach it.
    ///
    /// The failure has already been logged at error level (unified log, subsystem
    /// `dev.undra.runtime`) when the handler runs. It runs synchronously on the thread that made the
    /// call (the main actor for a store), so keep it short, and it must not call into Undra: a failure
    /// reported while the handler is running is only logged, so a handler that calls a failing
    /// command cannot recurse (a `Task` started from inside the handler inherits that, so what it
    /// reports is only logged too). The default (`nil`) logs and returns.
    ///
    /// ```swift
    /// // Stop at the failing line in debug builds; the default never stops the process.
    /// var options = LoadOptions.inproc(expectedSchemaHash: UndraIds.schemaHash)
    /// options.onError = { assertionFailure("\($0)") }
    /// ```
    public var onError: (@Sendable (UndraUnhandledError) -> Void)?

    /// Creates options with every setting spelled out; prefer `inproc(...)` and `remote(...)`.
    public init(
        mode: Mode,
        adapters: Adapters = .platformDefault,
        expectedSchemaHash: UInt64,
        logLevel: UInt8 = 2,
        connectTimeout: Double = 10,
        blockingCallTimeout: Double = 30,
        onError: (@Sendable (UndraUnhandledError) -> Void)? = nil
    ) {
        self.mode = mode
        self.adapters = adapters
        self.expectedSchemaHash = expectedSchemaHash
        self.logLevel = logLevel
        self.connectTimeout = connectTimeout
        self.blockingCallTimeout = blockingCallTimeout
        self.onError = onError
    }

    /// A core linked into this process.
    public static func inproc(
        adapters: Adapters = .platformDefault,
        expectedSchemaHash: UInt64,
        onError: (@Sendable (UndraUnhandledError) -> Void)? = nil
    ) -> LoadOptions {
        return LoadOptions(
            mode: .inproc,
            adapters: adapters,
            expectedSchemaHash: expectedSchemaHash,
            onError: onError
        )
    }

    /// An `undra dev` core reached over the WebSocket at `url` (`ws://host:port` or `wss://...`).
    public static func remote(
        url: String,
        adapters: Adapters = .platformDefault,
        expectedSchemaHash: UInt64,
        onError: (@Sendable (UndraUnhandledError) -> Void)? = nil
    ) -> LoadOptions {
        return LoadOptions(
            mode: .remote(url: url),
            adapters: adapters,
            expectedSchemaHash: expectedSchemaHash,
            onError: onError
        )
    }
}
