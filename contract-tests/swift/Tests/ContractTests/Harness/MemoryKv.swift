import Foundation
@testable import UndraRuntime

/// The `Kv` port of the harness (scenarios.md, "Adapters"): a dictionary in memory that records
/// every operation the core asks for, in order, and fails on demand.
///
/// A failure is a `StorageError` (ADR-049). The harness serves the port through the runtime's own
/// key-value port table (`KeyValuePort.makeImpl`, what `KvAdapter` uses over its files), so a
/// failure is answered exactly as the platform adapter answers one: port status 1 with the encoded
/// error. `fail(_:key:with:times:)` makes the next `times` operations of one kind (or of one key)
/// fail, or every one of them until `heal()`.
final class MemoryKv: UndraAdapter, KeyValueBackend, @unchecked Sendable {
    /// The kinds of operation of the `Kv` port.
    enum Kind: String, Sendable {
        case get
        case set
        case delete
        case list
    }

    /// One operation the core asked for, with the failure the harness answered it with (`nil`
    /// when it succeeded).
    enum Operation: Equatable, Sendable {
        case get(key: String, failure: StorageError?)
        case set(key: String, value: [UInt8], failure: StorageError?)
        case delete(key: String, failure: StorageError?)
        case list(prefix: String, failure: StorageError?)

        /// The kind of the operation.
        var kind: Kind {
            switch self {
            case .get: return .get
            case .set: return .set
            case .delete: return .delete
            case .list: return .list
            }
        }

        /// The key the operation touched (the prefix of a `list`).
        var key: String {
            switch self {
            case .get(let key, _), .set(let key, _, _), .delete(let key, _), .list(let key, _):
                return key
            }
        }

        /// The failure the harness answered with, or `nil` for a success.
        var failure: StorageError? {
            switch self {
            case .get(_, let failure), .set(_, _, let failure), .delete(_, let failure), .list(_, let failure):
                return failure
            }
        }
    }

    /// A failure the test asked for.
    private struct Rule {
        let kind: Kind?
        let key: String?
        let error: StorageError
        /// How many more operations it fails; `nil` until the store heals.
        var remaining: Int?

        func matches(_ kind: Kind, _ key: String) -> Bool {
            return (self.kind == nil || self.kind == kind) && (self.key == nil || self.key == key)
        }
    }

    private struct State {
        var entries: [String: [UInt8]]
        var operations: [Operation] = []
        var rules: [Rule] = []
    }

    private let state: Locked<State>

    /// A store holding `entries` (S14.8: build B starts from what build A persisted).
    init(entries: [String: [UInt8]] = [:]) {
        state = Locked(State(entries: entries))
    }

    var portId: UInt32 {
        return StandardPorts.Kv.portId
    }

    // MARK: Scripting

    /// Makes operations of `kind` (every kind if `nil`) on `key` (every key if `nil`) fail with
    /// `error`: the next `times` of them, or every one until `heal()` if `times` is `nil`.
    func fail(_ kind: Kind?, key: String? = nil, with error: StorageError, times: Int? = nil) {
        state.withLock { (current: inout State) -> Void in
            current.rules.append(Rule(kind: kind, key: key, error: error, remaining: times))
        }
    }

    /// Removes every failure the test asked for.
    func heal() {
        state.withLock { (current: inout State) -> Void in
            current.rules = []
        }
    }

    // MARK: Reading

    /// Every operation so far, oldest first, failed ones included.
    var operations: [Operation] {
        return state.withLock { (current: inout State) -> [Operation] in current.operations }
    }

    /// Every key and its value now.
    var entries: [String: [UInt8]] {
        return state.withLock { (current: inout State) -> [String: [UInt8]] in current.entries }
    }

    /// The value stored under `key`, if any.
    func value(for key: String) -> [UInt8]? {
        return state.withLock { (current: inout State) -> [UInt8]? in current.entries[key] }
    }

    // MARK: The port

    func makePortImpl(core: UndraCore) -> PortImpl? {
        return KeyValuePort.makeImpl(ids: .kv, backend: self)
    }

