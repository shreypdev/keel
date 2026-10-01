// The mirror: the host side of change-set delivery (docs/SPEC.md sections 5.5 and 11, ADR-031).
//
// The core reports state changes as change-sets (`Wire.ChangeSet`) through a callback that can
// run on any thread and must not call back into the core. The mirror parses each change-set's
// entry table on that thread, queues the entries and applies the queue on the main actor, where
// the stores that own the observable state live.
//
// A *drain* applies the queue. It folds the queued entries per signal `(handle, signal)` in
// arrival order without decoding any value: a full value (op 0) or a lazy invalidation (op 2)
// supersedes everything queued earlier for the signal, and consecutive keyed patches (op 1)
// become one patch (the counts add up, the ops follow each other: SPEC 3.8 applies ops one after
// the other, each index relative to the list the previous op left, so the concatenation is the
// same change). Each signal is therefore applied at most twice per drain, its last full value and
// then its merged patch, signals in the order of their first entry. Signals a store declared
// `no_coalesce` are applied entry by entry instead.
//
// Drains are frame-aligned (`FrameScheduler`): what the core produces on its own waits for the
// next display frame. Replies, synchronous calls made on the main thread and `observe` drain
// immediately (see `UndraCore`), so code that awaits a call sees the call's effects.
//
// The queue is bounded: past `maxPendingEntries` or `maxPendingBytes` the enqueuing thread folds
// it in place. A signal whose merged patch grows past either patch bound is dropped there and
// re-observed by the next drain, so memory stays proportional to the observed signals however far
// the main thread falls behind.

import Dispatch
import Foundation

/// Applies one change-set entry to the object registered for its handle: the signal id, how to
/// interpret the value, and a reader restricted to the entry's value bytes.
///
/// Registered handlers run on the main actor.
public typealias MirrorApply = @MainActor @Sendable (UInt32, ChangeOp, inout UndraReader) -> Void

/// The registry that routes change-set entries to the stores mirroring the core's signals, and
/// the queue that turns the core's change-sets into one merged drain per frame.
///
/// Generated stores never talk to it directly: `UndraStore.init(core:handle:noCoalesce:)`
/// registers the store and `UndraObject.close()` unregisters it. Registration, unregistration,
/// the counters and the drain listeners are safe from any thread; applying (`flush()`) is
/// main-actor only.
///
/// A drain applies, for each signal, its last full value and then the keyed patches that followed
/// it merged into one, so a store sees the state after every change-set the drain consumed but not
/// the states in between (SwiftUI renders once per frame in any case). Signals registered as
/// `noCoalesce` see every entry.
public final class Mirror: @unchecked Sendable {
    // MARK: Limits

    /// Default of `LoadOptions.maxPendingEntries`.
    static let defaultMaxPendingEntries = 65_536
    /// Default of `LoadOptions.maxPendingBytes`.
    static let defaultMaxPendingBytes = 16 * 1024 * 1024
    /// A merged patch with more operations than this, **or** more op bytes than
    /// `maxMergedPatchBytes`, is dropped by a compaction and its signal re-observed: the bytes
    /// bound what the backlog holds per signal, the operations what a drain replays for it (the
    /// core's own op log stops at 4,096 too).
    static let maxMergedPatchOps = 4096
    /// See `maxMergedPatchOps`.
    static let maxMergedPatchBytes = 1024 * 1024
    /// Rounds one drain runs before it leaves the rest to the next one: a store whose apply keeps
    /// causing changes to a store it observes would otherwise hold the main actor forever. The
    /// same bound as the core's commit (docs/SPEC.md section 16.1).
    static let maxRounds = 1000
    /// What a queued entry costs besides its value: the wire's fixed part (handle, signal id, op,
    /// length).
    static let entryOverhead = 17

    // MARK: Queue model

    /// One signal of one store.
    struct Key: Hashable {
        var handle: UInt64
        var signal: UInt32
    }

    /// One parsed change-set entry. `value` shares the storage of the change-set's payload until a
    /// compaction copies it.
    struct Entry {
        var handle: UInt64
        var signal: UInt32
        var op: ChangeOp
        var value: ArraySlice<UInt8>

        var key: Key {
            return Key(handle: handle, signal: signal)
        }
    }

    /// A store's registration: its apply function and its `no_coalesce` signals.
    struct Registration {
        let apply: MirrorApply
        let noCoalesce: Set<UInt32>
    }

