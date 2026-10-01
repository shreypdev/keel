package dev.undra.runtime

import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Payloads.ChangeSet
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.WireException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import kotlin.time.Duration.Companion.nanoseconds

/**
 * The host-side registry that routes core change-sets to the stores that observe them, and the queue
 * that turns them into one merged application per display frame (SPEC section 11, ADR-031).
 *
 * A store registers a callback for its handle; the runtime feeds the mirror every change-set the core
 * commits. Callbacks always run on the main thread ([UndraDispatchers.main]), never on the thread the
 * core delivered the change-set on, and:
 *
 *  - **Frame-aligned.** What the core produces on its own (timers, streams, events, background work) is
 *    applied once per display frame ([FramePacer]); a reply, a `callSync` made on the main thread and an
 *    `observe` apply what is queued without waiting for it, so `store.increment()` followed by a read of
 *    the store on the main thread sees the change.
 *  - **Merged.** One application (a *drain*) folds the queued entries per signal, in arrival order, without
 *    decoding them: a full value or a lazy invalidation supersedes everything before it, and keyed patches
 *    that follow each other become one patch (their counts add up, their operations keep their order,
 *    SPEC 3.8). Each signal is applied at most twice per drain (its last full value, then its merged patch),
 *    signals in the order their first entry arrived. Signals a store declared `no_coalesce` are applied
 *    entry by entry instead.
 *  - **Bounded.** Past [MirrorOptions.maxPendingEntries] or [MirrorOptions.maxPendingBytes] the queue is
 *    folded in place on the thread that passed the bound. A signal whose merged patch grows past 4,096
 *    operations and 1 MiB is dropped and re-observed at the next drain, so a main thread that falls far
 *    behind (or an app in the background) catches up in one drain with bounded memory.
 *  - **Isolated.** A callback that throws is logged and skipped; the other signals are still applied. A
 *    malformed change-set is dropped as a whole.
 *
 * Entries for a handle that is not registered (a store closed meanwhile) are dropped and counted
 * ([MirrorStats.droppedEntries]).
 *
 * This class is open only so that tests can substitute a fake mirror in a fake core; the runtime never
 * calls the overridable members from a core thread.
 */
