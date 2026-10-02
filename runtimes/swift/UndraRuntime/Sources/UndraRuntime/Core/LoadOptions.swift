/// How to attach an `UndraCore` to a Rust core (docs/SPEC.md sections 11 and 17.3).
///
/// Apps load a core through the entry generated for it, `Undra<Namespace>`, which fills in the
/// core's table (``api``) and the schema hash its bindings expect (``expectedSchemaHash``):
///
/// ```swift
/// // In process: the core is linked into the app.
/// let core = try UndraPlaygroundCore.load()
///
/// // Dev loop: an `undra dev` core on the same machine.
/// let dev = try UndraPlaygroundCore.load(.remote(url: "ws://127.0.0.1:7878"))
/// ```
public struct LoadOptions: Sendable {
    /// Where the core lives.
    public enum Mode: Sendable, Equatable {
        /// Linked into this process; called through its C ABI table.
        case inproc
        /// Running elsewhere (`undra dev`); reached over the WebSocket at `url`.
        case remote(url: String)
    }

    /// A core's table: an address of immutable static data in the core's image, so sharing it
    /// between threads is safe.
    private struct TableAddress: @unchecked Sendable {
        let pointer: UnsafeRawPointer?
    }

    /// Where the core lives.
    public var mode: Mode
    /// The `UndraApi` table of an in-process core: what its `<namespace>_undra_api()` returns
    /// (docs/SPEC.md section 6, ADR-044). `nil` lets the generated entry (`Undra<Namespace>.load`)
    /// fill it in; ``UndraCore/load(_:)`` throws ``UndraLoadError/missingCoreTable`` without one.
    /// Ignored by a remote core.
    public var api: UnsafeRawPointer? {
        get { table.pointer }
        set { table = TableAddress(pointer: newValue) }
    }
    /// The port implementations to register.
    public var adapters: Adapters
    /// The schema hash the generated bindings were built for (`UndraIds.schemaHash`). `load`
    /// throws `UndraSchemaMismatchError` if the core reports another one. `nil` lets the
    /// generated entry fill it in; ``UndraCore/load(_:)`` throws
    /// ``UndraLoadError/missingSchemaHash`` without one.
    public var expectedSchemaHash: UInt64?
    /// The lowest level of core log records forwarded to the Log port (0 trace ... 5 fatal).
    public var logLevel: UInt8
    /// Seconds to wait for the remote handshake (`remote` only).
    public var connectTimeout: Double
    /// Seconds a blocking `callSync` or `construct` waits for the remote core (`remote` only).
    public var blockingCallTimeout: Double
    /// The most change-set entries the mirror keeps waiting for a drain. Past it, the thread that
    /// delivers the next change-set folds the queue in place (docs/SPEC.md section 11). Default
    /// 65,536.
    public var maxPendingEntries: Int
    /// The most bytes those entries may account for (their values plus 17 bytes each) before the
    /// queue is folded in place. Default 16 MiB.
    public var maxPendingBytes: Int
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
    /// var options = LoadOptions.inproc()
    /// options.onError = { assertionFailure("\($0)") }
    /// let core = try UndraPlaygroundCore.load(options)
    /// ```
    public var onError: (@Sendable (UndraUnhandledError) -> Void)?
    /// How a remote core reconnects by itself when its connection drops (ADR-051); `nil` turns it
    /// off, and a drop then closes the core. The default is on, with ``UndraReconnectPolicy/default``.
    public var reconnect: UndraReconnectPolicy?
    /// Called with every change of ``UndraCore/connectionState``, starting with `.connecting`, on
    /// the thread that changed it (a thread of the runtime's own for a reconnect). It must return
    /// quickly and must not call into the core.
    public var onConnectionChange: (@Sendable (UndraConnectionState) -> Void)?
    /// **Development only, and inert unless the core is a `remote` one served by `undra dev`.** Called
    /// with a one-line message the dev server says about itself, such as `Reloaded, state kept` after
    /// it rebuilt the core (ADR-053): show it in a status bar for a few seconds. `undra dev` tells every
    /// client that attaches soon after a rebuild, once; an in-process or production core never produces
    /// one, so the closure never runs there. The message is also written to the log. It runs on a queue
    /// of the runtime's own, never on the transport's: keep it short, and hop to the main actor before
    /// touching UI.
    public var onDevNotice: (@Sendable (String) -> Void)?