    typealias ResyncHandler = @MainActor @Sendable (UndraHandle, UInt32) -> Void
    typealias DrainListener = @MainActor @Sendable (DrainStats) -> Void

    private struct State {
        var registrations: [UInt64: Registration] = [:]
        /// Entries waiting for a drain, in arrival order.
        var queue: [Entry] = []
        /// What `queue` accounts for: `entryOverhead + value.count` per entry.
        var queueBytes = 0
        /// Change-sets and entries received since a drain last took the queue.
        var queuedChangeSets = 0
        var queuedEntries = 0
        /// The queue is folded when it passes these (the bounds, or twice the size of the queue
        /// a compaction left, whichever is larger).
        var compactAtEntries: Int
        var compactAtBytes: Int
        /// Signals whose keyed patches are discarded until a full value arrives: their pending
        /// content was dropped, so later patches are relative to a list this host never had.
        /// `true` while the re-observe is still to be sent.
        var awaiting: [Key: Bool] = [:]
        /// Some signal in `awaiting` is to be re-observed by the next drain.
        var resyncDue = false
        var resync: ResyncHandler?
        /// A frame was requested from the scheduler and has not ticked yet.
        var frameRequested = false
        /// An immediate drain (for a reply) is queued on the main queue and has not started yet.
        var immediateScheduled = false
        var flushing = false
        /// Synchronous calls in progress on the main thread (`withImmediateDrain`): what the core
        /// delivers on the main thread meanwhile is drained when the call returns, not at a frame.
        var mainThreadCalls = 0
        /// Something was queued on the main thread while draining (by the drain itself: a store
        /// re-observing, a resync answered in process), so the drain runs another round.
        var moreRounds = false
        var listeners: [(id: UInt64, call: DrainListener)] = []
        var nextListenerId: UInt64 = 0

        var changeSetsReceived = 0
        var entriesReceived = 0
        var entriesApplied = 0
        var drains = 0
        var compactions = 0
        var resyncs = 0
        var droppedEntries = 0

        init(maxPendingEntries: Int, maxPendingBytes: Int) {
            compactAtEntries = maxPendingEntries
            compactAtBytes = maxPendingBytes
        }
    }

    private let state: Guarded<State>
    private let scheduler: any FrameScheduler
    private let maxPendingEntries: Int
    private let maxPendingBytes: Int

    /// A mirror whose queue is folded past `maxPendingEntries` entries or `maxPendingBytes` bytes
    /// (each at least 1), draining at the frames `scheduler` gives (the platform's by default).
    init(
        maxPendingEntries: Int = Mirror.defaultMaxPendingEntries,
        maxPendingBytes: Int = Mirror.defaultMaxPendingBytes,
        scheduler: (any FrameScheduler)? = nil
    ) {
        self.maxPendingEntries = Swift.max(1, maxPendingEntries)
        self.maxPendingBytes = Swift.max(1, maxPendingBytes)
        self.scheduler = scheduler ?? makePlatformFrameScheduler()
        self.state = Guarded<State>(State(maxPendingEntries: self.maxPendingEntries, maxPendingBytes: self.maxPendingBytes))
    }

    deinit {
        scheduler.invalidate()
    }

    // MARK: Registration

    /// Registers `apply` for `handle`, replacing any earlier registration of the same handle.
    ///
    /// `noCoalesce` lists the store's signals declared `#[undra(no_coalesce)]` (generated stores
    /// pass them): a drain applies every entry of those, in order, at its arrival position,
    /// instead of folding them into the last value. The mirror applies every one; whether a view
    /// shows each is up to the UI (SwiftUI renders once per frame whatever the model does). A
    /// queue folded because it passed its bound folds these signals too: the bound wins.
    public func register(_ handle: UndraHandle, noCoalesce: Set<UInt32> = [], _ apply: @escaping MirrorApply) {
        let registration = Registration(apply: apply, noCoalesce: noCoalesce)
        state.withLock { (current: inout State) -> Void in
            current.registrations[handle.rawValue] = registration
        }
    }

    /// Removes the registration of `handle`. Entries that arrive for it afterwards are dropped
    /// (and counted in `MirrorStats.droppedEntries`). Unknown handles are ignored.
    public func unregister(_ handle: UndraHandle) {
        let raw = handle.rawValue
        state.withLock { (current: inout State) -> Void in
            current.registrations[raw] = nil
            if !current.awaiting.isEmpty {
                current.awaiting = current.awaiting.filter { $0.key.handle != raw }
            }
        }
    }

