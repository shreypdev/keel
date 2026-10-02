// `Lazy<T>` store signals on the Swift side (ADR-043 decision 3, docs/SPEC.md section 11).
//
// A `Lazy<T>` is a list the core owns and the host pages through: the host holds the length, a bounded cache of pages, and asks
// for the pages it shows. Two public classes share one engine (`UndraLazyListEngine`) so that they cannot drift:
// `UndraLazyList` for iOS 17 / macOS 14 and later (`@Observable`) and `UndraLazyListObject` for every floor (`ObservableObject`,
// ADR-045). Generated stores hold one per `Lazy<T>` signal and hand it the change-set entries of that signal:
//
//     case .fullValue:            try books.applyFull(&reader)
//     case .lazyListInvalidated:  try books.applyInvalidated(&reader)

import Combine
import Observation

/// A core-owned list the host pages through (a `Lazy<T>` store signal), for SwiftUI on iOS 17 / macOS 14 and later: an `@Observable`
/// random-access collection of optional rows. A floor below iOS 17 (ADR-045) uses ``UndraLazyListObject`` instead.
///
/// `count` is the length of the list. `list[index]` is the row, or `nil` while its page loads; reading it **requests** the page
/// that holds it and one page on each side (once), and never blocks. The requests of one main-actor turn are sent together after
/// it: synchronously where the transport allows (in process), asynchronously otherwise. When pages arrive, or the length changes,
/// whatever read `count` or `list[index]` is told, so a SwiftUI view that reads them updates.
///
/// ```swift
/// ScrollView {
///     LazyVStack {
///         ForEach(0..<library.books.count, id: \.self) { index in
///             BookRow(books: library.books, index: index)   // reads books[index] in its own body
///         }
///     }
/// }
/// ```
///
/// Put the rows in a lazy container (`LazyVStack`, `LazyVGrid`) and read `list[index]` in the row's own `body`, so that only the
/// rows near the screen are built and only their pages are read, and a row is updated when its own page arrives. A SwiftUI `List`
/// builds the rows of a `ForEach` up front (every row, on iOS 26: the playground's Library screen measured it), so a list of ten
/// thousand rows would read every page, and the cache (``maxCachedPages``) cannot hold them all: it keeps loading and dropping
/// pages. For a small list, or a `List` over rows that fit the cache, `List` is fine.
///
/// The list keeps at most ``maxCachedPages`` pages (least recently read first out). When the core changes the list it sends the
/// new length and version; the rows already cached stay on screen, stale, while the pages read since the previous change are asked
/// for again, so a change costs the pages of the window and never the whole list. Indexes outside `0..<count` return `nil` and
/// request nothing.
///
/// Iterating the collection (`for row in list`) reads every index and so asks for every page (the most recent
/// ``maxCachedPages`` of them): use `prefetch(_:)` and indexes for a window instead.
@available(iOS 17, macOS 14, *)
@MainActor
@Observable
public final class UndraLazyList<Item: UndraCodec & Sendable> {
    /// The number of rows: the length the core last announced (the signal's value, or the newest invalidation).
    public private(set) var count: Int = 0

    @ObservationIgnored
    private let engine: UndraLazyListEngine<Item>

    /// Bumped when pages are cached, replaced or dropped: what a read of `list[index]` depends on.
    private var revision: Int = 0

    /// Creates a list that pages through `core`. A generated store makes one per `Lazy<T>` signal, in its initializer, and hands
    /// it the signal's change-set entries (``applyFull(_:)``, ``applyInvalidated(_:)``).
    public convenience init(core: UndraCore) {
        self.init(engine: UndraLazyListEngine<Item>(core: core))
    }

    /// Creates a list whose request batches are started by `schedule` (tests drive the turn by hand).
    convenience init(core: UndraCore, schedule: @escaping UndraLazyListSchedule) {
        self.init(engine: UndraLazyListEngine<Item>(core: core, schedule: schedule))
    }

    private init(engine: UndraLazyListEngine<Item>) {
        self.engine = engine
        engine.onChange = { [weak self] change in
            self?.engineChanged(change)
        }
    }

    /// The number of rows in a page: 50 by default, between 1 and 1,048,576. Setting another value drops the cache. Set it once,
    /// before the list is read.
    public var pageSize: Int {
        get {
            return engine.pageSize
        }
        set {
            engine.pageSize = newValue
        }
    }

