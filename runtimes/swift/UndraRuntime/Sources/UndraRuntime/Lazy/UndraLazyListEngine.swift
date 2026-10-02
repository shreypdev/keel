// The engine under `UndraLazyList` and `UndraLazyListObject` (ADR-043 decision 3.5).
//
// Everything a lazy list does that is not "tell SwiftUI" lives here, once: the page cache, the requests (coalesced into one batch
// per main-actor turn, deduplicated while in flight), the version rules, the window that an invalidation re-pages, and the
// checks that keep a hostile reply from doing any harm. The two public classes are thin: they own an engine, mirror its `count`
// into an observable property and bump a revision when pages change. Reading never crosses the boundary (docs/SPEC.md R5): a
// read returns what is cached and only *queues* a request, which a later main-actor turn sends.

import Foundation

/// Why a page of a lazy list, or the value of its signal, could not be used (ADR-043). Reported through `UndraCore.report`
/// (``LoadOptions/onError``), never thrown at a reader: a row whose page failed stays `nil` and is asked for again the next time
/// it is read.
public enum UndraLazyListError: Error, Sendable, Equatable {
    /// The value of a `Lazy<T>` signal carries the null handle: there is no page server to ask.
    case nullHandle
    /// A page reply claims more items than the request's `limit`.
    case pageTooLarge(page: Int, count: UInt32, limit: Int)
    /// A page reply carries a number of items that its `total` does not allow: a page at `offset` with `limit` holds exactly
    /// `min(limit, max(0, total - offset))` items.
    case pageCountMismatch(page: Int, count: UInt32, expected: Int)
    /// A page reply read at the list's current version reports another total than the list's length.
    case totalMismatch(page: Int, total: UInt32, expected: Int, version: UInt64)
    /// A page reply that does not decode: truncated, trailing bytes, or an item that is not a valid `Item`.
    case malformedPage(page: Int, reason: String)
    /// The core answered a page again and again with a version older than the one it announced.
    case staleReplies(page: Int, got: UInt64, expected: UInt64)
}

extension UndraLazyListError: CustomStringConvertible {
    /// A one-line description that names the page and what is wrong with it.
    public var description: String {
        switch self {
        case .nullHandle:
            return "a lazy list's value carries the null handle, so there is no page server to ask"
        case .pageTooLarge(let page, let count, let limit):
            return "page \(page) of a lazy list holds \(count) items, more than the \(limit) that were asked for"
        case .pageCountMismatch(let page, let count, let expected):
            return "page \(page) of a lazy list holds \(count) items, but its total says \(expected)"
        case .totalMismatch(let page, let total, let expected, let version):
            return "page \(page) of a lazy list was read at version \(version) with total \(total), but the list holds \(expected) rows at that version"
        case .malformedPage(let page, let reason):
            return "page \(page) of a lazy list does not decode: \(reason)"
        case .staleReplies(let page, let got, let expected):
            return "the core keeps answering page \(page) of a lazy list at version \(got), older than the announced \(expected)"
        }
    }
}

/// What changed in an engine since the observable shell last heard: `count` (the length) and/or `pages` (rows were cached,
/// replaced or dropped).
struct UndraLazyListChange: OptionSet, Sendable {
    let rawValue: UInt8

    static let count = UndraLazyListChange(rawValue: 1)
    static let pages = UndraLazyListChange(rawValue: 2)
}

/// Runs `work` once, on the main actor, after the current turn: how an engine coalesces the requests of a turn into one batch.
typealias UndraLazyListSchedule = @MainActor (@escaping @MainActor @Sendable () -> Void) -> Void

/// The page cache and the paging logic of one lazy list. Main actor only.
@MainActor
final class UndraLazyListEngine<Item: UndraCodec & Sendable> {
    // MARK: Limits

    /// The default number of rows in a page.
    static var defaultPageSize: Int { return 50 }
    /// The default bound of the page cache.
    static var defaultMaxCachedPages: Int { return 24 }
    /// The largest page a list asks for: the wire's `limit` is a `u32`, and a page this big is already a mistake.
    static var largestPageSize: Int { return 1 << 20 }
    /// How often a page is asked for again after replies older than the list's version, before the failure is reported.
    static var maxStaleRetries: Int { return 3 }

    // MARK: State

    /// One cached page.
    private struct CachedPage {
        var rows: [Item]
        /// The list's version when the rows were read: older than the list's current one means stale (still shown, asked for again
        /// the next time it is read).
        var version: UInt64
    }

    private let core: UndraCore
    private let schedule: UndraLazyListSchedule
    /// Told once per operation that changed something (the observable shell's hook).
    var onChange: (@MainActor (UndraLazyListChange) -> Void)?

