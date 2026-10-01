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
    /// The most change-set entries the mirror keeps waiting for a drain. Past it, the thread that
    /// delivers the next change-set folds the queue in place (docs/SPEC.md section 11). Default
    /// 65,536.
    public var maxPendingEntries: Int
    /// The most bytes those entries may account for (their values plus 17 bytes each) before the
    /// queue is folded in place. Default 16 MiB.
    public var maxPendingBytes: Int

    /// Creates options with every setting spelled out; prefer `inproc(...)` and `remote(...)`.
    public init(
        mode: Mode,
        adapters: Adapters = .platformDefault,
        expectedSchemaHash: UInt64,
        logLevel: UInt8 = 2,
        connectTimeout: Double = 10,
        blockingCallTimeout: Double = 30,
        maxPendingEntries: Int = 65_536,
        maxPendingBytes: Int = 16 * 1024 * 1024
    ) {
        self.mode = mode
        self.adapters = adapters
        self.expectedSchemaHash = expectedSchemaHash
        self.logLevel = logLevel
        self.connectTimeout = connectTimeout
        self.blockingCallTimeout = blockingCallTimeout
        self.maxPendingEntries = maxPendingEntries
        self.maxPendingBytes = maxPendingBytes
    }

    /// A core linked into this process.
    public static func inproc(
        adapters: Adapters = .platformDefault,
        expectedSchemaHash: UInt64
    ) -> LoadOptions {
        return LoadOptions(mode: .inproc, adapters: adapters, expectedSchemaHash: expectedSchemaHash)
    }

    /// An `undra dev` core reached over the WebSocket at `url` (`ws://host:port` or `wss://...`).
    public static func remote(
        url: String,
        adapters: Adapters = .platformDefault,
        expectedSchemaHash: UInt64
    ) -> LoadOptions {
        return LoadOptions(mode: .remote(url: url), adapters: adapters, expectedSchemaHash: expectedSchemaHash)
    }
}