    /// Runs one operation under the lock: records it, and either fails it with the first rule
    /// that applies or performs it with `body`.
    private func perform<Value>(
        _ kind: Kind,
        _ key: String,
        record: (StorageError?) -> Operation,
        _ body: (inout [String: [UInt8]]) -> Value
    ) throws(StorageError) -> Value {
        let outcome = state.withLock { (current: inout State) -> Result<Value, StorageError> in
            if let index = current.rules.firstIndex(where: { $0.matches(kind, key) }) {
                let error = current.rules[index].error
                if let remaining = current.rules[index].remaining {
                    if remaining <= 1 {
                        current.rules.remove(at: index)
                    } else {
                        current.rules[index].remaining = remaining - 1
                    }
                }
                current.operations.append(record(error))
                return .failure(error)
            }
            current.operations.append(record(nil))
            return .success(body(&current.entries))
        }
        return try outcome.get()
    }

    func get(_ key: String) throws(StorageError) -> [UInt8]? {
        return try perform(.get, key, record: { .get(key: key, failure: $0) }) { (entries: inout [String: [UInt8]]) -> [UInt8]? in
            return entries[key]
        }
    }

    func set(_ key: String, _ value: [UInt8]) throws(StorageError) {
        try perform(.set, key, record: { .set(key: key, value: value, failure: $0) }) { (entries: inout [String: [UInt8]]) -> Void in
            entries[key] = value
        }
    }

    func delete(_ key: String) throws(StorageError) {
        try perform(.delete, key, record: { .delete(key: key, failure: $0) }) { (entries: inout [String: [UInt8]]) -> Void in
            entries[key] = nil
        }
    }

    func list(prefix: String) throws(StorageError) -> [String] {
        return try perform(.list, prefix, record: { .list(prefix: prefix, failure: $0) }) { (entries: inout [String: [UInt8]]) -> [String] in
            return entries.keys.filter { $0.hasPrefix(prefix) }.sorted()
        }
    }
}

// MARK: - Persisted formats (ADR-037)

/// The keys and layouts of what the query client persists, as the scenarios read them.
enum Persisted {
    /// The offline queue (format 2).
    static let queueKey = "undra.query.queue2"
    /// The dead-letter queue.
    static let deadLetterKey = "undra.query.queue.dead"

    /// The number of items of a format-2 queue: `format u16 = 2, schema_hash u64, count u32, ..`, the
    /// `u32` at offset 10; `nil` if the value is not that.
    static func queueCount(_ value: [UInt8]) -> UInt32? {
        guard value.count >= 14, value[0] == 2, value[1] == 0 else {
            return nil
        }
        var reader = UndraReader(Array(value[10 ..< 14]))
        return try? reader.readU32()
    }

    /// The fingerprint recorded at `offset` (a `u64`), if the value is long enough.
    static func fingerprint(_ value: [UInt8], at offset: Int) -> UInt64? {
        guard value.count >= offset + 8 else {
            return nil
        }
        var reader = UndraReader(Array(value[offset ..< offset + 8]))
        return try? reader.readU64()
    }

    /// `undra.types.<fingerprint as 16 hex digits>`: where a closure description is stored.
    static func typesKey(_ fingerprint: UInt64) -> String {
        return "undra.types." + hex(fingerprint, digits: 16)
    }

    /// `undra.query.cache2.<query id>.<fnv1a64 of the encoded arguments>`: the key of a cache entry.
    static func cacheKey(queryId: UInt32, arguments: [UInt8]) -> String {
        return "undra.query.cache2." + hex(UInt64(queryId), digits: 8) + "." + hex(fnv1a64(bytes: arguments), digits: 16)
    }

    /// FNV-1a, 64 bits, over bytes.
    static func fnv1a64(bytes: [UInt8]) -> UInt64 {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in bytes {
            hash ^= UInt64(byte)
            hash = hash &* 0x0000_0100_0000_01B3
        }
        return hash
    }

    /// `value` in lowercase hex, zero-padded to `digits`.
    static func hex(_ value: UInt64, digits: Int) -> String {
        let text = String(value, radix: 16)
        return String(repeating: "0", count: max(0, digits - text.count)) + text
    }
}
