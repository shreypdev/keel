// The mirror: the host side of change-set delivery (docs/SPEC.md sections 5.5 and 11).
//
// The core reports state changes as change-sets (`Wire.ChangeSet`) through a callback that can
// run on any thread and must not call back into the core. The mirror copies each change-set into
// a queue and applies the queue on the main actor, where the stores that own the observable
// state live. Everything queued while the main actor was busy is applied in one hop, in commit
// order, so a burst of change-sets costs one main-actor turn, not one each.

/// Applies one change-set entry to the object registered for its handle: the signal id, how to
/// interpret the value, and a reader restricted to the entry's value bytes.
///
/// Registered handlers run on the main actor.
public typealias MirrorApply = @MainActor @Sendable (UInt32, ChangeOp, inout UndraReader) -> Void

/// The registry that routes change-set entries to the stores mirroring the core's signals.
///
/// Generated stores never talk to it directly: `UndraStore.init(core:handle:)` registers the
/// store and `UndraObject.close()` unregisters it. Registration and unregistration are safe from
/// any thread; applying (`flush()`) is main-actor only.
public final class Mirror: @unchecked Sendable {
    private struct State {
        var handlers: [UInt64: MirrorApply] = [:]
        var pending: [[UInt8]] = []
        var hopScheduled = false
        var flushing = false
    }

    private let state = Guarded<State>(State())

    init() {}

    /// Registers `apply` for `handle`, replacing any earlier registration of the same handle.
    public func register(_ handle: UndraHandle, _ apply: @escaping MirrorApply) {
        state.withLock { (current: inout State) -> Void in
            current.handlers[handle.rawValue] = apply
        }
    }

    /// Removes the registration of `handle`. Change-sets that arrive for it afterwards are
    /// dropped. Unknown handles are ignored.
    public func unregister(_ handle: UndraHandle) {
        state.withLock { (current: inout State) -> Void in
            current.handlers[handle.rawValue] = nil
        }
    }

    /// The number of registered handles.
    public var registeredCount: Int {
        return state.withLock { (current: inout State) -> Int in
            return current.handlers.count
        }
    }

    /// The number of change-sets that were received but not yet applied.
    public var pendingCount: Int {
        return state.withLock { (current: inout State) -> Int in
            return current.pending.count
        }
    }

    /// Applies every queued change-set, oldest first, on the main actor.
    ///
    /// Called by the runtime after each hop and by `UndraCore.observe` (so that a store has its
    /// initial values before its initializer returns). Re-entrant calls, made by a store that
    /// re-observes a signal from inside `apply`, return at once: the outer call keeps draining
    /// the queue and picks up whatever they caused.
    @MainActor
    public func flush() {
        let mayDrain = state.withLock { (current: inout State) -> Bool in
            if current.flushing {
                return false
            }
            current.flushing = true
            return true
        }
        if !mayDrain {
            return
        }
        while true {
            let batch = state.withLock { (current: inout State) -> [[UInt8]] in
                let taken = current.pending
                current.pending.removeAll(keepingCapacity: true)
                if taken.isEmpty {
                    current.flushing = false
                }
                return taken
            }
            if batch.isEmpty {
                return
            }
            for payload in batch {
                apply(payload)
            }
        }
    }

    // MARK: Delivery from the core

    /// Queues one change-set payload (already copied out of the callback's buffer) and makes
    /// sure a main-actor hop is coming. Safe from any thread, including a core callback.
    func enqueue(_ payload: [UInt8]) {
        let needsHop = state.withLock { (current: inout State) -> Bool in
            current.pending.append(payload)
            if current.hopScheduled {
                return false
            }
            current.hopScheduled = true
            return true
        }
        if needsHop {
            Task { @MainActor [weak self] in
                self?.hop()
            }
        }
    }

    @MainActor
    private func hop() {
        state.withLock { (current: inout State) -> Void in
            current.hopScheduled = false
        }
        flush()
    }

    @MainActor
    private func apply(_ payload: [UInt8]) {
        var reader = UndraReader(payload)
        do {
            try Wire.ChangeSet.forEachEntry(from: &reader) { handle, signal, op, value in
                let handler = self.state.withLock { (current: inout State) -> MirrorApply? in
                    return current.handlers[handle.rawValue]
                }
                if let handler = handler {
                    handler(signal, op, &value)
                }
            }
            try reader.finish()
        } catch {
            UndraLog.error("dropped a malformed change-set: \(error)")
        }
    }
}