    /// The number of registered handles.
    public var registeredCount: Int {
        return state.withLock { (current: inout State) -> Int in
            return current.registrations.count
        }
    }

    /// The number of entries received but not yet applied (`stats().pendingEntries`).
    public var pendingCount: Int {
        return state.withLock { (current: inout State) -> Int in
            return current.queue.count
        }
    }

    // MARK: Counters and listeners

    /// The mirror's counters (see `MirrorStats`). Safe from any thread.
    public func stats() -> MirrorStats {
        return state.withLock { (current: inout State) -> MirrorStats in
            return MirrorStats(
                changeSetsReceived: current.changeSetsReceived,
                entriesReceived: current.entriesReceived,
                entriesApplied: current.entriesApplied,
                drains: current.drains,
                compactions: current.compactions,
                resyncs: current.resyncs,
                pendingEntries: current.queue.count,
                pendingBytes: current.queueBytes,
                droppedEntries: current.droppedEntries
            )
        }
    }

    /// Calls `listener` on the main actor after every drain with what the drain did. Drains are
    /// timed (`ContinuousClock`) only while a listener is registered. Safe from any thread.
    ///
    /// ```swift
    /// let registration = core.mirror.addDrainListener { stats in
    ///     print("\(stats.changeSets) change-sets, \(stats.appliedEntries) applies in \(stats.duration)")
    /// }
    /// registration.remove()
    /// ```
    @discardableResult
    public func addDrainListener(_ listener: @escaping @MainActor @Sendable (DrainStats) -> Void) -> DrainListenerRegistration {
        let id = state.withLock { (current: inout State) -> UInt64 in
            current.nextListenerId += 1
            current.listeners.append((id: current.nextListenerId, call: listener))
            return current.nextListenerId
        }
        return DrainListenerRegistration { [weak self] in
            self?.removeDrainListener(id)
        }
    }

    private func removeDrainListener(_ id: UInt64) {
        state.withLock { (current: inout State) -> Void in
            current.listeners.removeAll { $0.id == id }
        }
    }

    /// Installs what a drain calls to re-observe a signal whose pending content was dropped
    /// (`UndraCore` sends `Observe` on). Without one such a signal waits for the core's next full
    /// value.
    func setResyncHandler(_ handler: @escaping ResyncHandler) {
        state.withLock { (current: inout State) -> Void in
            current.resync = handler
        }
    }

    /// Releases the frame scheduler's resources (the display link). Later drains still happen.
    func invalidateScheduler() {
        scheduler.invalidate()
    }

    // MARK: Delivery from the core

    /// Queues the entries of one change-set payload (already copied out of the callback's
    /// buffer) and makes sure a frame drain is coming. Safe from any thread, including a core
    /// callback; it never calls into the core.
    ///
    /// The entry table is parsed here, once; a malformed change-set is logged and dropped whole
    /// (a change-set is a transaction). Keyed patches of a signal waiting for a full value are
    /// discarded. When the queue passes its bound it is folded in place, on this thread.
    func enqueue(_ payload: [UInt8]) {
        var parsed: [Entry] = []
        do {
            try Wire.ChangeSet.forEachEntry(slice: payload[...]) { handle, signal, op, value in
                parsed.append(Entry(handle: handle.rawValue, signal: signal, op: op, value: value.readRemaining()))
            }
        } catch {
            UndraLog.error("dropped a malformed change-set: \(error)")
            return
        }
        let onMainThread = Thread.isMainThread
        let maxEntries = maxPendingEntries
        let maxBytes = maxPendingBytes
        let outcome = state.withLock { (current: inout State) -> (requestFrame: Bool, unmergeable: [Key]) in
            current.changeSetsReceived += 1
            current.entriesReceived += parsed.count
            current.queuedChangeSets += 1
            current.queuedEntries += parsed.count
            for entry in parsed {
                if !current.awaiting.isEmpty, current.awaiting[entry.key] != nil {
                    if entry.op == .keyedPatch {
                        continue
                    }
                    current.awaiting[entry.key] = nil
                }
                current.queue.append(entry)
                current.queueBytes += Mirror.entryOverhead + entry.value.count
            }
            var unmergeable: [Key] = []
            if current.queue.count > current.compactAtEntries || current.queueBytes > current.compactAtBytes {
                unmergeable = Mirror.compact(&current, maxEntries: maxEntries, maxBytes: maxBytes)
            }
            if current.queue.isEmpty && !current.resyncDue {
                return (false, unmergeable)
            }
            if onMainThread && current.flushing {
                // Queued by the drain that is running (a store re-observing from inside apply):
                // it applies this in another round.
                current.moreRounds = true
                return (false, unmergeable)
            }
            if onMainThread && current.mainThreadCalls > 0 {
                // Delivered by a synchronous call on the main thread, which drains when it returns.
                return (false, unmergeable)
            }
            if current.frameRequested {
                return (false, unmergeable)
            }
            current.frameRequested = true
            return (true, unmergeable)
        }
        Mirror.logUnmergeable(outcome.unmergeable)
        if outcome.requestFrame {
            requestFrame()
        }
    }