    /// The most pages kept in memory: 24 by default, at least 1. The least recently read page goes first; the pages of the window
    /// (read since the last change of the list) go last.
    public var maxCachedPages: Int {
        get {
            return engine.maxCachedPages
        }
        set {
            engine.maxCachedPages = newValue
        }
    }

    /// The row at `position`, or `nil` while its page loads or when `position` is outside `0..<count`.
    ///
    /// Reading a row of a page that is not cached requests that page and one page on each side; reading a row of a page the core
    /// has changed since requests it again (the stale row is returned meanwhile). Nothing blocks and nothing is requested from
    /// inside the call: the requests are queued and sent after the current main-actor turn.
    public subscript(position: Int) -> Item? {
        _ = revision
        guard position >= 0, position < count else {
            return nil
        }
        return engine.item(at: position)
    }

    /// Requests the pages that hold `range` (the part of it inside `0..<count`, at most ``maxCachedPages`` pages), for a view that
    /// is about to show them. Pages already cached and current are not requested again.
    public func prefetch(_ range: Range<Int>) {
        engine.prefetch(range)
    }

    /// Applies the signal's value (change-set op 0, a `LazyValue`): the page server's handle, the length and the version. A new
    /// handle (a restart, a restored snapshot) drops the cache. Generated stores call it; it reads the whole of `reader`.
    ///
    /// - Throws: `WireError` for a value that does not decode, ``UndraLazyListError/nullHandle`` for the null handle. The list is
    ///   left as it was.
    public func applyFull(_ reader: inout UndraReader) throws {
        try engine.applyFull(&reader)
    }

    /// Applies the signal's invalidation (change-set op 2, a `LazyInvalidated`): the new length and version are taken at once, the
    /// rows cached stay visible (stale) and the pages of the window are requested again. An invalidation that is not newer than
    /// what the list knows (the one that follows a page that already raised the version) changes nothing. Generated stores call it.
    ///
    /// - Throws: `WireError` for a value that does not decode. The list is left as it was.
    public func applyInvalidated(_ reader: inout UndraReader) throws {
        try engine.applyInvalidated(&reader)
    }

    private func engineChanged(_ change: UndraLazyListChange) {
        if change.contains(.count) {
            count = engine.count
        }
        if change.contains(.pages) {
            revision &+= 1
        }
    }

    /// The engine, for tests.
    var testEngine: UndraLazyListEngine<Item> {
        return engine
    }
}

@available(iOS 17, macOS 14, *)
extension UndraLazyList: @preconcurrency RandomAccessCollection {
    /// The row at an index, `nil` while it loads.
    public typealias Element = Item?
    /// A row number.
    public typealias Index = Int

    /// Always 0.
    public var startIndex: Int {
        return 0
    }

    /// The number of rows (reading it registers a dependency on the length).
    public var endIndex: Int {
        return count
    }

    /// The index after `i`.
    public func index(after i: Int) -> Int {
        return i + 1
    }

    /// The index before `i`.
    public func index(before i: Int) -> Int {
        return i - 1
    }
}

/// A core-owned list the host pages through (a `Lazy<T>` store signal), for SwiftUI on every iOS and macOS version: an
/// `ObservableObject` random-access collection of optional rows, the twin of ``UndraLazyList`` (which needs iOS 17) and the one to
/// use when the app supports iOS 15 or 16 (ADR-045). Same paging, same cache, same rules; `count` is `@Published` and
/// `objectWillChange` is sent once per change (a length, or pages that arrived), however many pages arrive in it.
///
/// ```swift
/// struct BookList: View {
///     @ObservedObject var books: UndraLazyListObject<Book>   // library.books
///     var body: some View {
///         ScrollView {
///             LazyVStack {
///                 ForEach(0..<books.count, id: \.self) { index in
///                     if let book = books[index] { BookRow(book) } else { BookRow.placeholder }
///                 }
///             }
///         }
///     }
/// }
/// ```
///
/// An `ObservableObject` re-renders every view that observes it when it changes, so a list this big belongs in a lazy container
/// (`LazyVStack`), not in a `List`, which builds all of its rows (see ``UndraLazyList``).
@MainActor
public final class UndraLazyListObject<Item: UndraCodec & Sendable>: ObservableObject {
    /// The number of rows: the length the core last announced (the signal's value, or the newest invalidation).
    @Published public private(set) var count: Int = 0