public open class Mirror internal constructor(
    private val main: MainThread,
    options: MirrorOptions,
    private val resync: ((handle: Long, signalId: UInt) -> Unit)?,
) {

    /** A mirror that applies change-sets on [UndraDispatchers.main], paced by the runtime's default frame grid. */
    public constructor() : this(UndraDispatchers.mainThread(), MirrorOptions(), null)

    private class Registration(val apply: (UInt, ChangeOp, UndraReader) -> Unit, val noCoalesce: IntArray?)

    private class Listener(val onDrain: (DrainStats) -> Unit)

    private val pacer: FramePacer = options.framePacer ?: PacedFramePacer(main)
    private val maxEntries: Int = options.maxPendingEntries
    private val maxBytes: Long = options.maxPendingBytes

    private val handlers = ConcurrentHashMap<Long, Registration>()
    private val listeners = CopyOnWriteArrayList<Listener>()

    // ---- guarded by `lock`: the queue, what it holds, and the signals waiting for a full value ----
    private val lock = ReentrantLock()
    private val progress = lock.newCondition()
    private var queue = EntryBuffer()
    private var queueBytes = 0L
    private var compactAtEntries: Int = maxEntries
    private var compactAtBytes: Long = maxBytes

    /** Change-sets and entries received since a drain last took the queue. */
    private var queuedChangeSets = 0
    private var queuedEntries = 0
    private var frameRequested = false
    private var immediatePosted = false

    /** Signals whose content was dropped: `true` until re-observed, then `false` until their next full value. */
    private val awaiting = HashMap<SignalKey, Boolean>()
    private var resyncDue = false
    private var changeSetsReceived = 0L
    private var entriesReceived = 0L

    /** Change-sets a drain has taken and applied: what [awaitApplied] waits for. */
    private var changeSetsApplied = 0L
    private var compactions = 0L

    // ---- written on the main thread only ----
    @Volatile private var draining = false
    @Volatile private var entriesApplied = 0L
    @Volatile private var drains = 0L
    @Volatile private var resyncs = 0L
    @Volatile private var droppedEntries = 0L
    private var appliedInDrain = 0

    private val frameTask = Runnable {
        lock.withLock { frameRequested = false }
        drain()
    }
    private val immediateTask = Runnable {
        lock.withLock { immediatePosted = false }
        drain()
    }

    /**
     * Registers [apply] to receive the entries of every change-set that name [handle]: the signal id,
     * how to read the value, and a reader over exactly the value's bytes (valid only during the call).
     * A second registration for the same handle replaces the first. Same as
     * `register(handle, emptySet(), apply)`.
     */
    public open fun register(handle: Long, apply: (UInt, ChangeOp, UndraReader) -> Unit) {
        register(handle, emptySet(), apply)
    }

    /**
     * Registers [apply] for [handle], like the two-argument form, and names the store's signals declared
     * `#[undra(no_coalesce)]`: the mirror applies every entry of those, in order, instead of merging them
     * per drain. Generated stores pass them through [UndraStore]'s constructor.
     *
     * A `StateFlow` conflates by design, so a collector of such a signal may still miss values that were
     * set in quick succession; the mirror applies every one (the flow's `value` passes through each).
     */
    public open fun register(handle: Long, noCoalesce: Set<UInt>, apply: (UInt, ChangeOp, UndraReader) -> Unit) {
        val ids = if (noCoalesce.isEmpty()) null else IntArray(noCoalesce.size).also { out ->
            noCoalesce.forEachIndexed { i, id -> out[i] = id.toInt() }
        }
        handlers[handle] = Registration(apply, ids)
    }

    /** Stops routing entries to [handle]. Entries already queued for it are dropped (and counted) when they come up. */
    public open fun unregister(handle: Long) {
        handlers.remove(handle)
        lock.withLock {
            if (awaiting.isNotEmpty()) awaiting.keys.removeAll { it.handle == handle }
        }
    }

    /** The mirror's counters (SPEC section 17.2), also reported by [UndraCore.stats] as [UndraStats.mirror]. */
    public fun stats(): MirrorStats = lock.withLock {
        MirrorStats(
            changeSetsReceived = changeSetsReceived,
            entriesReceived = entriesReceived,
            entriesApplied = entriesApplied,
            drains = drains,
            compactions = compactions,
            resyncs = resyncs,
            pendingEntries = queue.size,
            pendingBytes = queueBytes,
            droppedEntries = droppedEntries,
        )
    }

    /**
     * Calls [listener] on the main thread after every drain with what it did ([DrainStats]). Drains are
     * timed only while a listener is registered. Closing the returned handle removes the listener (closing
     * it again does nothing); a listener that throws is logged and the others still run.
     */
    public fun addDrainListener(listener: (DrainStats) -> Unit): AutoCloseable {
        val entry = Listener(listener)
        listeners.add(entry)
        return AutoCloseable { listeners.remove(entry) }
    }

    /**
     * Applies everything queued now, without waiting for the next frame. Call it on the main thread
     * ([UndraDispatchers.main]); on any other thread it only requests an immediate (not frame-paced) drain
     * and returns. Called from inside a drain (from a store's `apply`) it returns at once: the running drain
     * applies what arrived in its next round.
     *
     * The runtime already drains where read-your-writes needs it (replies, `callSync` on the main thread,
     * `observe`); tests and tools that inspect stores right after the core produced something use this.
     */
    public fun flush() {
        if (main.isCurrent()) drain() else drainSoon()
    }

    /** Number of handles currently registered. */
    internal val registeredCount: Int get() = handlers.size

    /**
     * Queues the entries of a whole `ChangeSet` payload for the main thread and, unless one is already
     * outstanding, asks the frame pacer for a drain. Safe from any thread, including a core callback: it
     * never runs application code and never calls into the core. A malformed change-set is dropped as a
     * whole. The entries keep views into [changeSet], which must not be reused.
     */
    internal fun submit(changeSet: ByteArray) {
        // A change-set queued by the drain's own thread (a resync, a sync call from a callback) is picked
        // up by the drain's next round: no frame for it.
        val insideDrain = draining && main.isCurrent()
        var request = false
        var malformed: WireException? = null
        lock.withLock {
            val before = queue.size
            try {
                appendLocked(changeSet)
            } catch (e: WireException) {
                queue.truncate(before)
                malformed = e
                return@withLock
            }
            val added = queue.size - before
            changeSetsReceived++
            entriesReceived += added
            queuedChangeSets++
            queuedEntries += added
            if (queue.size > compactAtEntries || queueBytes > compactAtBytes) compactLocked()
            if (added > 0 && !frameRequested && !insideDrain) {
                frameRequested = true
                request = true
            }
        }
        malformed?.let {
            UndraLog.warn("dropping a malformed change-set (${changeSet.size} bytes)", it)
            return
        }
        if (request) requestFrame()
    }

    /**
     * Posts an immediate drain to the main thread (not the frame pacer) if anything is queued: what a
     * reply does before it resumes its caller, so a caller on the main thread runs after the drain.
     * At most one such post is outstanding.
     */
    internal fun drainSoon() {
        lock.withLock {
            if (immediatePosted || !hasWorkLocked()) return
            immediatePosted = true
        }
        main.post(immediateTask)
    }

    /** On the main thread, outside a drain: applies what is queued now. Anywhere else it does nothing. */
    internal fun drainIfOnMainThread() {
        if (!draining && main.isCurrent()) drain()
    }

    /**
     * Waits until every change-set received before this call has been applied. On the main thread this
     * applies them right away; anywhere else it asks the main thread for an immediate drain and blocks up
     * to [timeoutMillis] for it. Returns `false` only if the wait timed out.
     *
     * Called from inside a running callback (a store re-observing a signal it lost sync with), it
     * cannot apply anything without reordering the drain in progress, so it returns `true` at once: the
     * drain picks the new change-sets up in its next round.
     */
    internal fun awaitApplied(timeoutMillis: Long): Boolean {
        val target = lock.withLock {
            if (changeSetsApplied >= changeSetsReceived) return true
            changeSetsReceived
        }
        if (main.isCurrent()) {
            if (draining) return true
            drain()
            return lock.withLock { changeSetsApplied >= target }
        }
        drainSoon()
        var remaining = TimeUnit.MILLISECONDS.toNanos(timeoutMillis)
        lock.withLock {
            while (changeSetsApplied < target) {
                if (remaining <= 0L) return false
                remaining = progress.awaitNanos(remaining)
            }
        }
        return true
    }

    // ---- the queue (enqueuing threads, under the lock) ------------------------------------------------

    /** Parses [bytes] (SPEC 3.5) straight into the queue; the caller truncates it again on a [WireException]. */
    private fun appendLocked(bytes: ByteArray) {
        val r = UndraReader(bytes)
        r.readU64() // transaction id
        val count = r.readLen(ChangeSet.MIN_ENTRY_BYTES)
        var added = 0L
        for (i in 0 until count) {
            val handle = r.readI64()
            val signal = r.readI32()
            val at = r.position
            val op = ChangeOp.fromByte(r.readU8(), at).code.toByte()
            val length = r.readLen()
            val start = r.position
            r.skip(length)
            queue.add(handle, signal, op, bytes, start, length, weight = 1)
            added += ENTRY_OVERHEAD + length
        }
        r.finish()
        queueBytes += added
    }

    private fun hasWorkLocked(): Boolean = queue.size > 0 || resyncDue || queuedChangeSets > 0

    /**
     * Folds the backlog in place (ADR-031 decision 3), on the thread that passed the bound. Every signal is
     * folded, `no_coalesce` ones included (the bound wins over the opt-out); values are copied out of
     * their payloads when they would otherwise keep a much larger array alive. A merged patch past both
     * patch bounds is dropped and its signal re-observed at the next drain. The next compaction waits
     * until the backlog doubles, so folding stays O(1) per entry.
     */
    private fun compactLocked() {
        val source = queue
        val units = fold(source, everyKey = true)
        val out = EntryBuffer(maxOf(16, units.size * 2))
        var bytes = 0L
        for (unit in units) {
            val slot = unit as Slot
            if (slot.oversized) {
                markDropped(slot)
                continue
            }
            var weight = slot.entries
            val f = slot.full
            if (f >= 0) {
                out.addOwned(slot.handle, slot.signal, source.ops[f], source.values[f]!!, source.starts[f], source.lengths[f], weight)
                bytes += ENTRY_OVERHEAD + source.lengths[f]
                weight = 0
            }
            if (slot.patchCount == 1) {
                val p = slot.patchAt(0)
                out.addOwned(slot.handle, slot.signal, PATCH_CODE, source.values[p]!!, source.starts[p], source.lengths[p], weight)
                bytes += ENTRY_OVERHEAD + source.lengths[p]
            } else if (slot.patchCount > 1) {
                val merged = slot.mergedPatch(source)
                out.add(slot.handle, slot.signal, PATCH_CODE, merged, 0, merged.size, weight)
                bytes += ENTRY_OVERHEAD + merged.size
            }
        }
        queue = out
        queueBytes = bytes
        compactions++
        compactAtEntries = maxOf(maxEntries, 2 * out.size)
        compactAtBytes = maxOf(maxBytes, 2 * bytes)
    }

    /**
     * Folds [batch] (arrival order) per signal: one [Slot] per signal, in the order of its first entry;
     * for a drain (`everyKey` false), a [Single] per entry of a `no_coalesce` signal, in place. Entries of a
     * signal awaiting a full value are discarded until that value arrives. Runs under the lock, so that it
     * and a compaction see the awaiting signals in arrival order.
     */
    private fun fold(batch: EntryBuffer, everyKey: Boolean): ArrayList<Any> {
        val units = ArrayList<Any>()
        val index = SlotIndex()
        var lastHandle = 0L
        var lastNoCoalesce: IntArray? = null
        var haveLast = false
        for (i in 0 until batch.size) {
            val handle = batch.handles[i]
            val signal = batch.signals[i]
            val op = batch.ops[i]
            if (awaiting.isNotEmpty()) {
                val key = SignalKey(handle, signal)
                if (awaiting.containsKey(key)) {
                    // Its patches are relative to a list this host never saw: wait for a full value.
                    if (op == PATCH_CODE) continue
                    awaiting.remove(key)
                }
            }
            if (!everyKey) {
                if (!haveLast || handle != lastHandle) {
                    haveLast = true
                    lastHandle = handle
                    lastNoCoalesce = handlers[handle]?.noCoalesce
                }
                if (lastNoCoalesce?.contains(signal) == true) {
                    units.add(Single(i))
                    continue
                }
            }
            var slot = index[handle, signal]
            if (slot == null) {
                slot = Slot(handle, signal)
                index.put(slot)
                units.add(slot)
            }
            slot.entries += batch.weights[i]
            if (op != PATCH_CODE) {
                slot.setFull(i)
            } else if (!slot.addPatch(batch, i)) {
                UndraLog.warn(
                    "a keyed patch for signal ${signal.toUInt()} of ${Handle(handle)} cannot be merged (${batch.lengths[i]} bytes); " +
                        "the signal is re-observed",
                )
                markDropped(slot)
            }
        }
        return units
    }

    /** Forgets what [slot] holds and waits for a full value of its signal (re-observed at the next drain). */
    private fun markDropped(slot: Slot) {
        slot.clear()
        awaiting[SignalKey(slot.handle, slot.signal)] = true
        resyncDue = true
    }

    // ---- draining (main thread) ---------------------------------------------------------------------

    private fun requestFrame() {
        try {
            pacer.requestFrame(frameTask)
        } catch (e: Exception) {
            UndraLog.warn("the frame pacer did not accept a frame request; draining at the next main-thread turn instead", e)
            main.post(frameTask)
        }
    }

    /** What one round of a drain took from the queue. */
    private class Round(val batch: EntryBuffer, val units: List<Any>, val changeSets: Int, val entries: Int)

    /** Takes the queue and folds it, or returns `null` when nothing waits (empty change-sets then count as applied). */
    private fun takeRound(): Round? = lock.withLock {
        if (queue.size == 0 && !resyncDue) {
            if (queuedChangeSets > 0) {
                changeSetsApplied += queuedChangeSets
                queuedChangeSets = 0
                queuedEntries = 0
                progress.signalAll()
            }
            return null
        }
        val batch = queue
        queue = EntryBuffer()
        queueBytes = 0L
        compactAtEntries = maxEntries
        compactAtBytes = maxBytes
        val round = Round(batch, if (batch.size > 0) fold(batch, everyKey = false) else emptyList(), queuedChangeSets, queuedEntries)
        queuedChangeSets = 0
        queuedEntries = 0
        round
    }

    /**
     * Drains: takes the queue, folds it and applies it (see the class documentation), re-observes the
     * signals a compaction dropped, then calls the drain listeners. Entries queued while draining (a
     * callback whose synchronous core call commits, a resync answered in process) are applied by further
     * rounds of the same drain; after 1000 rounds the rest is left to the next frame.
     */
    private fun drain() {
        if (draining) return
        val timed = listeners.isNotEmpty()
        val started = if (timed) System.nanoTime() else 0L
        var changeSets = 0
        var entries = 0
        var rounds = 0
        draining = true
        appliedInDrain = 0
        try {
            while (true) {
                if (rounds == MAX_ROUNDS) {
                    UndraLog.warn(
                        "the mirror applied $MAX_ROUNDS rounds of change-sets in one drain: a store callback keeps causing " +
                            "changes to a store it observes; the rest is applied at the next frame",
                    )
                    break
                }
                val round = takeRound() ?: break
                rounds++
                changeSets += round.changeSets
                entries += round.entries
                try {
                    applyUnits(round.batch, round.units)
                } finally {
                    lock.withLock {
                        changeSetsApplied += round.changeSets
                        progress.signalAll()
                    }
                }
                requestResyncs()
            }
        } finally {
            draining = false
            if (rounds > 0) {
                drains++
                entriesApplied += appliedInDrain
            }
            // Left over by the round cap or by an error that unwound the loop: drained later, never stranded.
            val request = lock.withLock {
                (queue.size > 0 || resyncDue) && !frameRequested && run {
                    frameRequested = true
                    true
                }
            }
            if (request) requestFrame()
        }
        if (timed && rounds > 0) notifyListeners(DrainStats(changeSets, entries, appliedInDrain, (System.nanoTime() - started).nanoseconds))
    }

    private fun applyUnits(batch: EntryBuffer, units: List<Any>) {
        for (unit in units) {
            if (unit is Slot) applySlot(batch, unit) else applySingle(batch, (unit as Single).index)
        }
    }

    private fun applySlot(batch: EntryBuffer, slot: Slot) {
        val registration = handlers[slot.handle]
        if (registration == null) {
            droppedEntries += slot.entries
            return
        }
        val f = slot.full
        if (f >= 0) call(registration, slot.handle, slot.signal, batch.ops[f], batch.values[f]!!, batch.starts[f], batch.lengths[f])
        if (slot.patchCount == 1) {
            val p = slot.patchAt(0)
            call(registration, slot.handle, slot.signal, PATCH_CODE, batch.values[p]!!, batch.starts[p], batch.lengths[p])
        } else if (slot.patchCount > 1) {
            val merged = slot.mergedPatch(batch)
            call(registration, slot.handle, slot.signal, PATCH_CODE, merged, 0, merged.size)
        }
    }

    private fun applySingle(batch: EntryBuffer, i: Int) {
        val registration = handlers[batch.handles[i]]
        if (registration == null) {
            droppedEntries += batch.weights[i]
            return
        }
        call(registration, batch.handles[i], batch.signals[i], batch.ops[i], batch.values[i]!!, batch.starts[i], batch.lengths[i])
    }

    private fun call(registration: Registration, handle: Long, signal: Int, op: Byte, bytes: ByteArray, start: Int, length: Int) {
        appliedInDrain++
        try {
            registration.apply(signal.toUInt(), ChangeOp.entries[op.toInt()], UndraReader(bytes, start, length))
        } catch (e: OutOfMemoryError) {
            throw e
        } catch (e: Throwable) {
            UndraLog.warn("applying a change to signal ${signal.toUInt()} of ${Handle(handle)} failed; the change is skipped", e)
        }
    }

    /** Re-observes, once each, the signals whose content a compaction or an unmergeable patch dropped. */
    private fun requestResyncs() {
        val due = lock.withLock {
            if (!resyncDue) return
            resyncDue = false
            val out = ArrayList<SignalKey>()
            val it = awaiting.entries.iterator()
            while (it.hasNext()) {
                val entry = it.next()
                if (!handlers.containsKey(entry.key.handle)) {
                    it.remove()
                } else if (entry.value) {
                    entry.setValue(false)
                    out.add(entry.key)
                }
            }
            out
        }
        val resync = resync ?: return
        for (key in due) {
            resyncs++
            try {
                resync(key.handle, key.signal.toUInt())
            } catch (e: Exception) {
                UndraLog.warn("re-observing signal ${key.signal.toUInt()} of ${Handle(key.handle)} failed", e)
            }
        }
    }

    private fun notifyListeners(stats: DrainStats) {
        for (listener in listeners) {
            try {
                listener.onDrain(stats)
            } catch (e: OutOfMemoryError) {
                throw e
            } catch (e: Throwable) {
                UndraLog.warn("a drain listener failed", e)
            }
        }
    }

    // ---- data ---------------------------------------------------------------------------------------

    private data class SignalKey(val handle: Long, val signal: Int)

    /** An entry of a `no_coalesce` signal: applied on its own, at its arrival position. */
    private class Single(val index: Int)

    /** Queued entries as parallel arrays: a queued entry costs no object of its own. */
    private class EntryBuffer(capacity: Int = 16) {
        var size: Int = 0
            private set
        var handles = LongArray(capacity)
            private set
        var signals = IntArray(capacity)
            private set
        var ops = ByteArray(capacity)
            private set
        var values = arrayOfNulls<ByteArray>(capacity)
            private set
        var starts = IntArray(capacity)
            private set
        var lengths = IntArray(capacity)
            private set

        /** Entries of the original change-sets this entry stands for (a compaction folds many into one). */
        var weights = IntArray(capacity)
            private set

        fun add(handle: Long, signal: Int, op: Byte, value: ByteArray, start: Int, length: Int, weight: Int) {
            if (size == handles.size) grow()
            handles[size] = handle
            signals[size] = signal
            ops[size] = op
            values[size] = value
            starts[size] = start
            lengths[size] = length
            weights[size] = weight
            size++
        }

        /** [add], copying the value out when it is a small window into a much larger array. */
        fun addOwned(handle: Long, signal: Int, op: Byte, value: ByteArray, start: Int, length: Int, weight: Int) {
            if (length.toLong() * 2 + 64 < value.size) {
                add(handle, signal, op, value.copyOfRange(start, start + length), 0, length, weight)
            } else {
                add(handle, signal, op, value, start, length, weight)
            }
        }

        /** Drops the entries from [newSize] on (a change-set that turned out malformed). */
        fun truncate(newSize: Int) {
            for (i in newSize until size) values[i] = null
            size = newSize
        }

        private fun grow() {
            val n = maxOf(16, handles.size * 2)
            handles = handles.copyOf(n)
            signals = signals.copyOf(n)
            ops = ops.copyOf(n)
            values = values.copyOf(n)
            starts = starts.copyOf(n)
            lengths = lengths.copyOf(n)
            weights = weights.copyOf(n)
        }
    }

    /** One signal of one store as a drain or a compaction folds it (indices into the folded [EntryBuffer]). */
    private class Slot(val handle: Long, val signal: Int) {
        /** The last full value or lazy invalidation, or -1; everything before it is superseded. */
        var full = -1
            private set
        private var patches = IntArray(0)

        /** The keyed patches after [full], in arrival order. */
        var patchCount = 0
            private set

        /** Sum of the patches' op counts and of their op bytes (counts excluded). */
        private var ops = 0L
        private var opBytes = 0L

        /** Entries folded into this slot (weights). */
        var entries = 0

        val oversized: Boolean get() = ops > MAX_MERGED_PATCH_OPS && opBytes > MAX_MERGED_PATCH_BYTES

        fun setFull(index: Int) {
            full = index
            patchCount = 0
            ops = 0L
            opBytes = 0L
        }

        fun clear() = setFull(-1)

        fun patchAt(k: Int): Int = patches[k]

        /** Appends the keyed patch at [index]; `false` when it cannot be merged (no count, or a sum past `u32`). */
        fun addPatch(batch: EntryBuffer, index: Int): Boolean {
            val length = batch.lengths[index]
            if (length < 4) return false
            val count = readU32(batch.values[index]!!, batch.starts[index])
            val nextOps = ops + count
            val nextBytes = opBytes + (length - 4)
            if (nextOps > MAX_U32 || nextBytes > Int.MAX_VALUE - 8) return false
            if (patchCount == patches.size) patches = patches.copyOf(maxOf(4, patchCount * 2))
            patches[patchCount++] = index
            ops = nextOps
            opBytes = nextBytes
            return true
        }

        /**
         * The patches as one keyed patch: the sum of the counts, then every patch's ops in arrival order.
         * SPEC 3.8 applies ops one after the other, each index relative to the list the previous op left,
         * so this is the same change.
         */
        fun mergedPatch(batch: EntryBuffer): ByteArray {
            val out = ByteArray(4 + opBytes.toInt())
            out[0] = ops.toByte()
            out[1] = (ops ushr 8).toByte()
            out[2] = (ops ushr 16).toByte()
            out[3] = (ops ushr 24).toByte()
            var at = 4
            for (k in 0 until patchCount) {
                val i = patches[k]
                val n = batch.lengths[i] - 4
                System.arraycopy(batch.values[i]!!, batch.starts[i] + 4, out, at, n)
                at += n
            }
            return out
        }

        private fun readU32(bytes: ByteArray, at: Int): Long =
            (bytes[at].toLong() and 0xff) or
                ((bytes[at + 1].toLong() and 0xff) shl 8) or
                ((bytes[at + 2].toLong() and 0xff) shl 16) or
                ((bytes[at + 3].toLong() and 0xff) shl 24)
    }

    /** An open-addressing table from (handle, signal) to the [Slot] of one fold: no allocation per lookup. */
    private class SlotIndex {
        private var table = arrayOfNulls<Slot>(16)
        private var count = 0

        operator fun get(handle: Long, signal: Int): Slot? {
            val mask = table.size - 1
            var i = hash(handle, signal) and mask
            while (true) {
                val slot = table[i] ?: return null
                if (slot.handle == handle && slot.signal == signal) return slot
                i = (i + 1) and mask
            }
        }

        fun put(slot: Slot) {
            if ((count + 1) * 2 > table.size) {
                val old = table
                table = arrayOfNulls(old.size * 2)
                for (s in old) if (s != null) insert(s)
            }
            insert(slot)
            count++
        }

        private fun insert(slot: Slot) {
            val mask = table.size - 1
            var i = hash(slot.handle, slot.signal) and mask
            while (table[i] != null) i = (i + 1) and mask
            table[i] = slot
        }

        private fun hash(handle: Long, signal: Int): Int {
            val h = (handle xor (signal.toLong() * 0x9E3779B97F4A7C15uL.toLong())) * -0x40a7b892e31b1a47L
            return (h xor (h ushr 29)).toInt()
        }
    }

    internal companion object {
        /** Rounds one drain runs before it leaves the rest to the next frame (the same bound as the core's commit, SPEC 16.1). */
        const val MAX_ROUNDS: Int = 1000

        /** What a queued entry costs besides its value: the wire's fixed part (handle, signal id, op, length). */
        const val ENTRY_OVERHEAD: Int = 17

        /** A merged patch with more operations than this **and** more op bytes than [MAX_MERGED_PATCH_BYTES] is dropped by a compaction. */
        const val MAX_MERGED_PATCH_OPS: Long = 4096

        /** See [MAX_MERGED_PATCH_OPS]. */
        const val MAX_MERGED_PATCH_BYTES: Long = 1L shl 20

        private const val MAX_U32: Long = 0xFFFF_FFFFL
        private val PATCH_CODE: Byte = ChangeOp.PATCH.code.toByte()
    }
}