    /// Called once for every panic the core contained: a call, a stream, a detached task, an effect, a
    /// computed value, a port or callback dispatcher, a background task (ADR-046). The report is what an
    /// app hands to its crash reporter: the message, `file:line:column`, what the core was running, the
    /// Rust stack as addresses (and names, in debug builds) relative to the core's image, and the
    /// identity of the build. The call that panicked still fails with ``UndraCallError/panicked(message:backtrace:)``;
    /// this is the structured report of the same panic, and the only one of a panic in a task nobody awaits.
    ///
    /// The core reports from the thread that panicked, possibly while it holds its lock, so the runtime
    /// answers the core at once and runs the handler later **on the main thread**: once per report and in
    /// the order the panics happened. It may call into Undra and into the app's crash reporter freely.
    /// The default (`nil`) logs one error line (the operation, the message, the location) so that a panic is
    /// never silent; the core has already logged its own fatal record.
    ///
    /// ```swift
    /// // Hand the report to the app's crash reporter (any: Crashlytics, Sentry, your own).
    /// var options = LoadOptions.inproc()
    /// options.onPanic = { report in
    ///     MyCrashReporter.recordNonFatal(
    ///         name: "UndraPanic in \(report.operation)",
    ///         reason: report.message,
    ///         location: report.location,                    // file:line:column
    ///         frames: report.frames.map(\.address),         // offsets into the core's image
    ///         imageId: report.imageId                       // which build's symbol files resolve them
    ///     )
    /// }
    /// let core = try UndraPlaygroundCore.load(options)
    /// ```
    public var onPanic: (@Sendable (UndraPanicReport) -> Void)?

    private var table: TableAddress

    /// Creates options with every setting spelled out; prefer `inproc(...)` and `remote(...)`.
    public init(
        mode: Mode,
        api: UnsafeRawPointer? = nil,
        adapters: Adapters = .platformDefault,
        expectedSchemaHash: UInt64? = nil,
        logLevel: UInt8 = 2,
        connectTimeout: Double = 10,
        blockingCallTimeout: Double = 30,
        maxPendingEntries: Int = 65_536,
        maxPendingBytes: Int = 16 * 1024 * 1024,
        onError: (@Sendable (UndraUnhandledError) -> Void)? = nil,
        reconnect: UndraReconnectPolicy? = .default,
        onConnectionChange: (@Sendable (UndraConnectionState) -> Void)? = nil,
        onDevNotice: (@Sendable (String) -> Void)? = nil,
        onPanic: (@Sendable (UndraPanicReport) -> Void)? = nil
    ) {
        self.mode = mode
        self.table = TableAddress(pointer: api)
        self.adapters = adapters
        self.expectedSchemaHash = expectedSchemaHash
        self.logLevel = logLevel
        self.connectTimeout = connectTimeout
        self.blockingCallTimeout = blockingCallTimeout
        self.maxPendingEntries = maxPendingEntries
        self.maxPendingBytes = maxPendingBytes
        self.onError = onError
        self.reconnect = reconnect
        self.onConnectionChange = onConnectionChange
        self.onDevNotice = onDevNotice
        self.onPanic = onPanic
    }

    /// A core linked into this process, reached through its table `api` (filled in by the
    /// generated entry when `nil`).
    public static func inproc(
        api: UnsafeRawPointer? = nil,
        adapters: Adapters = .platformDefault,
        expectedSchemaHash: UInt64? = nil,
        onError: (@Sendable (UndraUnhandledError) -> Void)? = nil,
        onPanic: (@Sendable (UndraPanicReport) -> Void)? = nil
    ) -> LoadOptions {
        return LoadOptions(
            mode: .inproc,
            api: api,
            adapters: adapters,
            expectedSchemaHash: expectedSchemaHash,
            onError: onError,
            onPanic: onPanic
        )
    }

    /// An `undra dev` core reached over the WebSocket at `url` (`ws://host:port` or `wss://...`).
    public static func remote(
        url: String,
        adapters: Adapters = .platformDefault,
        expectedSchemaHash: UInt64? = nil,
        onError: (@Sendable (UndraUnhandledError) -> Void)? = nil,
        reconnect: UndraReconnectPolicy? = .default,
        onConnectionChange: (@Sendable (UndraConnectionState) -> Void)? = nil,
        onDevNotice: (@Sendable (String) -> Void)? = nil,
        onPanic: (@Sendable (UndraPanicReport) -> Void)? = nil
    ) -> LoadOptions {
        return LoadOptions(
            mode: .remote(url: url),
            adapters: adapters,
            expectedSchemaHash: expectedSchemaHash,
            onError: onError,
            reconnect: reconnect,
            onConnectionChange: onConnectionChange,
            onDevNotice: onDevNotice,
            onPanic: onPanic
        )
    }
}
