package dev.undra.runtime

import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Payloads.ChangeSet
import dev.undra.runtime.wire.WireException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/**
 * The host-side registry that routes core change-sets to the stores that observe them (SPEC section 11).
 *
 * A store registers a callback for its handle; the runtime feeds it every change-set the core emits.
 * Callbacks always run on the main thread ([UndraDispatchers.main]), never on the thread the core
 * delivered the change-set on, and:
 *
 *  - **Batching.** Change-sets that arrive while the main thread is busy are applied together in one
 *    hop, in commit order, so a burst of transactions costs one main-thread turn.
 *  - **Coalescing.** Inside a batch, an entry that a later *full value* of the same signal makes
 *    obsolete is skipped, so a store that changed 60 times between two frames decodes its list once.
 *    Patches are never merged and keep their order relative to the full values around them.
 *  - **Isolation.** A callback that throws is logged and skipped; the rest of the change-set and later
 *    change-sets are still applied. A malformed change-set is dropped as a whole.
 *
 * Entries for a handle that is not registered (a store closed meanwhile) are ignored.
 *
 * This class is open only so that tests can substitute a fake mirror in a fake core; the runtime never
 * calls the overridable members from a core thread.
 */
public open class Mirror internal constructor(private val main: MainThread) {

    /** A mirror that applies change-sets on [UndraDispatchers.main]. */
    public constructor() : this(UndraDispatchers.mainThread())

    private val handlers = ConcurrentHashMap<Long, (UInt, ChangeOp, UndraReader) -> Unit>()
    private val queue = ConcurrentLinkedQueue<ByteArray>()
    private val scheduled = AtomicBoolean(false)
    private val submitted = AtomicLong(0)
    private val applied = AtomicLong(0)
    private val hopCount = AtomicLong(0)
    private val waiters = AtomicInteger(0)
    private val waitLock = ReentrantLock()
    private val progress = waitLock.newCondition()
    private val drainTask = Runnable { drain() }

    /** Only touched on the main thread: set while a batch is being applied, so a nested drain is a no-op. */
    private var draining = false

    /**
     * Registers [apply] to receive the entries of every change-set that name [handle]: the signal id,
     * how to read the value, and a reader over exactly the value's bytes (valid only during the call).
     * A second registration for the same handle replaces the first.
     */
    public open fun register(handle: Long, apply: (UInt, ChangeOp, UndraReader) -> Unit) {
        handlers[handle] = apply
    }

    /** Stops routing entries to [handle]. Entries already queued for it are dropped when they come up. */
    public open fun unregister(handle: Long) {
        handlers.remove(handle)
    }

    /** Number of handles currently registered. */
    internal val registeredCount: Int get() = handlers.size

    /** Number of main-thread hops that applied at least one change-set (tests and stats). */
    internal val hops: Long get() = hopCount.get()

    /**
     * Queues a whole `ChangeSet` payload for the main thread. Safe from any thread, including a core
     * callback: it never runs application code and never calls into the core.
     */
    internal fun submit(changeSet: ByteArray) {
        submitted.incrementAndGet()
        queue.add(changeSet)
        if (scheduled.compareAndSet(false, true)) main.post(drainTask)
    }

    /**
     * Waits until every change-set submitted before this call has been applied. On the main thread
     * this applies them right away; anywhere else it blocks up to [timeoutMillis] for the main thread
     * to do it. Returns `false` only if the wait timed out.
     *
     * Called from inside a running callback (a store re-observing a signal it lost sync with), it
     * cannot apply anything without reordering the batch in progress, so it returns `true` at once: the
     * batch picks the new change-sets up as soon as the current callback returns.
     */
    internal fun awaitApplied(timeoutMillis: Long): Boolean {
        val target = submitted.get()
        if (applied.get() >= target) return true
        if (main.isCurrent()) {
            if (draining) return true
            drain()
            return applied.get() >= target
        }
        var remaining = TimeUnit.MILLISECONDS.toNanos(timeoutMillis)
        waiters.incrementAndGet()
        try {
            waitLock.withLock {
                while (applied.get() < target) {
                    if (remaining <= 0L) return false
                    remaining = progress.awaitNanos(remaining)
                }
            }
        } finally {
            waiters.decrementAndGet()
        }
        return true
    }

