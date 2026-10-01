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

extension Adapters {
    /// Every standard port with its Apple-platform implementation.
    ///
    /// Each adapter can be replaced by another `PortImpl` with `replacing(portId:with:)`, or
    /// dropped with `removing(portId:)`. Lifecycle is not here: it is an event the app reports
    /// with `UndraLifecycle`.
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
        ])
    }
}

/// Why a port method could not do what the core asked. Thrown from port methods that have no
/// typed error in the port's signature (`Rng.fill`, for example); the runtime answers
/// "unavailable" (port status 2) and logs it at ERROR level. A method with an error channel
/// (`Kv`, `SecureStore`, `Fs`, `Http`) never throws this: it answers with its typed error.
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