    private func requestFrame() {
        scheduler.requestFrame { [weak self] in
            self?.frameTick()
        }
    }

    /// The frame the mirror asked for has come: drain.
    @MainActor
    func frameTick() {
        state.withLock { (current: inout State) -> Void in
            current.frameRequested = false
        }
        flush()
    }

    /// Queues an immediate drain on the main queue when anything waits: what `UndraCore` does
    /// before it resumes a call's continuation, so a caller on the main actor (resumed after this
    /// job, FIFO) sees the change-sets that arrived before the reply. Safe from any thread.
    func drainBeforeResuming() {
        let wanted = state.withLock { (current: inout State) -> Bool in
            if current.immediateScheduled || (current.queue.isEmpty && !current.resyncDue) {
                return false
            }
            current.immediateScheduled = true
            return true
        }
        if !wanted {
            return
        }
        DispatchQueue.main.async { [self] in
            MainActor.assumeIsolated {
                self.state.withLock { (current: inout State) -> Void in
                    current.immediateScheduled = false
                }
                self.flush()
            }
        }
    }

    /// Runs `body`, a synchronous call into the core, and, when called on the main thread,
    /// drains before returning or throwing: what `callSync`, `construct` and `observe` do, so the
    /// code after them sees what the core delivered meanwhile (read-your-writes). Inside a drain
    /// (a store calling from `apply`) the running drain applies it in its next round instead.
    /// Elsewhere it only runs `body`.
    func withImmediateDrain<Output>(_ body: () throws -> Output) rethrows -> Output {
        if !Thread.isMainThread {
            return try body()
        }
        state.withLock { (current: inout State) -> Void in
            current.mainThreadCalls += 1
        }
        defer {
            state.withLock { (current: inout State) -> Void in
                current.mainThreadCalls -= 1
            }
            MainActor.assumeIsolated {
                self.flush()
            }
        }
        return try body()
    }

    // MARK: Draining

    /// What one round of a drain takes from the queue.
    private struct Round {
        var entries: [Entry]
        var registrations: [UInt64: Registration]
        var changeSets: Int
        var entryCount: Int
    }

    /// After a round: another one, or the end of the drain.
    private enum DrainStep {
        case next(Round)
        case done(listeners: [DrainListener], requestFrame: Bool, capped: Bool)
    }

