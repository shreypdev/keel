// One wrapper per handle (ADR-040): the identity map of a core, `adopt` and the helpers generated
// code calls for an object a method returns or takes.
//
// Every handle in a successful reply is one reference the host owns. A wrapper (`UndraObject`) owns
// exactly one and gives it back once, at `close()` or `deinit`. When a reply carries a handle the host
// already has a live wrapper for, `adopt` returns that wrapper and gives the extra reference back at
// once, so `account.mailbox("inbox") === account.mailbox("inbox")` while the first is alive and a store
// returned twice is mirrored once.

/// The weak map from a core's handles to their live wrappers.
///
/// Keyed by the handle's number, under its own lock. An entry whose wrapper is gone or closed is
/// replaced by the next `adopt` of that handle and removed when its wrapper closes, so the map holds
/// no more entries than there are live wrappers (and the few being replaced).
final class ObjectIdentityMap: @unchecked Sendable {
    /// A weak reference to a wrapper, and how many open wrappers the handle has: usually one; a store whose `new`
    /// returns an object the host already wraps has two, each owning a reference.
    ///
    /// `id` names the wrapper the slot was made for, so that the map can tell whose slot it is **without loading the
    /// weak reference**: a load under the lock makes a strong temporary, and when another thread drops the wrapper's
    /// last reference meanwhile, that temporary is the last one: the wrapper deallocates inside the lock, its
    /// `deinit` closes it, and closing takes this lock again (a deadlock; it needs no more than a wrapper released on
    /// one thread while another adopts or closes the same handle).
    private struct Slot {
        weak var object: UndraObject?
        var id: ObjectIdentifier?
        var holders = 0
    }

    private let slots = Guarded<[UInt64: Slot]>([:])

    /// The live (not closed) wrapper of `handle`, if there is one.
    func live(_ handle: UndraHandle) -> UndraObject? {
        let found = slots.withLock { (current: inout [UInt64: Slot]) -> UndraObject? in
            return current[handle.rawValue]?.object
        }
        guard let object = found, !object.isClosed else {
            return nil
        }
        return object
    }

    /// Makes `object` the wrapper of its handle unless another live wrapper already is, and returns the
    /// wrapper of the handle afterwards.
    @discardableResult
    func register(_ object: UndraObject) -> UndraObject {
        let raw = object.handle.rawValue
        let existing = slots.withLock { (current: inout [UInt64: Slot]) -> UndraObject? in
            var slot = current[raw] ?? Slot()
            slot.holders += 1
            if let other = slot.object, other !== object {
                current[raw] = slot
                return other
            }
            slot.object = object
            slot.id = ObjectIdentifier(object)
            current[raw] = slot
            return nil
        }
        // A closed wrapper is no wrapper: the new one takes its place (checked outside the lock, since
        // `isClosed` takes the wrapper's own).
        if let existing = existing {
            if existing.isClosed {
                slots.withLock { (current: inout [UInt64: Slot]) -> Void in
                    current[raw]?.object = object
                    current[raw]?.id = ObjectIdentifier(object)
                }
                return object
            }
            return existing
        }
        return object
    }

    /// `object` closes (or is discarded as a duplicate): it stops counting as a holder of its handle and stops
    /// being its wrapper, and `cleanup` runs **under the map's lock** with whether it was the last holder.
    ///
    /// Running the cleanup under the lock is what makes a close race an `adopt` of the same handle on another
    /// thread: a wrapper that registers before the cleanup makes this one not the last, so the routing and the
    /// observed set the new wrapper depends on stay; one that registers after finds the cleanup done and sets
    /// them up again. The cleanup takes the mirror's and the core's locks and never calls into the map.
    func leave(_ object: UndraObject, cleanup: (_ last: Bool) -> Void) {
        let raw = object.handle.rawValue
        slots.withLock { (current: inout [UInt64: Slot]) -> Void in
            var slot = current[raw] ?? Slot(holders: 1)
            slot.holders = Swift.max(0, slot.holders - 1)
            if slot.id == nil || slot.id == ObjectIdentifier(object) {
                slot.object = nil
                slot.id = nil
            }
            let last = slot.holders == 0
            cleanup(last)
            current[raw] = last ? nil : slot
        }
    }

    /// The number of entries (live or waiting to be replaced), for tests.
    var count: Int {
        return slots.withLock { (current: inout [UInt64: Slot]) -> Int in
            return current.count
        }
    }
}

extension UndraCore {
    // MARK: Adopting handles the core issued

