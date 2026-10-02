import UndraRuntime
import PlaygroundCore

/// A store driven through the runtime's raw API, for the checks the generated classes hide: which
/// entries one change-set carried, in which form (`op`), and how large their values were.
///
/// It does what a generated store's initializer does (`construct`, register with the mirror,
/// `observe`) and keeps the entries instead of decoding them into properties.
@MainActor
final class RawStore {
    /// One change-set entry as the mirror handed it over.
    struct Entry {
        let signal: UInt32
        let op: ChangeOp
        /// The encoded value (a full value, or the encoded keyed patch).
        let value: [UInt8]

        /// The value decoded as `Value`, which must use every byte.
        func decode<Value: UndraCodec>(_ type: Value.Type = Value.self) throws -> Value {
            var reader = UndraReader(value)
            let decoded = try Value.undraDecode(&reader)
            try reader.finish()
            return decoded
        }

        /// The value decoded as a keyed patch of `Item`s.
        func decodePatch<Item: UndraCodec>(_ type: Item.Type = Item.self) throws -> [PatchOp<Item>] {
            var reader = UndraReader(value)
            let ops: [PatchOp<Item>] = try UndraRuntime.decodePatch(&reader)
            try reader.finish()
            return ops
        }
    }

    let core: UndraCore
    let handle: UndraHandle
    /// Every entry received since the last `clear()`, oldest first.
    private(set) var entries: [Entry] = []
    /// Called for every entry as it arrives, on the main actor, from inside the mirror's apply:
    /// the place to test what an observer may do (S04 calls back into the core from here).
    var onEntry: (@MainActor (Entry) -> Void)?

    /// Constructs `type` with `method` and `args`, and registers for its change-sets. It does not
    /// observe yet; call `observe()`.
    init(core: UndraCore, type: UInt32, method: UInt32, args: [UInt8] = []) throws {
        self.core = core
        self.handle = try core.construct(type: type, method: method, args: args)
        core.mirror.register(handle) { [weak self] signal, op, reader in
            let value = Array(reader.readRemaining())
            let entry = Entry(signal: signal, op: op, value: value)
            self?.entries.append(entry)
            self?.onEntry?(entry)
        }
    }

    /// Registers for the change-sets of a handle this runner did not construct: one a restored snapshot re-issued in a
    /// runtime that never held it (S35 step 10). It does not observe yet; call `observe()`. `close()` releases the handle.
    init(core: UndraCore, adopting handle: UndraHandle) {
        self.core = core
        self.handle = handle
        core.mirror.register(handle) { [weak self] signal, op, reader in
            let value = Array(reader.readRemaining())
            let entry = Entry(signal: signal, op: op, value: value)
            self?.entries.append(entry)
            self?.onEntry?(entry)
        }
    }

    /// Starts observing every signal; the current values are in `entries` when it returns.
    func observe() {
        core.observe(handle, signal: Observe.allSignals, on: true)
    }

    /// Stops observing.
    func stopObserving() {
        core.observe(handle, signal: Observe.allSignals, on: false)
    }

    /// Forgets the entries received so far.
    func clear() {
        entries.removeAll()
    }

    /// Unregisters and releases the handle.
    func close() {
        core.mirror.unregister(handle)
        core.release(handle)
    }

    /// The entries of `signal`, oldest first.
    func entries(of signal: UInt32) -> [Entry] {
        return entries.filter { $0.signal == signal }
    }

    /// A synchronous method of the store.
    @discardableResult
    func callSync(_ method: UInt32, _ args: [UInt8] = []) throws -> [UInt8] {
        return try core.callSync(.objectMethod(handle: handle, methodId: method), method: method, args: args)
    }

    /// An asynchronous method of the store.
    @discardableResult
    func call(_ method: UInt32, _ args: [UInt8] = []) async throws -> [UInt8] {
        return try await core.call(.objectMethod(handle: handle, methodId: method), method: method, args: args)
    }
}
