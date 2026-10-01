// The default adapters of the Apple platforms (docs/SPEC.md section 11):
//
//   Http                 URLSession                        HttpAdapter
//   Kv                   files in Application Support      KvAdapter
//   SecureStore          the Keychain                      SecureStoreAdapter
//   Fs                   FileManager                       FsAdapter
//   Clock, Rng, Log      Foundation / SecRandom / os.Logger  ClockAdapter, RngAdapter, LogAdapter
//   Timer                DispatchQueue                     TimerAdapter
//   Connectivity         NWPathMonitor                     ConnectivityAdapter
//   Lifecycle            called by the app                 UndraLifecycle
//   WebSocket (opt-in)   URLSessionWebSocketTask           URLSessionWebSocketAdapter
//   Sse (opt-in)         URLSession.bytes + SseParser      URLSessionSseAdapter
//   Db (opt-in)          the SQLite3 C API                 SQLiteDbAdapter
//
// The opt-in ports (ADR-047, ADR-048) are declared only by a core built with the `websocket`,
// `sse` or `db` feature; registering one a core does not declare is harmless.

extension Adapters {
    /// Every standard port with its Apple-platform implementation.
    ///
    /// Each adapter can be replaced by another `PortImpl` with `replacing(portId:with:)`, or
    /// dropped with `removing(portId:)`. Lifecycle is not here: it is an event the app reports
    /// with `UndraLifecycle`. WebSocket, Sse and Db are here for the cores that enable them; an
    /// app's own implementation replaces one through its binding
    /// (`replacing(WebSocketPortAdapter(MyWebSocket()))`).
    public static var platformDefault: Adapters {
        return Adapters([
            HttpAdapter(),
            KvAdapter(),
            SecureStoreAdapter(),
            FsAdapter(),
            ClockAdapter(),
            RngAdapter(),
            LogAdapter(),
            TimerAdapter(),
            ConnectivityAdapter(),
            URLSessionWebSocketAdapter(),
            URLSessionSseAdapter(),
            SQLiteDbAdapter(),
        ])
    }
}

/// Why a port method could not do what the core asked. Thrown from port methods that have no
/// typed error in the port's signature; the runtime answers "unavailable" (port status 2).
enum PortAdapterError: Error, Sendable, Equatable, CustomStringConvertible {
    /// The arguments are well-formed but not acceptable.
    case invalidArgument(String)
    /// The platform call failed.
    case failed(String)

    var description: String {
        switch self {
        case .invalidArgument(let reason):
            return "invalid argument: \(reason)"
        case .failed(let reason):
            return "failed: \(reason)"
        }
    }
}