    /// The wrapper of `handle`, which a reply of this core carried as one reference the host now owns.
    ///
    /// When a live wrapper of `handle` exists it is returned and the reference the reply carried is given
    /// back at once; otherwise `make` wraps the handle (it runs outside any lock, so a store may observe
    /// its signals there) and the new wrapper owns the reference. Generated code calls this (or
    /// ``adoptObject(_:_:)`` and its siblings) for every object a call returns, so one handle has one
    /// wrapper.
    public func adopt<Object: UndraObject>(_ handle: UndraHandle, _ make: (UndraHandle, UndraCore) -> Object) -> Object {
        if let existing = identities.live(handle) as? Object {
            releaseExtraReference(handle)
            return existing
        }
        let made = make(handle, self)
        // `made` registered itself (`UndraObject.init`) unless another live wrapper of the handle appeared
        // while it was being made (a call on another thread, or a callback that a store's first drain
        // delivered): then that one is kept and `made` gives its reference back.
        guard let existing = identities.live(handle) as? Object, existing !== made else {
            noteHeld(handle)
            return made
        }
        made.discardAsDuplicate(of: existing)
        return existing
    }

    /// The object a reply body holds (one handle), adopted (``adopt(_:_:)``).
    ///
    /// - Throws: `UndraProtocolError.nullHandle` for the null handle, `WireError` for a body that is
    ///   not exactly one handle. Generated code maps both with ``UndraCallError/mapped(_:)``.
    public func adoptObject<Object: UndraObject>(_ body: [UInt8], _ make: (UndraHandle, UndraCore) -> Object) throws -> Object {
        var reader = UndraReader(body)
        let handle = try UndraCore.readHandle(&reader)
        try reader.finish()
        return adopt(handle, make)
    }

    /// The optional object a reply body holds (a tag, then a handle when present), adopted.
    public func adoptOptional<Object: UndraObject>(_ body: [UInt8], _ make: (UndraHandle, UndraCore) -> Object) throws -> Object? {
        var reader = UndraReader(body)
        let present = try reader.readU8()
        switch present {
        case 0:
            try reader.finish()
            return nil
        case 1:
            let handle = try UndraCore.readHandle(&reader)
            let object = adopt(handle, make)
            try reader.finish()
            return object
        default:
            throw WireError.invalidTag(tag: UInt32(present), at: 0, type: "Option")
        }
    }

    /// The objects a reply body holds (a count, then the handles), adopted one by one as they are read:
    /// should the body turn out malformed halfway, the wrappers already made own their references and
    /// give them back when they go.
    public func adoptList<Object: UndraObject>(_ body: [UInt8], _ make: (UndraHandle, UndraCore) -> Object) throws -> [Object] {
        var reader = UndraReader(body)
        let count = try reader.readU32()
        if UInt64(count) * 8 > UInt64(reader.remaining) {
            throw WireError.lengthTooLarge(len: count, at: 0)
        }
        var objects: [Object] = []
        objects.reserveCapacity(Int(count))
        var index: UInt32 = 0
        while index < count {
            let handle = try UndraCore.readHandle(&reader)
            objects.append(adopt(handle, make))
            index += 1
        }
        try reader.finish()
        return objects
    }

    private static func readHandle(_ reader: inout UndraReader) throws -> UndraHandle {
        let handle = try UndraHandle.undraDecode(&reader)
        if handle.isNull {
            throw UndraProtocolError.nullHandle
        }
        return handle
    }

    // MARK: Objects as arguments

    /// Checks that `object` was made by this core, before its handle is written into a call: a handle
    /// names an entry of one core's table, and two cores hand out the same numbers (ADR-044).
    ///
    /// - Throws: ``UndraCallError/refused(reason:)`` naming the object's class, when it belongs to
    ///   another core. Nothing has been sent then.
    public func requireOwn(_ object: UndraObject) throws {
        if object.core !== self {
            throw UndraCallError.refused(
                reason: "this \(type(of: object)) belongs to another Undra core; an object can only be passed to the core that made it"
            )
        }
    }

    /// `requireOwn` for an optional argument: `nil` is always accepted.
    public func requireOwn(_ object: UndraObject?) throws {
        if let object = object {
            try requireOwn(object)
        }
    }

    /// `requireOwn` for each object of a list argument.
    public func requireOwn(_ objects: [UndraObject]) throws {
        for object in objects {
            try requireOwn(object)
        }
    }
}

// MARK: - Identity

/// Wrappers are equal when they are the same wrapper. One wrapper per handle (``UndraCore/adopt(_:_:)``)
/// makes that the same as naming the same object of the same core, so `ForEach(shelves, id: \.self)`
/// works.
extension UndraObject: Hashable {
    public static func == (lhs: UndraObject, rhs: UndraObject) -> Bool {
        return lhs === rhs
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(ObjectIdentifier(self))
    }
}
