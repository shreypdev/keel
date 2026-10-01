// Objects and stores: the host-side owners of core handles (docs/SPEC.md sections 10.1, 11 and
// 17.3).

/// The base class of every generated object: it owns one handle of the core's object table.
///
/// `close()` releases the handle explicitly. `deinit` is only a backstop for objects that go out
/// of scope without being closed; it releases the handle too, so a leaked object is a late
/// release, not a leaked handle. `close()` is idempotent and safe from any thread.
///
/// The initializer adopts a handle that the core issued (from a constructor call). Generated
/// subclasses have a private `init(adopting:core:)` that calls it.
open class UndraObject: @unchecked Sendable {
    /// The core that issued `handle`.
    public let core: UndraCore
    /// The core's handle for this object.
    public let handle: UndraHandle

    private let closedFlag = Guarded<Bool>(false)

    /// Adopts `handle`, which `core` issued and which this object now owns.
    public init(core: UndraCore, handle: UndraHandle) {
        self.core = core
        self.handle = handle
        core.noteHandleAdopted()
    }

    /// Whether `close()` has run.
    public var isClosed: Bool {
        return closedFlag.withLock { (closed: inout Bool) -> Bool in
            return closed
        }
    }

    /// Releases the handle. Later calls do nothing. Calls made on the object afterwards fail:
    /// the core answers a released handle with a bad-request reply.
    public func close() {
        let first = closedFlag.withLock { (closed: inout Bool) -> Bool in
            if closed {
                return false
            }
            closed = true
            return true
        }
        if !first {
            return
        }
        core.mirror.unregister(handle)
        core.release(handle)
        core.noteHandleReleased()
    }

    deinit {
        close()
    }
}

/// The base class of every generated store: an object whose signals the mirror keeps up to date.
///
/// `init(core:handle:noCoalesce:)` registers the store with the core's mirror; change-set entries
/// for the store's handle then arrive at `apply(signal:op:reader:)` on the main actor. The
/// generated subclass (which is `@Observable`) overrides `apply` to decode each signal into its
/// properties. After `super.init` the generated initializer calls
/// `core.observe(handle, signal:on:)`, which applies the initial values before it returns.
///
/// The mirror applies a store's changes once per display frame, merged: each signal gets its last
/// full value and the keyed patches that followed it as one patch, so `apply` sees the state after
/// every change the core committed, not each state in between. Signals declared
/// `#[undra(no_coalesce)]` get every entry.
@MainActor
open class UndraStore: UndraObject, @unchecked Sendable {
    /// Adopts `handle` and registers this store with `core.mirror`.
    ///
    /// `noCoalesce` lists the ids of the store's signals declared `#[undra(no_coalesce)]`
    /// (generated code passes them): `apply` receives every value of those, in order, instead of
    /// the last one per drain. SwiftUI still renders once per frame whatever the model does, so a
    /// view may not show each intermediate value; the store's properties take every one.
    public init(core: UndraCore, handle: UndraHandle, noCoalesce: Set<UInt32> = []) {
        super.init(core: core, handle: handle)
        core.mirror.register(handle, noCoalesce: noCoalesce) { [weak self] signal, op, reader in
            guard let store = self else {
                return
            }
            store.apply(signal: signal, op: op, reader: &reader)
        }
    }

    /// Applies one change-set entry to the store's state.
    ///
    /// `signal` is the signal's zero-based index in the store, `op` says how to read the value,
    /// and `reader` is restricted to the entry's bytes. The base implementation ignores the
    /// entry; generated stores override it.
    open func apply(signal: UInt32, op: ChangeOp, reader: inout UndraReader) {}
}