    /// The page server of the core, `nil` until the signal's first value.
    private(set) var handle: UndraHandle?
    /// The number of rows at `version`.
    private(set) var count = 0
    /// The newest version the engine knows (from the signal or from a page).
    private(set) var version: UInt64 = 0

    private var pageSizeValue = UndraLazyListEngine.defaultPageSize
    private var maxCachedPagesValue = UndraLazyListEngine.defaultMaxCachedPages

    private var pages: [Int: CachedPage] = [:]
    /// Pages cached or asked for, with the tick of their last touch (a read, a prefetch, a prefetch of a neighbour).
    private var touched: [Int: UInt64] = [:]
    /// Pages that a flush has yet to ask for.
    private var queued: Set<Int> = []
    /// Pages whose reply has not come, with the epoch they were asked in.
    private var inFlight: [Int: UInt64] = [:]
    private var staleAttempts: [Int: Int] = [:]
    private var tick: UInt64 = 0
    /// The tick at the last invalidation: the pages touched after it are the window.
    private var windowFloor: UInt64 = 0
    /// Bumped when what was asked for is void (a restart, another page size): replies of an older epoch are dropped.
    private var epoch: UInt64 = 0
    private var flushScheduled = false
    /// Greater than 0 while a flush runs: what its pages and any change-set applied by a call's drain change is told once, at the end.
    private var batching = 0
    private var pending: UndraLazyListChange = []

    init(core: UndraCore, schedule: UndraLazyListSchedule? = nil) {
        self.core = core
        self.schedule = schedule ?? { work in
            Task { @MainActor in
                work()
            }
        }
    }

    // MARK: Configuration

    /// The number of rows in a page. Changing it drops the cache (page boundaries move). Between 1 and 1,048,576.
    var pageSize: Int {
        get {
            return pageSizeValue
        }
        set {
            let clamped = Swift.max(1, Swift.min(newValue, UndraLazyListEngine.largestPageSize))
            if clamped == pageSizeValue {
                return
            }
            pageSizeValue = clamped
            dropCache()
            commit()
        }
    }

    /// The most pages kept. At least 1.
    var maxCachedPages: Int {
        get {
            return maxCachedPagesValue
        }
        set {
            maxCachedPagesValue = Swift.max(1, newValue)
            evict(keeping: nil)
            commit()
        }
    }

    // MARK: Reading

    /// The row at `index`, or `nil` while its page loads. Requests the page that holds it and one page on each side (queued: the
    /// requests of one turn are sent together by a later flush). An index outside `0..<count` returns `nil` and requests nothing.
    func item(at index: Int) -> Item? {
        guard handle != nil, index >= 0, index < count else {
            return nil
        }
        let page = index / pageSizeValue
        let last = pageCount - 1
        if page > 0 {
            want(page - 1)
        }
        if page < last {
            want(page + 1)
        }
        want(page)
        scheduleFlushIfNeeded()
        guard let cached = pages[page] else {
            return nil
        }
        let row = index - page * pageSizeValue
        return row < cached.rows.count ? cached.rows[row] : nil
    }

    /// Requests the pages that hold `range` (the part inside `0..<count`; at most `maxCachedPages` pages of it).
    func prefetch(_ range: Range<Int>) {
        guard handle != nil, count > 0 else {
            return
        }
        let lower = Swift.max(range.lowerBound, 0)
        let upper = Swift.min(range.upperBound, count)
        guard lower < upper else {
            return
        }
        let first = lower / pageSizeValue
        let last = (upper - 1) / pageSizeValue
        let end = Swift.min(last, first + maxCachedPagesValue - 1)
        var page = first
        while page <= end {
            want(page)
            page += 1
        }
        scheduleFlushIfNeeded()
    }

    // MARK: Applying the signal

    /// The signal's value (op 0): a new page server restarts the list (the cache is dropped), the same one is a refresh.
    func applyFull(_ reader: inout UndraReader) throws {
        let value = try UndraLazyValue.undraDecode(&reader)
        try reader.finish()
        if value.handle.isNull {
            throw UndraLazyListError.nullHandle
        }
        if value.handle == handle {
            adopt(length: Int(value.len), version: value.version)
        } else {
            restart(handle: value.handle, length: Int(value.len), version: value.version)
        }
        commit()
    }

    /// The signal's invalidation (op 2): the new length and version are taken at once, the cached rows stay (stale) while the
    /// window is asked for again.
    func applyInvalidated(_ reader: inout UndraReader) throws {
        let value = try UndraLazyInvalidated.undraDecode(&reader)
        try reader.finish()
        if handle != nil {
            adopt(length: Int(value.len), version: value.version)
        }
        commit()
    }

    // MARK: The version rules

    private var pageCount: Int {
        return (count + pageSizeValue - 1) / pageSizeValue
    }