    private fun drain() {
        if (draining) return
        draining = true
        try {
            var rounds = 0
            while (rounds < MAX_ROUNDS_PER_HOP) {
                val batch = takeBatch()
                if (batch.isEmpty()) break
                rounds++
                try {
                    applyBatch(batch)
                } finally {
                    hopCount.incrementAndGet()
                    applied.addAndGet(batch.size.toLong())
                    signalProgress()
                }
            }
        } finally {
            draining = false
            scheduled.set(false)
        }
        // Change-sets that arrived after the last poll, or the rounds cap: another hop.
        if (!queue.isEmpty() && scheduled.compareAndSet(false, true)) main.post(drainTask)
    }

    private fun takeBatch(): List<ByteArray> {
        val first = queue.poll() ?: return emptyList()
        val next = queue.poll() ?: return listOf(first)
        val batch = ArrayList<ByteArray>(4)
        batch.add(first)
        batch.add(next)
        while (true) batch.add(queue.poll() ?: break)
        return batch
    }

    private fun signalProgress() {
        if (waiters.get() == 0) return
        waitLock.withLock { progress.signalAll() }
    }

    private fun applyBatch(batch: List<ByteArray>) {
        val sets = ArrayList<Parsed>(batch.size)
        for (bytes in batch) parse(bytes)?.let { sets.add(it) }
        if (sets.isEmpty()) return
        // A later full value of the same signal makes every earlier entry for it obsolete. With one
        // change-set there is nothing to compare (a transaction writes a signal once).
        val lastFull: HashMap<SignalKey, Long>? = if (sets.size > 1) lastFullValues(sets) else null
        for (setIndex in sets.indices) {
            val set = sets[setIndex]
            for (i in 0 until set.count) {
                if (lastFull != null) {
                    val newest = lastFull[SignalKey(set.handles[i], set.signals[i])]
                    if (newest != null && newest > order(setIndex, i)) continue
                }
                deliver(set, i)
            }
        }
    }

    private fun lastFullValues(sets: List<Parsed>): HashMap<SignalKey, Long> {
        val last = HashMap<SignalKey, Long>()
        for (setIndex in sets.indices) {
            val set = sets[setIndex]
            for (i in 0 until set.count) {
                if (set.ops[i] == FULL_CODE) last[SignalKey(set.handles[i], set.signals[i])] = order(setIndex, i)
            }
        }
        return last
    }

    private fun deliver(set: Parsed, i: Int) {
        val handler = handlers[set.handles[i]] ?: return
        try {
            val op = ChangeOp.entries[set.ops[i].toInt()]
            handler(set.signals[i].toUInt(), op, UndraReader(set.bytes, set.starts[i], set.lens[i]))
        } catch (e: OutOfMemoryError) {
            throw e
        } catch (e: Throwable) {
            UndraLog.warn(
                "applying a change to signal ${set.signals[i].toUInt()} of ${describeHandle(set.handles[i])} failed; the change is skipped",
                e,
            )
        }
    }

    private fun parse(bytes: ByteArray): Parsed? =
        try {
            val r = UndraReader(bytes)
            r.readU64() // transaction id
            val count = r.readLen(ChangeSet.MIN_ENTRY_BYTES)
            val set = Parsed(bytes, count)
            for (i in 0 until count) {
                set.handles[i] = r.readI64()
                set.signals[i] = r.readI32()
                val at = r.position
                set.ops[i] = ChangeOp.fromByte(r.readU8(), at).code.toByte()
                val len = r.readLen()
                set.starts[i] = r.position
                set.lens[i] = len
                r.skip(len)
            }
            r.finish()
            set
        } catch (e: WireException) {
            UndraLog.warn("dropping a malformed change-set (${bytes.size} bytes)", e)
            null
        }

    /** A change-set's entries as parallel arrays, so applying it allocates no per-entry objects. */
    private class Parsed(val bytes: ByteArray, val count: Int) {
        val handles = LongArray(count)
        val signals = IntArray(count)
        val ops = ByteArray(count) // ChangeOp wire codes
        val starts = IntArray(count)
        val lens = IntArray(count)
    }

    private data class SignalKey(val handle: Long, val signal: Int)

    private companion object {
        /** After this many back-to-back batches the main thread gets a turn before the next hop. */
        const val MAX_ROUNDS_PER_HOP = 32

        fun order(setIndex: Int, entry: Int): Long = (setIndex.toLong() shl 32) or entry.toLong()

        val FULL_CODE: Byte = ChangeOp.FULL.code.toByte()

        fun describeHandle(handle: Long): String = Handle(handle).toString()
    }
}
