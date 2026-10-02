// The standard ports (docs/SPEC.md section 8): their ids, and the runtime's own `RuntimeConfig`
// record. The records the ports exchange (`HttpRequest`, `HttpError`, ...) are public and live in
// StandardRecords.swift.
//
// This file needs no Foundation; the adapters that implement the ports import what they need.

// MARK: - Ids

/// `fnv1a32("port.<Trait>")` and `fnv1a32("<Trait>.<method>")` for the standard ports.
enum StandardPorts {
    enum Clock {
        static let portId: UInt32 = fnv1a32("port.Clock")
        static let nowMs: UInt32 = fnv1a32("Clock.now_ms")
        static let monotonicNs: UInt32 = fnv1a32("Clock.monotonic_ns")
    }

    enum Rng {
        static let portId: UInt32 = fnv1a32("port.Rng")
        static let fill: UInt32 = fnv1a32("Rng.fill")
    }

    enum Log {
        static let portId: UInt32 = fnv1a32("port.Log")
        static let log: UInt32 = fnv1a32("Log.log")
    }

    enum Http {
        static let portId: UInt32 = fnv1a32("port.Http")
        static let request: UInt32 = fnv1a32("Http.request")
    }

    /// `Kv`, async, every method with the `StorageError` channel (ADR-049):
    ///
    ///     get(key: String) -> Result<Option<Bytes>, StorageError>
    ///     set(key: String, value: Bytes) -> Result<(), StorageError>
    ///     delete(key: String) -> Result<(), StorageError>      a missing key is not an error
    ///     list(prefix: String) -> Result<Vec<String>, StorageError>   ascending
    ///
    /// A success answers port status 0 with the encoded `Ok` value; a failure answers status 1
    /// with the encoded `StorageError` (an `UndraPortError`). Status 2 ("unavailable") is left for
    /// a port with no adapter and for an adapter bug (an untyped throw), and the core reads it as
    /// `StorageError::Unavailable`.
    enum Kv {
        static let portId: UInt32 = fnv1a32("port.Kv")
        static let get: UInt32 = fnv1a32("Kv.get")
        static let set: UInt32 = fnv1a32("Kv.set")
        static let delete: UInt32 = fnv1a32("Kv.delete")
        static let list: UInt32 = fnv1a32("Kv.list")
    }

    /// `SecureStore`: the same four methods, shapes and `StorageError` channel as ``Kv``, under
    /// its own ids.
    enum SecureStore {
        static let portId: UInt32 = fnv1a32("port.SecureStore")
        static let get: UInt32 = fnv1a32("SecureStore.get")
        static let set: UInt32 = fnv1a32("SecureStore.set")
        static let delete: UInt32 = fnv1a32("SecureStore.delete")
        static let list: UInt32 = fnv1a32("SecureStore.list")
    }

    /// `Fs`, async, every method with the `FsError` channel.
    enum Fs {
        static let portId: UInt32 = fnv1a32("port.Fs")
        static let read: UInt32 = fnv1a32("Fs.read")
        static let write: UInt32 = fnv1a32("Fs.write")
        static let delete: UInt32 = fnv1a32("Fs.delete")
        static let list: UInt32 = fnv1a32("Fs.list")
    }

    enum Timer {
        static let portId: UInt32 = fnv1a32("port.Timer")
        static let set: UInt32 = fnv1a32("Timer.set")
    }

    enum Connectivity {
        static let portId: UInt32 = fnv1a32("port.Connectivity")
        static let changed: UInt32 = fnv1a32("Connectivity.changed")
    }

    enum Lifecycle {
        static let portId: UInt32 = fnv1a32("port.Lifecycle")
        static let changed: UInt32 = fnv1a32("Lifecycle.changed")
    }

    /// `Diagnostics`, sync, one method (ADR-046): the core hands every contained panic to it as one
    /// `PanicReport`, fire and forget (port call id 0, like a `Log` record).
    enum Diagnostics {
        static let portId: UInt32 = fnv1a32("port.Diagnostics")
        static let panicked: UInt32 = fnv1a32("Diagnostics.panicked")
    }

    // The opt-in ports (ADR-047, ADR-048): a core declares them only when it is built with the
    // `websocket`, `sse` or `db` feature. Registering one the core does not declare is harmless.

    /// `WebSocket` (ADR-047): `0x7388b95f`.
    enum WebSocket {
        static let portId: UInt32 = fnv1a32("port.WebSocket")
        static let connect: UInt32 = fnv1a32("WebSocket.connect")
        static let send: UInt32 = fnv1a32("WebSocket.send")
        static let receive: UInt32 = fnv1a32("WebSocket.receive")
        static let close: UInt32 = fnv1a32("WebSocket.close")
    }