    private func dropCache() {
        epoch &+= 1
        if !pages.isEmpty || !inFlight.isEmpty {
            pending.insert(.pages)
        }
        pages.removeAll()
        touched.removeAll()
        queued.removeAll()
        inFlight.removeAll()
        staleAttempts.removeAll()
        windowFloor = tick
    }

    private func restart(handle newHandle: UndraHandle, length: Int, version newVersion: UInt64) {
        dropCache()
        handle = newHandle
        version = newVersion
        if length != count {
            count = length
            pending.insert(.count)
        }
    }

    /// Takes a length and a version that are newer than what the engine knows (an older or equal one changes nothing: the op 2
    /// that follows a page that already raised the version is one): the cached pages become stale and the window is queued.
    private func adopt(length: Int, version newVersion: UInt64) {
        guard newVersion > version else {
            return
        }
        version = newVersion
        if length != count {
            count = length
            pending.insert(.count)
        }
        let live = pageCount
        for page in Array(pages.keys) where page >= live {
            pages[page] = nil
            touched[page] = nil
            pending.insert(.pages)
        }
        queued = queued.filter { $0 < live }
        for page in Array(touched.keys) where page >= live {
            touched[page] = nil
        }
        // The window: what was touched since the previous invalidation. A page already on its way is not asked for again: its
        // reply is older than `version` and will be asked for again when it comes.
        for (page, at) in touched where at > windowFloor && page < live && inFlight[page] == nil {
            queued.insert(page)
        }
        windowFloor = tick
        scheduleFlushIfNeeded()
    }

    // MARK: Requests

    /// Touches `page` and queues a request unless it is cached and current, queued or on its way.
    private func want(_ page: Int) {
        tick &+= 1
        touched[page] = tick
        if let cached = pages[page], cached.version >= version {
            return
        }
        if inFlight[page] != nil {
            return
        }
        queued.insert(page)
    }

    private func scheduleFlushIfNeeded() {
        if queued.isEmpty || flushScheduled {
            return
        }
        flushScheduled = true
        schedule { [weak self] in
            self?.flush()
        }
    }

    /// Whether a flush is waiting for its turn (tests).
    var hasScheduledFlush: Bool {
        return flushScheduled
    }

    /// Sends the queued page requests: the whole batch of a turn, in page order. Synchronously (`callSync`) where the transport
    /// runs a call inline, else one task per page.
    func flush() {
        flushScheduled = false
        batching += 1
        defer {
            batching -= 1
            commit()
        }
        guard let target = handle else {
            queued.removeAll()
            return
        }
        var batch = queued.filter { needsRequest($0) }
        queued.removeAll()
        if batch.count > maxCachedPagesValue {
            // More pages than the cache holds are no use: the most recently touched ones.
            let newest = batch.sorted { (touched[$0] ?? 0) > (touched[$1] ?? 0) }.prefix(maxCachedPagesValue)
            for page in batch.subtracting(newest) {
                touched[page] = nil
            }
            batch = Set(newest)
        }
        let started = epoch
        let direct = core.transport.supportsDirectSync
        for page in batch.sorted() {
            // A call made before may have changed things (a change-set applied by its drain): look again.
            guard epoch == started, handle == target, needsRequest(page) else {
                continue
            }
            if direct {
                loadSynchronously(page, from: target)
            } else {
                loadAsynchronously(page, from: target)
            }
        }
    }

    /// Whether `page` is still to be asked for: inside the list, not on its way, not cached at the current version (an invalidation
    /// applied by the drain of an earlier call of the batch may have queued a page that the batch itself has since loaded).
    private func needsRequest(_ page: Int) -> Bool {
        if page >= pageCount || inFlight[page] != nil {
            return false
        }
        if let cached = pages[page], cached.version >= version {
            return false
        }
        return true
    }

    private func loadSynchronously(_ page: Int, from server: UndraHandle) {
        let offset = page * pageSizeValue
        let limit = pageSizeValue
        let asked = epoch
        inFlight[page] = asked
        let body: [UInt8]
        do {
            body = try core.callSync(
                .lazyListPage(handle: server, offset: UInt32(clamping: offset), limit: UInt32(clamping: limit)),
                method: 0,
                args: []
            )
        } catch {
            failed(page, asked, error)
            return
        }
        received(page, offset: offset, limit: limit, epoch: asked, body: body)
    }

    private func loadAsynchronously(_ page: Int, from server: UndraHandle) {
        let offset = page * pageSizeValue
        let limit = pageSizeValue
        let asked = epoch
        inFlight[page] = asked
        let core = self.core
        let target = CallTarget.lazyListPage(handle: server, offset: UInt32(clamping: offset), limit: UInt32(clamping: limit))
        Task { @MainActor [weak self] in
            let body: [UInt8]
            do {
                body = try await core.call(target, method: 0, args: [])
            } catch {
                self?.failed(page, asked, error)
                return
            }
            self?.received(page, offset: offset, limit: limit, epoch: asked, body: body)
        }
    }

