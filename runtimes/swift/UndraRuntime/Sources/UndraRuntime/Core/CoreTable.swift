// One core's C ABI table (`UndraApi` in `undra.h`, docs/SPEC.md section 6, ADR-044), read and
// checked once, before anything in it is called.
//
// A core exports one function, `<namespace>_undra_api()`, returning a pointer to its table. The
// table is immutable static data in the core's image: the ABI version, the table's size, the
// schema hash, the namespace and seventeen function pointers. Reading it is the only place the
// runtime interprets memory the core gave it; everything after goes through the copied entries.

import UndraFFI

/// The entry points of one core, copied out of its `UndraApi` table.
///
/// `init(reading:)` makes the checks a host makes before calling anything (`undra.h`): the ABI
/// version first (the only field read before it is known to be a version 2 table), then the
/// table's size, then the namespace and every entry. The table never changes, so the copy never
/// goes stale.
struct CoreTable: Sendable {
    /// The core's namespace (`[core] namespace` of its undra.toml): what an in-process claim is
    /// keyed by.
    let namespace: String
    /// The hash of the schema the core was built from, readable without a running core.
    let schemaHash: UInt64

    /// `UndraBuf schema_json(void)`.
    let schemaJSON: @convention(c) () -> UndraBuf
    /// `uint32_t init(cfg, len, reply, changes, stream, user)`.
    let initialize: @convention(c) (
        UnsafePointer<UInt8>?,
        UInt32,
        undra_reply_cb?,
        undra_changeset_cb?,
        undra_stream_cb?,
        UnsafeMutableRawPointer?
    ) -> UInt32
    /// `void shutdown(void)`.
    let shutdown: @convention(c) () -> Void
    /// `uint32_t call(ptr, len)`.
    let call: @convention(c) (UnsafePointer<UInt8>?, UInt32) -> UInt32
    /// `UndraBuf call_sync(ptr, len)`.
    let callSync: @convention(c) (UnsafePointer<UInt8>?, UInt32) -> UndraBuf
    /// `void cancel(call_id)`.
    let cancel: @convention(c) (UInt32) -> Void
    /// `void stream_credit(call_id, credit)`.
    let streamCredit: @convention(c) (UInt32, UInt32) -> Void
    /// `void observe(handle, signal_id, on)`.
    let observe: @convention(c) (UInt64, UInt32, UInt8) -> Void
    /// `void release(handle)`.
    let release: @convention(c) (UInt64) -> Void
    /// `void port_register(port_id, cb, user)`.
    let portRegister: @convention(c) (UInt32, undra_port_cb?, UnsafeMutableRawPointer?) -> Void
    /// `void port_reply(ptr, len)`.
    let portReply: @convention(c) (UnsafePointer<UInt8>?, UInt32) -> Void
    /// `void event(port_id, method_id, ptr, len)`.
    let event: @convention(c) (UInt32, UInt32, UnsafePointer<UInt8>?, UInt32) -> Void
    /// `void timer_fired(timer_id)`.
    let timerFired: @convention(c) (UInt32) -> Void
    /// `UndraBuf snapshot(void)`.
    let snapshot: @convention(c) () -> UndraBuf
    /// `uint32_t restore(ptr, len)`.
    let restore: @convention(c) (UnsafePointer<UInt8>?, UInt32) -> UInt32
    /// `UndraBuf stats_json(void)`.
    let statsJSON: @convention(c) () -> UndraBuf
    /// `void buf_free(buf)`.
    let bufFree: @convention(c) (UndraBuf) -> Void

    /// Reads and checks the table `pointer` points to (the result of `<namespace>_undra_api()`).
    ///
    /// - Throws: `UndraLoadError.abiMismatch` when the table is not of C ABI version
    ///   ``UndraCore/abiVersion`` (nothing after the version is read then), and
    ///   `UndraLoadError.invalidCoreTable` when it is smaller than this runtime's `UndraApi`, has
    ///   no namespace, or has a null entry.
    init(reading pointer: UnsafeRawPointer) throws {
        // `abi_version` is the first field of every version of the table, and the one field a host
        // reads before it knows which layout follows.
        let version = pointer.load(fromByteOffset: 0, as: UInt32.self)
        if version != UndraCore.abiVersion {
            throw UndraLoadError.abiMismatch(expected: UndraCore.abiVersion, got: version)
        }
        let size = pointer.load(fromByteOffset: MemoryLayout<UInt32>.size, as: UInt32.self)
        let needed = MemoryLayout<UndraApi>.size
        if Int(size) < needed {
            throw UndraLoadError.invalidCoreTable("the table is \(size) bytes, a version 2 table is at least \(needed)")
        }
        // A version 2 table of at least this runtime's size: its memory is an `UndraApi` (fields
        // are only ever appended, so a larger table starts with the same layout).
        let api = pointer.assumingMemoryBound(to: UndraApi.self).pointee
        guard let name = api.name_space else {
            throw UndraLoadError.invalidCoreTable("the table has no namespace")
        }
        let namespace = String(cString: name)
        if namespace.isEmpty {
            throw UndraLoadError.invalidCoreTable("the table's namespace is empty")
        }
        func entry<Function>(_ value: Function?, _ field: String) throws -> Function {
            guard let value = value else {
                throw UndraLoadError.invalidCoreTable("the `\(field)` entry of core `\(namespace)` is null")
            }
            return value
        }
        self.namespace = namespace
        self.schemaHash = api.schema_hash
        self.schemaJSON = try entry(api.schema_json, "schema_json")
        self.initialize = try entry(api.`init`, "init")
        self.shutdown = try entry(api.shutdown, "shutdown")
        self.call = try entry(api.call, "call")
        self.callSync = try entry(api.call_sync, "call_sync")
        self.cancel = try entry(api.cancel, "cancel")
        self.streamCredit = try entry(api.stream_credit, "stream_credit")
        self.observe = try entry(api.observe, "observe")
        self.release = try entry(api.release, "release")
        self.portRegister = try entry(api.port_register, "port_register")
        self.portReply = try entry(api.port_reply, "port_reply")
        self.event = try entry(api.event, "event")
        self.timerFired = try entry(api.timer_fired, "timer_fired")
        self.snapshot = try entry(api.snapshot, "snapshot")
        self.restore = try entry(api.restore, "restore")
        self.statsJSON = try entry(api.stats_json, "stats_json")
        self.bufFree = try entry(api.buf_free, "buf_free")
    }
}