    /// Drains now, on the main actor: folds the queued entries per signal and applies them (see
    /// the type's documentation), re-observes the signals a compaction dropped, then reports to
    /// the drain listeners.
    ///
    /// The runtime calls it at each frame, before a call's continuation resumes, before a
    /// synchronous call made on the main thread returns, and from `UndraCore.observe` (so that a
    /// store has its initial values before its initializer returns). A nested call, made by a
    /// store from inside `apply`, returns at once: what it would have applied, the running drain
    /// applies in a further round. After 1000 rounds the rest is left to the next drain.
    @MainActor
    public func flush() {
        let maxEntries = maxPendingEntries
        let maxBytes = maxPendingBytes
        let start = state.withLock { (current: inout State) -> (round: Round, timed: Bool)? in
            if current.flushing || (current.queue.isEmpty && !current.resyncDue) {
                return nil
            }
            current.flushing = true
            return (round: Mirror.take(&current, maxEntries: maxEntries, maxBytes: maxBytes), timed: !current.listeners.isEmpty)
        }
        guard let start = start else {
            return
        }
        let clock = ContinuousClock()
        let started = start.timed ? clock.now : nil
        var round = start.round
        var rounds = 0
        var changeSets = 0
        var entries = 0
        var applied = 0
        var dropped = 0
        var finish: (listeners: [DrainListener], requestFrame: Bool, capped: Bool)
        while true {
            changeSets += round.changeSets
            entries += round.entryCount
            let result = drain(round)
            applied += result.applied
            dropped += result.dropped
            requestResyncs()
            rounds += 1
            let roundsSoFar = rounds
            let totals = (applied: applied, dropped: dropped)
            let step = state.withLock { (current: inout State) -> DrainStep in
                var capped = false
                if current.moreRounds || current.resyncDue {
                    if roundsSoFar < Mirror.maxRounds {
                        return .next(Mirror.take(&current, maxEntries: maxEntries, maxBytes: maxBytes))
                    }
                    capped = true
                }
                current.flushing = false
                current.moreRounds = false
                current.entriesApplied += totals.applied
                current.droppedEntries += totals.dropped
                current.drains += 1
                var request = false
                if (!current.queue.isEmpty || current.resyncDue) && !current.frameRequested {
                    // Left over by the round cap, or queued from another thread while draining.
                    current.frameRequested = true
                    request = true
                }
                let listeners = start.timed ? current.listeners.map { $0.call } : []
                return .done(listeners: listeners, requestFrame: request, capped: capped)
            }
            switch step {
            case .next(let next):
                round = next
                continue
            case .done(let listeners, let requestFrame, let capped):
                finish = (listeners: listeners, requestFrame: requestFrame, capped: capped)
            }
            break
        }
        if finish.capped {
            UndraLog.error("the mirror applied \(Mirror.maxRounds) rounds of change-sets in one drain: a store keeps causing changes to a store it observes from inside apply; the rest is applied at the next frame")
        }
        if finish.requestFrame {
            requestFrame()
        }
        if let started = started, !finish.listeners.isEmpty {
            let stats = DrainStats(changeSets: changeSets, entries: entries, appliedEntries: applied, duration: clock.now - started)
            for listener in finish.listeners {
                listener(stats)
            }
        }
    }

    /// Takes the queue for one round of a drain (with the lock held).
    private static func take(_ current: inout State, maxEntries: Int, maxBytes: Int) -> Round {
        let round = Round(
            entries: current.queue,
            registrations: current.registrations,
            changeSets: current.queuedChangeSets,
            entryCount: current.queuedEntries
        )
        current.queue = []
        current.queueBytes = 0
        current.queuedChangeSets = 0
        current.queuedEntries = 0
        current.compactAtEntries = maxEntries
        current.compactAtBytes = maxBytes
        current.moreRounds = false
        return round
    }

    /// Folds one round's entries and applies them. Returns how many apply calls ran and how many
    /// entries were dropped for want of a registration.
    @MainActor
    private func drain(_ round: Round) -> (applied: Int, dropped: Int) {
        if round.entries.isEmpty {
            return (applied: 0, dropped: 0)
        }
        let folded = Mirror.fold(round.entries, registrations: round.registrations)
        Mirror.logUnmergeable(folded.unmergeable)
        if !folded.waiting.isEmpty {
            state.withLock { (current: inout State) -> Void in
                Mirror.markAwaiting(folded.waiting, in: &current)
            }
        }
        var applied = 0
        var dropped = 0
        for unit in folded.units {
            switch unit {
            case .slot(let at):
                let slot = folded.slots[at]
                guard let registration = round.registrations[slot.key.handle] else {
                    dropped += slot.entries
                    continue
                }
                if let full = slot.full {
                    var reader = UndraReader(slice: full.value)
                    registration.apply(slot.key.signal, full.op, &reader)
                    applied += 1
                }
                if !slot.patches.isEmpty {
                    var reader = UndraReader(slice: slot.mergedPatch())
                    registration.apply(slot.key.signal, .keyedPatch, &reader)
                    applied += 1
                }
            case .single(let entry):
                guard let registration = round.registrations[entry.handle] else {
                    dropped += 1
                    continue
                }
                var reader = UndraReader(slice: entry.value)
                registration.apply(entry.signal, entry.op, &reader)
                applied += 1
            }
        }
        return (applied: applied, dropped: dropped)
    }

