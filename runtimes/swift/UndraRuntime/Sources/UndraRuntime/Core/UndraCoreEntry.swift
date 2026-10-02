// The entry point of one core (ADR-044): what the generated `Undra<Namespace>` enum of a core's
// bindings delegates to. It knows the core's namespace, the schema hash of the bindings and how to
// reach the core's table, loads the core with them, and remembers the core it loaded so that every
// generated type of those bindings uses it by default.

/// Loads one core and holds it for the bindings generated for it.
///
/// Generated code creates one per core; apps use the generated enum around it:
///
/// ```swift
/// // Generated (Sources/PlaygroundCore/Generated/Core.swift):
/// public enum UndraPlaygroundCore {
///     private static let entry = UndraCoreEntry(
///         namespace: "playground_core",
///         schemaHash: UndraIds.schemaHash,
///         api: { playground_core_undra_api() }
///     )
///     public static func load(_ options: LoadOptions = .inproc()) throws -> UndraCore { try entry.load(options) }
///     public static var core: UndraCore { entry.core }
/// }
///
/// // App:
/// let core = try UndraPlaygroundCore.load()
/// ```
public final class UndraCoreEntry: Sendable {
    /// What the entry holds.
    private enum Slot {
        /// Nothing loaded through this entry, or what was loaded is shut down.
        case empty
        /// A `load` is running.
        case loading
        /// The core this entry loaded (possibly shut down since).
        case loaded(UndraCore)
    }

    /// The core's namespace (`[core] namespace` of its undra.toml).
    public let namespace: String
    /// The schema hash the core's bindings were generated for (`UndraIds.schemaHash`).
    public let schemaHash: UInt64
    /// Returns the core's table (`<namespace>_undra_api()`); called by an in-process `load` only,
    /// so a remote-only app never needs it to resolve to anything.
    private let api: @Sendable () -> UnsafeRawPointer?
    /// The bridges of the core's callback interfaces (ADR-041), installed on every core this entry loads.
    private let callbacks: [UndraCallbackInterface]
    private let slot = Guarded<Slot>(.empty)
    /// Whether the "load this core first" message has been logged.
    private let warned = Guarded<Bool>(false)

    /// An entry for the core `namespace`, whose bindings expect `schemaHash` and whose table `api`
    /// returns. `callbacks` are the bridges of the core's callback interfaces, which every load
    /// registers before it returns the core.
    public init(
        namespace: String,
        schemaHash: UInt64,
        api: @escaping @Sendable () -> UnsafeRawPointer?,
        callbacks: [UndraCallbackInterface] = []
    ) {
        self.namespace = namespace
        self.schemaHash = schemaHash
        self.api = api
        self.callbacks = callbacks
    }

    /// Loads the core and makes it ``core``.
    ///
    /// Fills in what `options` leave out: ``LoadOptions/expectedSchemaHash`` (this entry's
    /// ``schemaHash``) and, in process, ``LoadOptions/api`` (the core's table). Then it is
    /// ``UndraCore/load(_:)``: the result also becomes `UndraCore.shared` if no core is.
    ///
    /// - Throws: `UndraLoadError.alreadyLoaded` while the core this entry loaded is not shut down
    ///   (or another `load` of it is running); otherwise what `UndraCore.load(_:)` throws.
    @discardableResult
    public func load(_ options: LoadOptions) throws -> UndraCore {
        let claimed = slot.withLock { (current: inout Slot) -> Bool in
            switch current {
            case .loading:
                return false
            case .loaded(let core) where !core.isShutDown:
                return false
            case .empty, .loaded:
                current = .loading
                return true
            }
        }
        if !claimed {
            throw UndraLoadError.alreadyLoaded
        }
        var resolved = options
        if resolved.expectedSchemaHash == nil {
            resolved.expectedSchemaHash = schemaHash
        }
        if resolved.mode == .inproc, resolved.api == nil {
            resolved.api = api()
        }
        do {
            let core = try UndraCore.load(resolved)
            core.callbacks.install(callbacks)
            slot.withLock { (current: inout Slot) -> Void in
                current = .loaded(core)
            }
            return core
        } catch {
            slot.withLock { (current: inout Slot) -> Void in
                current = .empty
            }
            throw error
        }
    }

    /// The core this entry loaded while it is not shut down; otherwise the closed placeholder
    /// `UndraCore.shared` also returns when nothing is loaded, whose calls fail with
    /// ``UndraCallError/unavailable(_:)`` (`.closed`).
    ///
    /// Every generated type and function of the core's bindings defaults to it (`ctx:`). Using it
    /// before `load` succeeds or after `shutdown()` is a programming error but not a crash; the
    /// first such use logs what to do.
    public var core: UndraCore {
        let loaded = slot.withLock { (current: inout Slot) -> UndraCore? in
            if case .loaded(let core) = current {
                return core
            }
            return nil
        }
        if let loaded = loaded, !loaded.isShutDown {
            return loaded
        }
        let first = warned.withLock { (done: inout Bool) -> Bool in
            if done {
                return false
            }
            done = true
            return true
        }
        if first {
            UndraLog.error(
                "the Undra core `\(namespace)` was used while it is not loaded (before its load succeeds, or after shutdown()); calls on it fail with UndraCallError.unavailable(.closed). Load it at app startup with its generated entry's load(), before creating any of its objects."
            )
        }
        return UndraCore.unloaded
    }
}
