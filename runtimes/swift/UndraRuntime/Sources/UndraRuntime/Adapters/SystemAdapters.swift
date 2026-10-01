// Clock, Rng and Log: the three sync ports every core needs (docs/SPEC.md section 8).

import Foundation
import Security

/// `Clock`: wall-clock milliseconds since the Unix epoch and a monotonic nanosecond counter.
public struct ClockAdapter: UndraAdapter {
    /// Creates the adapter.
    public init() {}

    public var portId: UInt32 {
        return StandardPorts.Clock.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return .sync([
            StandardPorts.Clock.nowMs: { _ in
                let milliseconds = (Date().timeIntervalSince1970 * 1000.0).rounded(.down)
                return Int64(milliseconds).undraEncoded()
            },
            StandardPorts.Clock.monotonicNs: { _ in
                return DispatchTime.now().uptimeNanoseconds.undraEncoded()
            },
        ])
    }
}

/// `Rng`: cryptographically secure random bytes from `SecRandomCopyBytes`.
public struct RngAdapter: UndraAdapter {
    /// The most bytes one `fill` may ask for.
    static let maxFill: UInt32 = 16 * 1024 * 1024

    /// Creates the adapter.
    public init() {}

    public var portId: UInt32 {
        return StandardPorts.Rng.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return .sync([
            StandardPorts.Rng.fill: { args in
                var reader = UndraReader(args)
                let length = try reader.readU32()
                try reader.finish()
                return try RngAdapter.randomBytes(count: length)
            },
        ])
    }

    /// The encoded `Bytes` reply for `count` random bytes.
    static func randomBytes(count: UInt32) throws -> [UInt8] {
        if count > RngAdapter.maxFill {
            throw PortAdapterError.invalidArgument("Rng.fill of \(count) bytes exceeds the limit of \(RngAdapter.maxFill)")
        }
        var bytes = [UInt8](repeating: 0, count: Int(count))
        if count > 0 {
            let status = bytes.withUnsafeMutableBytes { (buffer: UnsafeMutableRawBufferPointer) -> Int32 in
                guard let base = buffer.baseAddress else {
                    return errSecParam
                }
                // A nil generator selects the default one (kSecRandomDefault).
                return SecRandomCopyBytes(nil, buffer.count, base)
            }
            if status != errSecSuccess {
                throw PortAdapterError.failed("SecRandomCopyBytes returned \(status)")
            }
        }
        return UndraBytes(bytes).undraEncoded()
    }
}

/// `Log`: records from the core, written to the unified log under the subsystem `dev.undra.core`
/// with the record's target as the category.
public struct LogAdapter: UndraAdapter {
    /// Creates the adapter.
    public init() {}

    public var portId: UInt32 {
        return StandardPorts.Log.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return .sync([
            StandardPorts.Log.log: { args in
                var reader = UndraReader(args)
                let level = try reader.readU8()
                let target = try reader.readString()
                let message = try reader.readString()
                try reader.finish()
                UndraLog.forward(level: level, target: target, message: message)
                return []
            },
        ])
    }
}