    /// Re-observes, once each, the signals whose pending content was dropped, from the drain (on
    /// the main actor, never from a core callback). In process the core answers synchronously and
    /// the drain applies the full value in its next round.
    @MainActor
    private func requestResyncs() {
        let work = state.withLock { (current: inout State) -> (keys: [Key], handler: ResyncHandler?) in
            if !current.resyncDue {
                return (keys: [], handler: nil)
            }
            current.resyncDue = false
            var due: [Key] = []
            var kept: [Key: Bool] = [:]
            for (key, isDue) in current.awaiting where current.registrations[key.handle] != nil {
                if isDue {
                    due.append(key)
                }
                kept[key] = false
            }
            current.awaiting = kept
            guard let handler = current.resync else {
                return (keys: [], handler: nil)
            }
            current.resyncs += due.count
            due.sort { ($0.handle, $0.signal) < ($1.handle, $1.signal) }
            return (keys: due, handler: handler)
        }
        guard let handler = work.handler else {
            return
        }
        for key in work.keys {
            handler(UndraHandle(rawValue: key.handle), key.signal)
        }
    }

    // MARK: Folding

    /// One signal as a drain or a compaction folds it.
    private struct Slot {
        let key: Key
        /// The last full value or lazy invalidation; everything that arrived before it is
        /// superseded.
        var full: Entry?
        /// The keyed patches that arrived after `full`, whole (count and ops), in arrival order.
        var patches: [ArraySlice<UInt8>] = []
        /// Sum of the patches' op counts.
        var ops = 0
        /// Sum of the patches' op bytes, counts excluded.
        var opBytes = 0
        /// Entries folded into this slot.
        var entries = 0

        init(key: Key) {
            self.key = key
        }

        mutating func setFull(_ entry: Entry) {
            full = entry
            if !patches.isEmpty {
                patches.removeAll()
            }
            ops = 0
            opBytes = 0
        }

        /// Appends a keyed patch; `false` if it cannot be merged (shorter than its 4-byte count,
        /// or the counts would no longer fit a `u32`).
        mutating func addPatch(_ value: ArraySlice<UInt8>) -> Bool {
            if value.count < 4 {
                return false
            }
            let at = value.startIndex
            let count = Int(value[at]) | Int(value[at + 1]) << 8 | Int(value[at + 2]) << 16 | Int(value[at + 3]) << 24
            if ops + count > Int(UInt32.max) {
                return false
            }
            patches.append(value)
            ops += count
            opBytes += value.count - 4
            return true
        }

        /// Forgets what the slot holds (its signal waits for a full value).
        mutating func clear() {
            full = nil
            patches.removeAll()
            ops = 0
            opBytes = 0
        }

        /// Whether the merged patch passes either patch bound a compaction keeps.
        var oversized: Bool {
            return ops > Mirror.maxMergedPatchOps || opBytes > Mirror.maxMergedPatchBytes
        }

        /// The patches as one keyed patch: the sum of the counts, then every patch's ops in
        /// arrival order (the same change, by SPEC 3.8). One patch is returned as it is.
        func mergedPatch() -> ArraySlice<UInt8> {
            if patches.count == 1 {
                return patches[0]
            }
            var out: [UInt8] = []
            out.reserveCapacity(4 + opBytes)
            let count = UInt32(truncatingIfNeeded: ops)
            out.append(UInt8(truncatingIfNeeded: count))
            out.append(UInt8(truncatingIfNeeded: count >> 8))
            out.append(UInt8(truncatingIfNeeded: count >> 16))
            out.append(UInt8(truncatingIfNeeded: count >> 24))
            for patch in patches {
                out.append(contentsOf: patch.dropFirst(4))
            }
            return out[...]
        }
    }

    /// What a drain applies, in order: a folded signal, or one entry of a `no_coalesce` signal.
    private enum Unit {
        case slot(Int)
        case single(Entry)
    }

    private struct Folded {
        var slots: [Slot] = []
        /// Slots and single entries in application order (drains only).
        var units: [Unit] = []
        /// Signals that had a keyed patch the fold could not merge, for the log.
        var unmergeable: [Key] = []
        /// Signals left waiting for a full value: an unmergeable patch dropped their content and
        /// no full value followed it.
        var waiting: Set<Key> = []
    }