    private func failed(_ page: Int, _ asked: UInt64, _ error: any Error) {
        if inFlight[page] == asked {
            inFlight[page] = nil
        }
        if error is CancellationError || epoch != asked {
            return
        }
        core.report(error, operation: "UndraLazyList.page(\(page))")
    }

    private func received(_ page: Int, offset: Int, limit: Int, epoch asked: UInt64, body: [UInt8]) {
        defer {
            if inFlight[page] == asked {
                inFlight[page] = nil
            }
            commit()
        }
        guard epoch == asked else {
            return
        }
        do {
            try accept(page, offset: offset, limit: limit, body: body)
        } catch {
            core.report(error, operation: "UndraLazyList.page(\(page))")
        }
    }

    /// Checks a page reply and files it: the hostile cases are errors, an older version is asked for again, a newer one raises the
    /// list's version and length (and so makes the cached pages stale).
    private func accept(_ page: Int, offset: Int, limit: Int, body: [UInt8]) throws {
        var reader = UndraReader(body)
        let header: UndraLazyPageHeader
        do {
            header = try UndraLazyPageHeader.undraDecode(&reader)
        } catch {
            throw UndraLazyListError.malformedPage(page: page, reason: String(describing: error))
        }
        let announced = Int(header.count)
        guard announced <= limit else {
            throw UndraLazyListError.pageTooLarge(page: page, count: header.count, limit: limit)
        }
        let expected = Swift.min(limit, Swift.max(0, Int(header.total) - offset))
        guard announced == expected else {
            throw UndraLazyListError.pageCountMismatch(page: page, count: header.count, expected: expected)
        }
        if header.version < version {
            let attempts = (staleAttempts[page] ?? 0) + 1
            staleAttempts[page] = attempts
            if attempts > UndraLazyListEngine.maxStaleRetries {
                throw UndraLazyListError.staleReplies(page: page, got: header.version, expected: version)
            }
            queued.insert(page)
            scheduleFlushIfNeeded()
            return
        }
        guard header.version > version || Int(header.total) == count else {
            throw UndraLazyListError.totalMismatch(page: page, total: header.total, expected: count, version: header.version)
        }
        var rows: [Item] = []
        rows.reserveCapacity(expected)
        do {
            var read = 0
            while read < expected {
                rows.append(try Item.undraDecode(&reader))
                read += 1
            }
            try reader.finish()
        } catch {
            throw UndraLazyListError.malformedPage(page: page, reason: String(describing: error))
        }
        if header.version > version {
            adopt(length: Int(header.total), version: header.version)
        }
        guard page < pageCount else {
            return
        }
        pages[page] = CachedPage(rows: rows, version: header.version)
        if touched[page] == nil {
            tick &+= 1
            touched[page] = tick
        }
        staleAttempts[page] = nil
        pending.insert(.pages)
        evict(keeping: page)
    }

    // MARK: The cache bound

    /// Drops the least recently touched pages until the cache fits: pages outside the window first, then the oldest of the window
    /// (the window itself is bounded by the cache, so it cannot grow past it).
    private func evict(keeping keep: Int?) {
        while pages.count > maxCachedPagesValue {
            var victim: Int?
            var victimRank: (inWindow: Int, tick: UInt64) = (Int.max, UInt64.max)
            for page in pages.keys where page != keep {
                let at = touched[page] ?? 0
                let rank = (inWindow: at > windowFloor ? 1 : 0, tick: at)
                if rank.inWindow < victimRank.inWindow || (rank.inWindow == victimRank.inWindow && rank.tick < victimRank.tick) {
                    victim = page
                    victimRank = rank
                }
            }
            guard let evicted = victim else {
                return
            }
            pages[evicted] = nil
            touched[evicted] = nil
            pending.insert(.pages)
        }
    }

    private func commit() {
        if pending.isEmpty || batching > 0 {
            return
        }
        let change = pending
        pending = []
        onChange?(change)
    }

    // MARK: Inspection (tests)

    /// The cached pages, ascending.
    var cachedPages: [Int] {
        return pages.keys.sorted()
    }

    /// The pages whose reply is awaited, ascending.
    var inFlightPages: [Int] {
        return inFlight.keys.sorted()
    }

    /// The pages touched since the last invalidation and still cached or asked for, ascending.
    var windowPages: [Int] {
        return touched.filter { $0.value > windowFloor }.keys.sorted()
    }

    /// The version a cached page was read at.
    func versionOfCachedPage(_ page: Int) -> UInt64? {
        return pages[page]?.version
    }
}