    /// `Sse` (ADR-047): `0x75d2ef19`.
    enum Sse {
        static let portId: UInt32 = fnv1a32("port.Sse")
        static let open: UInt32 = fnv1a32("Sse.open")
        static let next: UInt32 = fnv1a32("Sse.next")
        static let close: UInt32 = fnv1a32("Sse.close")
    }

    /// `Db` (ADR-048): `0x559eda82`.
    enum Db {
        static let portId: UInt32 = fnv1a32("port.Db")
        static let open: UInt32 = fnv1a32("Db.open")
        static let execute: UInt32 = fnv1a32("Db.execute")
        static let query: UInt32 = fnv1a32("Db.query")
        static let begin: UInt32 = fnv1a32("Db.begin")
        static let commit: UInt32 = fnv1a32("Db.commit")
        static let rollback: UInt32 = fnv1a32("Db.rollback")
        static let close: UInt32 = fnv1a32("Db.close")
    }

    // MARK: Names, for diagnostics

    /// Every standard port and method, by name: `"Kv"`, `"Kv.get"`, ...
    private static let portNames: [String: [String]] = [
        "Clock": ["now_ms", "monotonic_ns"],
        "Rng": ["fill"],
        "Log": ["log"],
        "Http": ["request"],
        "Kv": ["get", "set", "delete", "list"],
        "SecureStore": ["get", "set", "delete", "list"],
        "Fs": ["read", "write", "delete", "list"],
        "Timer": ["set"],
        "Connectivity": ["changed"],
        "Lifecycle": ["changed"],
        "Diagnostics": ["panicked"],
        "WebSocket": ["connect", "send", "receive", "close"],
        "Sse": ["open", "next", "close"],
        "Db": ["open", "execute", "query", "begin", "commit", "rollback", "close"],
    ]

    private static let namesById: (ports: [UInt32: String], methods: [UInt32: String]) = {
        var ports: [UInt32: String] = [:]
        var methods: [UInt32: String] = [:]
        for (port, names) in portNames {
            ports[fnv1a32("port." + port)] = port
            for name in names {
                methods[fnv1a32(port + "." + name)] = port + "." + name
            }
        }
        return (ports, methods)
    }()

    /// The port's name for a log line: `"Kv"` for a standard port, `"port 0x1a2b3c4d"` otherwise.
    static func describe(portId: UInt32) -> String {
        return namesById.ports[portId] ?? "port 0x" + hex8(portId)
    }

    /// The method's name for a log line: `"Kv.get"` for a standard method, `"method 0x1a2b3c4d"`
    /// otherwise.
    static func describe(methodId: UInt32) -> String {
        return namesById.methods[methodId] ?? "method 0x" + hex8(methodId)
    }

    private static func hex8(_ value: UInt32) -> String {
        let digits = String(value, radix: 16)
        return String(repeating: "0", count: Swift.max(0, 8 - digits.count)) + digits
    }
}

// MARK: - Standard functions

/// `fnv1a32("fn.<name>")` for the standard functions: free functions every schema has and no binding
/// generates (ADR-024 and its amendment), which the runtime calls by itself.
enum StandardFunctions {
    /// `run_background(deadline_ms: u64) -> BackgroundReport` (ADR-046), an async call.
    static let runBackground: UInt32 = fnv1a32("fn.run_background")
}

// MARK: - RuntimeConfig

/// The `RuntimeConfig` record passed to `undra_init` (docs/SPEC.md section 6):
/// `platform String, mode String, core_threads u8, blocking_threads u8, log_level u8`.
struct RuntimeConfigRecord: UndraCodec, Sendable, Equatable {
    var platform: String
    var mode: String
    var coreThreads: UInt8
    var blockingThreads: UInt8
    var logLevel: UInt8

    static func undraDecode(_ r: inout UndraReader) throws -> RuntimeConfigRecord {
        let platform = try r.readString()
        let mode = try r.readString()
        let coreThreads = try r.readU8()
        let blockingThreads = try r.readU8()
        let logLevel = try r.readU8()
        return RuntimeConfigRecord(
            platform: platform,
            mode: mode,
            coreThreads: coreThreads,
            blockingThreads: blockingThreads,
            logLevel: logLevel
        )
    }

    func undraEncode(_ w: inout UndraWriter) {
        w.writeString(platform)
        w.writeString(mode)
        w.writeU8(coreThreads)
        w.writeU8(blockingThreads)
        w.writeU8(logLevel)
    }
}