    /// Folds `entries` (arrival order) per signal, one slot per signal in the order of its first
    /// entry. For a drain (`registrations` given), the entries of a `no_coalesce` signal stay
    /// single units in place; a compaction (`nil`) folds every signal.
    private static func fold(_ entries: [Entry], registrations: [UInt64: Registration]?) -> Folded {
        var folded = Folded()
        var index: [Key: Int] = [:]
        var lastHandle: UInt64?
        var lastNoCoalesce: Set<UInt32> = []
        for entry in entries {
            let key = entry.key
            if !folded.waiting.isEmpty, folded.waiting.contains(key) {
                // Relative to a list this host never had: wait for a full value.
                if entry.op == .keyedPatch {
                    continue
                }
                folded.waiting.remove(key)
            }
            if let registrations = registrations {
                if entry.handle != lastHandle {
                    lastHandle = entry.handle
                    lastNoCoalesce = registrations[entry.handle]?.noCoalesce ?? []
                }
                if !lastNoCoalesce.isEmpty, lastNoCoalesce.contains(entry.signal) {
                    folded.units.append(.single(entry))
                    continue
                }
            }
            let at: Int
            if let found = index[key] {
                at = found
            } else {
                at = folded.slots.count
                folded.slots.append(Slot(key: key))
                index[key] = at
                folded.units.append(.slot(at))
            }
            folded.slots[at].entries += 1
            if entry.op != .keyedPatch {
                folded.slots[at].setFull(entry)
            } else if !folded.slots[at].addPatch(entry.value) {
                folded.slots[at].clear()
                folded.waiting.insert(key)
                folded.unmergeable.append(key)
            }
        }
        return folded
    }

    /// Folds the queue in place (decision 3 of ADR-031), with the lock held, on the thread that
    /// passed the bound: every signal, `no_coalesce` ones included. A merged patch past either patch
    /// bound is dropped and its signal re-observed by the next drain; surviving values are copied
    /// so they no longer keep whole payloads alive. The next compaction waits until the queue
    /// doubles, so folding stays O(1) per entry. Returns the signals with unmergeable patches.
    private static func compact(_ current: inout State, maxEntries: Int, maxBytes: Int) -> [Key] {
        let folded = fold(current.queue, registrations: nil)
        var queue: [Entry] = []
        queue.reserveCapacity(folded.slots.count)
        var bytes = 0
        for slot in folded.slots {
            if folded.waiting.contains(slot.key) || slot.oversized {
                current.awaiting[slot.key] = true
                current.resyncDue = true
                continue
            }
            if let full = slot.full {
                let value = ArraySlice(Array(full.value))
                queue.append(Entry(handle: slot.key.handle, signal: slot.key.signal, op: full.op, value: value))
                bytes += entryOverhead + value.count
            }
            if !slot.patches.isEmpty {
                let merged = slot.mergedPatch()
                let value = slot.patches.count == 1 ? ArraySlice(Array(merged)) : merged
                queue.append(Entry(handle: slot.key.handle, signal: slot.key.signal, op: .keyedPatch, value: value))
                bytes += entryOverhead + value.count
            }
        }
        current.queue = queue
        current.queueBytes = bytes
        current.compactions += 1
        current.compactAtEntries = Swift.max(maxEntries, 2 * queue.count)
        current.compactAtBytes = Swift.max(maxBytes, 2 * bytes)
        return folded.unmergeable
    }

    /// Makes `keys`, whose content a drain dropped, wait for a full value (with the lock held):
    /// their keyed patches queued since the drain took the queue are discarded up to their next
    /// full value; a signal without one is re-observed by the next drain.
    private static func markAwaiting(_ keys: Set<Key>, in current: inout State) {
        var open = keys
        var kept: [Entry] = []
        kept.reserveCapacity(current.queue.count)
        var bytes = 0
        for entry in current.queue {
            if !open.isEmpty, open.contains(entry.key) {
                if entry.op == .keyedPatch {
                    continue
                }
                open.remove(entry.key)
            }
            kept.append(entry)
            bytes += entryOverhead + entry.value.count
        }
        current.queue = kept
        current.queueBytes = bytes
        for key in open {
            current.awaiting[key] = true
            current.resyncDue = true
        }
    }

    private static func logUnmergeable(_ keys: [Key]) {
        for key in keys {
            UndraLog.error("a keyed patch for signal \(key.signal) of handle \(UndraHandle(rawValue: key.handle)) cannot be merged (it is shorter than its operation count, or the merged count passes u32); the signal is re-observed")
        }
    }
}
