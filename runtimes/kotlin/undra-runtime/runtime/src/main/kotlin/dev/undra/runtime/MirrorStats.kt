package dev.undra.runtime

import kotlin.time.Duration

/**
 * The counters of a [Mirror] ([Mirror.stats], also [UndraStats.mirror]): what the core sent, what reached
 * the stores after merging, and how the backlog behaved (ADR-031, SPEC section 17.2).
 *
 * @property changeSetsReceived change-sets accepted (malformed ones are not counted).
 * @property entriesReceived entries those change-sets carried.
 * @property entriesApplied entries applied to stores after merging: calls of their `apply` callbacks.
 * @property drains drains run (applications of the queue that found something to do).
 * @property compactions times the backlog passed its bound and was folded in place.
 * @property resyncs signals re-observed because the backlog dropped their merged patch.
 * @property pendingEntries entries waiting for the next drain.
 * @property pendingBytes bytes those entries hold (their values plus 17 bytes each).
 * @property droppedEntries entries dropped because no store was registered for their handle.
 */
public class MirrorStats(
    public val changeSetsReceived: Long,
    public val entriesReceived: Long,
    public val entriesApplied: Long,
    public val drains: Long,
    public val compactions: Long,
    public val resyncs: Long,
    public val pendingEntries: Int,
    public val pendingBytes: Long,
    public val droppedEntries: Long,
) {
    override fun toString(): String =
        "MirrorStats(changeSetsReceived=$changeSetsReceived, entriesReceived=$entriesReceived, entriesApplied=$entriesApplied, " +
            "drains=$drains, compactions=$compactions, resyncs=$resyncs, pendingEntries=$pendingEntries, " +
            "pendingBytes=$pendingBytes, droppedEntries=$droppedEntries)"
}

/**
 * What one drain did; see [Mirror.addDrainListener].
 *
 * @property changeSets change-sets the drain consumed (received since the previous drain).
 * @property entries entries those change-sets carried.
 * @property appliedEntries entries applied to stores after merging: calls of their `apply` callbacks.
 * @property duration how long the drain took, from `System.nanoTime`.
 */
public class DrainStats(
    public val changeSets: Int,
    public val entries: Int,
    public val appliedEntries: Int,
    public val duration: Duration,
) {
    override fun toString(): String =
        "DrainStats(changeSets=$changeSets, entries=$entries, appliedEntries=$appliedEntries, duration=$duration)"
}

/** Every counter at zero: what [UndraStats] reports when nothing filled in the mirror's counters. */
internal val NO_MIRROR_STATS: MirrorStats = MirrorStats(0L, 0L, 0L, 0L, 0L, 0L, 0, 0L, 0L)
