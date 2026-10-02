// `Lazy<T>` signals on the Swift side (ADR-043 decision 3): a list the core owns and the host pages through.
//
// `UndraLazyList` (iOS 17 / macOS 14, `@Observable`) and `UndraLazyListObject` (every floor, `ObservableObject`, ADR-045) are
// twins over one engine (`UndraLazyListEngine`), so they cannot drift.

import Combine
import Observation

/// A core-owned list the host pages through (a `Lazy<T>` store signal), for SwiftUI on iOS 17 / macOS 14 and later.
///
/// ```swift
/// List(0..<library.books.count, id: \.self) { index in
///     if let book = library.books[index] { BookRow(book) } else { BookRow.placeholder }
/// }
/// ```
@available(iOS 17, macOS 14, *)
@MainActor
@Observable
public final class UndraLazyList<Item: UndraCodec & Sendable> {
    /// The number of rows.
    public private(set) var count: Int = 0

    /// The number of rows in a page.
    public var pageSize: Int = 50

    /// The most pages kept in memory.
    public var maxCachedPages: Int = 24

    /// Creates a list that pages through `core`.
    public init(core: UndraCore) {}

    /// The row at `position`, or `nil` while its page loads (reading requests it).
    public subscript(position: Int) -> Item? {
        return nil
    }

    /// Requests the pages that hold `range`.
    public func prefetch(_ range: Range<Int>) {}

    /// A `Lazy<T>` signal's value (change-set op 0).
    public func applyFull(_ reader: inout UndraReader) throws {}

    /// A `Lazy<T>` signal's invalidation (change-set op 2).
    public func applyInvalidated(_ reader: inout UndraReader) throws {}
}

@available(iOS 17, macOS 14, *)
extension UndraLazyList: @preconcurrency RandomAccessCollection {
    public typealias Element = Item?
    public typealias Index = Int

    public var startIndex: Int { return 0 }
    public var endIndex: Int { return count }

    public func index(after i: Int) -> Int { return i + 1 }
    public func index(before i: Int) -> Int { return i - 1 }
}

/// The twin of ``UndraLazyList`` for every iOS and macOS version.
@MainActor
public final class UndraLazyListObject<Item: UndraCodec & Sendable>: ObservableObject {
    /// The number of rows.
    @Published public private(set) var count: Int = 0

    /// The number of rows in a page.
    public var pageSize: Int = 50

    /// The most pages kept in memory.
    public var maxCachedPages: Int = 24

    /// Creates a list that pages through `core`.
    public init(core: UndraCore) {}

    /// The row at `position`, or `nil` while its page loads (reading requests it).
    public subscript(position: Int) -> Item? {
        return nil
    }

    /// Requests the pages that hold `range`.
    public func prefetch(_ range: Range<Int>) {}

    /// A `Lazy<T>` signal's value (change-set op 0).
    public func applyFull(_ reader: inout UndraReader) throws {}

    /// A `Lazy<T>` signal's invalidation (change-set op 2).
    public func applyInvalidated(_ reader: inout UndraReader) throws {}
}

extension UndraLazyListObject: @preconcurrency RandomAccessCollection {
    public typealias Element = Item?
    public typealias Index = Int

    public var startIndex: Int { return 0 }
    public var endIndex: Int { return count }

    public func index(after i: Int) -> Int { return i + 1 }
    public func index(before i: Int) -> Int { return i - 1 }
}
