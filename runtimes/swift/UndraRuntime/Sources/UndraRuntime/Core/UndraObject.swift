// Objects and stores: the host-side owners of core handles (docs/SPEC.md sections 10.1, 11 and
// 17.3).

import Foundation

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

    /// Adopts `handle`, which `core` issued and which this object now owns, and makes this object the
    /// wrapper of the handle in `core`'s identity map (ADR-040) unless a live one already is.
    public init(core: UndraCore, handle: UndraHandle) {
        self.core = core
        self.handle = handle
        core.noteHandleAdopted()
        core.identities.register(self)
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
        core.identities.forget(self)
        core.mirror.unregister(handle)
        core.release(handle)
        core.noteHandleReleased()
    }

    /// Gives this wrapper's reference back without touching what the handle's other, kept wrapper
    /// `kept` registered (``UndraCore/adopt(_:_:)`` found two for one handle).
    func discardAsDuplicate(of kept: UndraObject) {
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
        core.releaseExtraReference(handle)
        core.noteHandleReleased()
        kept.reattach()
    }

    /// Registers again what a duplicate wrapper of the same handle may have replaced. Nothing for a
    /// plain object.
    func reattach() {}

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
        self.noCoalesce = noCoalesce
        super.init(core: core, handle: handle)
        registerWithMirror()
    }

    /// The store's `no_coalesce` signals, as registered.
    private let noCoalesce: Set<UInt32>

    private func registerWithMirror() {
        core.mirror.register(handle, noCoalesce: noCoalesce) { [weak self] signal, op, reader in
            guard let store = self else {
                return
            }
            store.apply(signal: signal, op: op, reader: &reader)
        }
    }

    /// A duplicate wrapper of this store's handle was made (it registered with the mirror in its
    /// place) and discarded: register again and observe again, so the mirror delivers here.
    nonisolated override func reattach() {
        let again = { @MainActor @Sendable [weak self] () -> Void in
            guard let store = self else {
                return
            }
            store.registerWithMirror()
            store.core.observe(store.handle, signal: Observe.allSignals, on: true)
        }
        // Stores are made on the main actor, so this runs there; the hop is only a safety net.
        if Thread.isMainThread {
            MainActor.assumeIsolated(again)
        } else {
            DispatchQueue.main.async {
                MainActor.assumeIsolated(again)
            }
        }
    }

    /// Applies one change-set entry to the store's state.
    ///
    /// `signal` is the signal's zero-based index in the store, `op` says how to read the value,
    /// and `reader` is restricted to the entry's bytes. The base implementation ignores the
    /// entry; generated stores override it.
    open func apply(signal: UInt32, op: ChangeOp, reader: inout UndraReader) {}
}
