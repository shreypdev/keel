import Foundation
@testable import KeelRuntime

/// The `Kv` port of the harness: a dictionary in memory that also remembers every write, so a
/// scenario can ask what was persisted and when (S14 looks for the offline queue).
final class MemoryKv: KeelAdapter, @unchecked Sendable {
    /// One operation the core asked for.
    enum Operation: Equatable, Sendable {
        case set(key: String, value: [UInt8])
        case delete(key: String)

        /// The key the operation touched.
        var key: String {
            switch self {
            case .set(let key, _), .delete(let key):
                return key
            }
        }
    }

    private struct State {
        var entries: [String: [UInt8]] = [:]
        var operations: [Operation] = []
    }

    private let state = Locked<State>(State())

    var portId: UInt32 {
        return StandardPorts.Kv.portId
    }

    /// Every write and delete so far, oldest first.
    var operations: [Operation] {
        return state.withLock { (current: inout State) -> [Operation] in current.operations }
    }

    /// The value stored under `key`, if any.
    func value(for key: String) -> [UInt8]? {
        return state.withLock { (current: inout State) -> [UInt8]? in current.entries[key] }
    }

    func makePortImpl(core: KeelCore) -> PortImpl? {
        let state = self.state
        return .async([
            StandardPorts.Kv.get: { args in
                var reader = KeelReader(args)
                let key = try reader.readString()
                try reader.finish()
                let value = state.withLock { (current: inout State) -> [UInt8]? in current.entries[key] }
                return value.map(KeelBytes.init).keelEncoded()
            },
            StandardPorts.Kv.set: { args in
                var reader = KeelReader(args)
                let key = try reader.readString()
                let value = try reader.readBytes()
                try reader.finish()
                state.withLock { (current: inout State) -> Void in
                    current.entries[key] = value
                    current.operations.append(.set(key: key, value: value))
                }
                return []
            },
            StandardPorts.Kv.delete: { args in
                var reader = KeelReader(args)
                let key = try reader.readString()
                try reader.finish()
                state.withLock { (current: inout State) -> Void in
                    current.entries[key] = nil
                    current.operations.append(.delete(key: key))
                }
                return []
            },
            StandardPorts.Kv.list: { args in
                var reader = KeelReader(args)
                let prefix = try reader.readString()
                try reader.finish()
                let keys = state.withLock { (current: inout State) -> [String] in
                    return current.entries.keys.filter { $0.hasPrefix(prefix) }.sorted()
                }
                return keys.keelEncoded()
            },
        ])
    }
}
