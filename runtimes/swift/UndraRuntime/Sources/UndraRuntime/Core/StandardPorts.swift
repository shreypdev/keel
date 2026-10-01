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

    enum Kv {
        static let portId: UInt32 = fnv1a32("port.Kv")
        static let get: UInt32 = fnv1a32("Kv.get")
        static let set: UInt32 = fnv1a32("Kv.set")
        static let delete: UInt32 = fnv1a32("Kv.delete")
        static let list: UInt32 = fnv1a32("Kv.list")
    }

    enum SecureStore {
        static let portId: UInt32 = fnv1a32("port.SecureStore")
        static let get: UInt32 = fnv1a32("SecureStore.get")
        static let set: UInt32 = fnv1a32("SecureStore.set")
        static let delete: UInt32 = fnv1a32("SecureStore.delete")
        static let list: UInt32 = fnv1a32("SecureStore.list")
    }

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
