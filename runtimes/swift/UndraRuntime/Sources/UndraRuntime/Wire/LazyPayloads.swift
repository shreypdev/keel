// The three payloads of a `Lazy<T>` signal (ADR-043 decision 3.2; docs/SPEC.md sections 3.3 and 3.5).
//
// A `Lazy<T>` signal is a list the core owns and the host pages through. The core tells the host how long the list is and
// at which version (op 0 `Full`: `UndraLazyValue`; op 2 `LazyInvalidated`: `UndraLazyInvalidated`), and answers a page call
// (target 3) with a page header (`UndraLazyPageHeader`) followed by the items. `UndraLazyList` consumes them; they are public
// so that a test double or a recording tool can write them.

/// The value of a `Lazy<T>` signal (change-set op 0): the page server's handle, the list's length and its version.
///
/// Wire layout: `handle u64, len u32, version u64` (20 bytes). The handle is a transient object of the core's table that
/// serves the page calls; a restart or a snapshot restore sends a new one.
public struct UndraLazyValue: UndraPayload, Equatable {
    /// The page server: the `handle` of the page calls (`CallTarget.lazyListPage`).
    public var handle: UndraHandle
    /// The number of rows in the list at `version`.
    public var len: UInt32
    /// The list's version, bumped by every change; a page reply carries the version it was read at.
    public var version: UInt64

    /// Creates the value.
    public init(handle: UndraHandle, len: UInt32, version: UInt64) {
        self.handle = handle
        self.len = len
        self.version = version
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraLazyValue {
        let handle = try UndraHandle.undraDecode(&r)
        let len = try r.readU32()
        let version = try r.readU64()
        return UndraLazyValue(handle: handle, len: len, version: version)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        handle.undraEncode(&w)
        w.writeU32(len)
        w.writeU64(version)
    }
}

/// The value of a `Lazy<T>` signal's invalidation (change-set op 2): the new length and version, so the host knows the
/// length without a round trip and can tell which version a page was read at.
///
/// Wire layout: `len u32, version u64` (12 bytes).
public struct UndraLazyInvalidated: UndraPayload, Equatable {
    /// The number of rows in the list at `version`.
    public var len: UInt32
    /// The list's version after the change.
    public var version: UInt64

    /// Creates the value.
    public init(len: UInt32, version: UInt64) {
        self.len = len
        self.version = version
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraLazyInvalidated {
        let len = try r.readU32()
        let version = try r.readU64()
        return UndraLazyInvalidated(len: len, version: version)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU32(len)
        w.writeU64(version)
    }
}

/// What precedes the items in the reply to a page call (target 3): the version the page was read at, the list's total length
/// at that version and the number of items that follow.
///
/// Wire layout: `version u64, total u32, count u32`, then `count` items, each encoded as the item type. A reply for a page
/// at `offset` with `limit` carries exactly `min(limit, max(0, total - offset))` items.
public struct UndraLazyPageHeader: UndraPayload, Equatable {
    /// The list's version when the page was read.
    public var version: UInt64
    /// The list's length at `version`.
    public var total: UInt32
    /// The number of items that follow the header.
    public var count: UInt32

    /// Creates the header.
    public init(version: UInt64, total: UInt32, count: UInt32) {
        self.version = version
        self.total = total
        self.count = count
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraLazyPageHeader {
        let version = try r.readU64()
        let total = try r.readU32()
        let count = try r.readU32()
        return UndraLazyPageHeader(version: version, total: total, count: count)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU64(version)
        w.writeU32(total)
        w.writeU32(count)
    }
}
