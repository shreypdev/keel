// What the mirror reports about delivery (ADR-031 decision 5, docs/SPEC.md section 17): its
// counters, and what each drain did.

/// The counters of a `Mirror`, also reported as `UndraStats.mirror` by `UndraCore.stats()`.
///
/// Every field counts since the mirror was created, except `pendingEntries` and `pendingBytes`,
/// which describe the queue at the moment of the snapshot.
public struct MirrorStats: Sendable, Equatable {
    /// Change-sets accepted (a malformed change-set is dropped whole and not counted).
    public var changeSetsReceived: Int
    /// Entries those change-sets carried.
    public var entriesReceived: Int
    /// Entries applied to stores after merging: calls of their apply functions.
    public var entriesApplied: Int
    /// Drains run: flushes that found something to apply.
    public var drains: Int
    /// Times the queue passed `LoadOptions.maxPendingEntries` or `maxPendingBytes` and was folded
    /// in place.
    public var compactions: Int
    /// Signals re-observed because a compaction dropped their merged keyed patch (or a keyed patch
    /// could not be merged).
    public var resyncs: Int
    /// Entries waiting for the next drain.
    public var pendingEntries: Int
    /// Bytes those entries account for: their values plus 17 bytes each.
    public var pendingBytes: Int
    /// Entries dropped because no store was registered for their handle (a store closed while its
    /// updates were in flight).
    public var droppedEntries: Int
    /// Calls of the app's main-thread callback implementations the drains made (ADR-041): queued with
    /// the change-sets and delivered in arrival order, never folded with them.
    public var callbacksDelivered: Int

    /// Counters with the given values; every one defaults to zero.
    public init(
        changeSetsReceived: Int = 0,
        entriesReceived: Int = 0,
        entriesApplied: Int = 0,
        drains: Int = 0,
        compactions: Int = 0,
        resyncs: Int = 0,
        pendingEntries: Int = 0,
        pendingBytes: Int = 0,
        droppedEntries: Int = 0,
        callbacksDelivered: Int = 0
    ) {
        self.changeSetsReceived = changeSetsReceived
        self.entriesReceived = entriesReceived
        self.entriesApplied = entriesApplied
        self.drains = drains
        self.compactions = compactions
        self.resyncs = resyncs
        self.pendingEntries = pendingEntries
        self.pendingBytes = pendingBytes
        self.droppedEntries = droppedEntries
        self.callbacksDelivered = callbacksDelivered
    }
}

/// What one drain of the mirror did, as reported to a drain listener
/// (`Mirror.addDrainListener(_:)`).
public struct DrainStats: Sendable, Equatable {
    /// Change-sets the drain consumed: those received since the previous drain.
    public var changeSets: Int
    /// Entries those change-sets carried.
    public var entries: Int
    /// Entries applied to stores after merging: calls of their apply functions.
    public var appliedEntries: Int
    /// How long the drain took on the main actor, measured with `ContinuousClock`.
    public var duration: Duration

    /// A drain report with the given values.
    public init(changeSets: Int, entries: Int, appliedEntries: Int, duration: Duration) {
        self.changeSets = changeSets
        self.entries = entries
        self.appliedEntries = appliedEntries
        self.duration = duration
    }
}

/// A drain listener added with `Mirror.addDrainListener(_:)`. The listener stays registered until
/// `remove()` is called, whether or not this object is kept.
public final class DrainListenerRegistration: Sendable {
    private let removal: @Sendable () -> Void

    init(removal: @escaping @Sendable () -> Void) {
        self.removal = removal
    }

    /// Stops calling the listener. Later calls do nothing. Safe from any thread; a drain already
    /// running may still report to it once.
    public func remove() {
        removal()
    }
}