    private let engine: UndraLazyListEngine<Item>

    /// Bumped when pages change and the length does not, so that `objectWillChange` is sent for them.
    @Published private var revision: Int = 0

    /// Creates a list that pages through `core`. A generated store makes one per `Lazy<T>` signal, in its initializer, and hands
    /// it the signal's change-set entries (``applyFull(_:)``, ``applyInvalidated(_:)``).
    public convenience init(core: UndraCore) {
        self.init(engine: UndraLazyListEngine<Item>(core: core))
    }

    /// Creates a list whose request batches are started by `schedule` (tests drive the turn by hand).
    convenience init(core: UndraCore, schedule: @escaping UndraLazyListSchedule) {
        self.init(engine: UndraLazyListEngine<Item>(core: core, schedule: schedule))
    }

    private init(engine: UndraLazyListEngine<Item>) {
        self.engine = engine
        engine.onChange = { [weak self] change in
            self?.engineChanged(change)
        }
    }

    /// The number of rows in a page: 50 by default, between 1 and 1,048,576. Setting another value drops the cache. Set it once,
    /// before the list is read.
    public var pageSize: Int {
        get {
            return engine.pageSize
        }
        set {
            engine.pageSize = newValue
        }
    }

    /// The most pages kept in memory: 24 by default, at least 1. The least recently read page goes first; the pages of the window
    /// (read since the last change of the list) go last.
    public var maxCachedPages: Int {
        get {
            return engine.maxCachedPages
        }
        set {
            engine.maxCachedPages = newValue
        }
    }

    /// The row at `position`, or `nil` while its page loads or when `position` is outside `0..<count`.
    ///
    /// Reading a row of a page that is not cached requests that page and one page on each side; reading a row of a page the core
    /// has changed since requests it again (the stale row is returned meanwhile). Nothing blocks and nothing is requested from
    /// inside the call: the requests are queued and sent after the current main-actor turn.
    public subscript(position: Int) -> Item? {
        guard position >= 0, position < count else {
            return nil
        }
        return engine.item(at: position)
    }

    /// Requests the pages that hold `range` (the part of it inside `0..<count`, at most ``maxCachedPages`` pages), for a view that
    /// is about to show them. Pages already cached and current are not requested again.
    public func prefetch(_ range: Range<Int>) {
        engine.prefetch(range)
    }

    /// Applies the signal's value (change-set op 0, a `LazyValue`): the page server's handle, the length and the version. A new
    /// handle (a restart, a restored snapshot) drops the cache. Generated stores call it; it reads the whole of `reader`.
    ///
    /// - Throws: `WireError` for a value that does not decode, ``UndraLazyListError/nullHandle`` for the null handle. The list is
    ///   left as it was.
    public func applyFull(_ reader: inout UndraReader) throws {
        try engine.applyFull(&reader)
    }

    /// Applies the signal's invalidation (change-set op 2, a `LazyInvalidated`): the new length and version are taken at once, the
    /// rows cached stay visible (stale) and the pages of the window are requested again. An invalidation that is not newer than
    /// what the list knows (the one that follows a page that already raised the version) changes nothing. Generated stores call it.
    ///
    /// - Throws: `WireError` for a value that does not decode. The list is left as it was.
    public func applyInvalidated(_ reader: inout UndraReader) throws {
        try engine.applyInvalidated(&reader)
    }

    private func engineChanged(_ change: UndraLazyListChange) {
        // One `objectWillChange` per change: a new length already tells the views, so the revision is bumped only for pages alone.
        if change.contains(.count) {
            count = engine.count
        } else if change.contains(.pages) {
            revision &+= 1
        }
    }

    /// The engine, for tests.
    var testEngine: UndraLazyListEngine<Item> {
        return engine
    }
}

extension UndraLazyListObject: @preconcurrency RandomAccessCollection {
    /// The row at an index, `nil` while it loads.
    public typealias Element = Item?
    /// A row number.
    public typealias Index = Int

    /// Always 0.
    public var startIndex: Int {
        return 0
    }

    /// The number of rows.
    public var endIndex: Int {
        return count
    }

    /// The index after `i`.
    public func index(after i: Int) -> Int {
        return i + 1
    }

    /// The index before `i`.
    public func index(before i: Int) -> Int {
        return i - 1
    }
}
