package dev.undra.runtime

import dev.undra.runtime.support.ManualFramePacer
import dev.undra.runtime.support.ManualMainThread
import dev.undra.runtime.support.flushOnThisThread
import dev.undra.runtime.support.full
import dev.undra.runtime.support.lazyInvalidated
import dev.undra.runtime.support.lazyValue
import dev.undra.runtime.support.manualMirror
import dev.undra.runtime.support.patch
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.KeyedPatch
import dev.undra.runtime.wire.PatchOp
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/*
 * A model-based check of frame-coalesced delivery (ADR-031, SPEC section 11), written by the review of the
 * piece (the TypeScript twin is `runtimes/ts/@undra/runtime/test/coalesce-model.test.ts`). A model core
 * commits transactions over two stores (one change-set per store, entries in signal-id order), a "wire"
 * delivers a random prefix of what it holds, drains run at random points (a transaction can straddle two),
 * the backlog bound is small (compactions run mid-history), one signal is `no_coalesce`, out-of-bounds
 * patches ask for a resync, and resyncs are answered later, behind what the core already sent. A rare
 * bulk patch passes both patch bounds on its own, so a compaction drops it and the mirror re-observes.
 *
 * Invariants: once the wire is empty, no resync is outstanding and a drain left nothing queued, every
 * mirrored value equals the core's; between those points (without corrupt patches) a signal never shows a
 * value the core never had; the `no_coalesce` signal sees a subsequence of its committed values (all of
 * them while no compaction ran), ending with the last.
 *
 * One signal of each store is a lazy list (ADR-043; added by the types-paging review): a commit sends an invalidation
 * (length, version) and now and then a restart of its page server sends a full value with a new handle. ADR-031's
 * amended fold rule (a full value supersedes everything before it, an invalidation only earlier invalidations) must
 * never lose the handle: reverting it to "an invalidation supersedes everything" fails this model.
 */

private val HANDLES = listOf(1L, 2L)
private val U32_LISTS = listOf(0, 2)
private const val STRING_LIST = 4
private val SCALARS = listOf(1, 3)
private const val NO_COALESCE_HANDLE = 2L
private const val NO_COALESCE_SIGNAL = 3
/** A `Lazy<T>` signal: its value is (page server handle, length, version). */
private const val LAZY = 5
private val ALL_KEYS = U32_LISTS + STRING_LIST + SCALARS + LAZY
private val u32List = Codecs.vec(Codecs.u32)
private val strList = Codecs.vec(Codecs.string)

private class ModelRng(seed: Int) {
    private var x = if (seed == 0) 0x9e3779b9.toInt() else seed

    operator fun invoke(n: Int): Int {
        x = x xor (x shl 13)
        x = x xor (x ushr 17)
        x = x xor (x shl 5)
        return Integer.remainderUnsigned(x, n)
    }
}

private class StoreState {
    val lists = HashMap<Int, List<UInt>>().apply { for (id in U32_LISTS) put(id, emptyList()) }
    var strings: List<String> = emptyList()
    val scalars = HashMap<Int, UInt>().apply { for (id in SCALARS) put(id, 0u) }
    var lazyHandle = 1L
    var lazyLen = 0
    var lazyVersion = 0uL

    fun render(id: Int): String = when {
        id == LAZY -> "$lazyHandle/$lazyLen/$lazyVersion"
        id == STRING_LIST -> "#${strings.size}/" + strings.joinToString(",") { it.take(12) }
        id in U32_LISTS -> lists.getValue(id).toString()
        else -> scalars.getValue(id).toString()
    }

    fun render(): String = ALL_KEYS.joinToString(" ") { "$it:${render(it)}" }
}

private fun <T> patchBytes(ops: List<PatchOp<T>>, codec: UndraCodec<T>): ByteArray = KeyedPatch.encodePatch(ops, codec)

private class ModelCore(val rand: ModelRng, val bulk: Boolean, val corrupt: Boolean) {
    val truth = HANDLES.associateWith { StoreState() }
    val wire = ArrayDeque<ByteArray>()
    val resyncRequests = ArrayDeque<Pair<Long, Int>>()
    val progressCommitted = ArrayList<UInt>()
    val history = HashMap<String, MutableSet<String>>()
    private var txn = 0uL
    private var item = 1u

    init {
        for (h in HANDLES) for (id in ALL_KEYS) remember(h, id)
    }

    private fun remember(h: Long, id: Int) {
        history.getOrPut("$h/$id") { HashSet() }.add(truth.getValue(h).render(id))
    }

    fun full(h: Long, id: Int): Payloads.ChangeEntry {
        val s = truth.getValue(h)
        val value = when {
            id == LAZY -> lazyValue(s.lazyHandle, s.lazyLen, s.lazyVersion)
            id == STRING_LIST -> strList.encodeToByteArray(s.strings)
            id in U32_LISTS -> u32List.encodeToByteArray(s.lists.getValue(id))
            else -> Codecs.u32.encodeToByteArray(s.scalars.getValue(id))
        }
        return full(h, id.toUInt(), value)
    }

    fun commit() {
        txn++
        val stores = if (rand(3) == 0) HANDLES else listOf(HANDLES[rand(HANDLES.size)])
        for (h in stores) {
            val touched = LinkedHashMap<Int, Payloads.ChangeEntry>()
            repeat(1 + rand(3)) {
                val pick = rand(100)
                when {
                    pick >= 92 -> {
                        // The lazy list changes (an invalidation), or its page server restarts (a full value with a new
                        // handle). One entry per signal per change-set: after a restart in this transaction, a full value.
                        val s = truth.getValue(h)
                        val restart = pick >= 97 || touched[LAZY]?.op == ChangeOp.FULL
                        if (pick >= 97) s.lazyHandle++
                        s.lazyLen = rand(500)
                        s.lazyVersion++
                        touched[LAZY] = if (restart) full(h, LAZY) else lazyInvalidated(h, LAZY.toUInt(), s.lazyLen, s.lazyVersion)
                    }
                    pick < 30 -> {
                        val id = SCALARS[rand(SCALARS.size)]
                        val v = item++
                        truth.getValue(h).scalars[id] = v
                        if (h == NO_COALESCE_HANDLE && id == NO_COALESCE_SIGNAL) {
                            if (touched.containsKey(id)) progressCommitted.removeAt(progressCommitted.size - 1)
                            progressCommitted.add(v)
                        }
                        touched[id] = full(h, id)
                    }
                    pick < 38 -> {
                        val id = U32_LISTS[rand(U32_LISTS.size)]
                        truth.getValue(h).lists[id] = List(rand(6)) { item++ }
                        touched[id] = full(h, id)
                    }
                    pick < 39 && bulk -> touched[STRING_LIST] = bulkPatch(h, touched.containsKey(STRING_LIST))
                    bulk && rand(3) == 0 -> touched[STRING_LIST] = stringPatch(h, touched.containsKey(STRING_LIST))
                    else -> {
                        val id = U32_LISTS[rand(U32_LISTS.size)]
                        touched[id] = listPatch(h, id, touched.containsKey(id))
                    }
                }
            }
            for (id in touched.keys) remember(h, id)
            val entries = touched.values.sortedBy { it.signalId }
            wire.addLast(Payloads.ChangeSet(txn, entries).toByteArray())
        }
    }

    private fun listPatch(h: Long, id: Int, again: Boolean): Payloads.ChangeEntry {
        val s = truth.getValue(h)
        val ops = ArrayList<PatchOp<UInt>>()
        repeat(1 + rand(4)) {
            val list = s.lists.getValue(id)
            val n = list.size
            val op: PatchOp<UInt> = when (if (n == 0) 0 else rand(6)) {
                0 -> PatchOp.Insert(rand(n + 1).toUInt(), item++)
                1 -> PatchOp.Remove(rand(n).toUInt())
                2 -> PatchOp.Update(rand(n).toUInt(), item++)
                3 -> PatchOp.Move(rand(n).toUInt(), rand(n).toUInt())
                4 -> if (rand(8) == 0) PatchOp.Clear else PatchOp.Remove((n - 1).toUInt())
                else -> PatchOp.Insert(n.toUInt(), item++)
            }
            ops.add(op)
            s.lists[id] = KeyedPatch.applyPatch(list, listOf(op))
        }
        if (again) return full(h, id)
        if (corrupt && rand(30) == 0) ops.add(rand(ops.size + 1), PatchOp.Remove(1_000_000u))
        return patch(h, id.toUInt(), patchBytes(ops, Codecs.u32))
    }

    private fun stringPatch(h: Long, again: Boolean): Payloads.ChangeEntry {
        val s = truth.getValue(h)
        val ops = ArrayList<PatchOp<String>>()
        repeat(1 + rand(3)) {
            val n = s.strings.size
            val op: PatchOp<String> = when (if (n == 0) 0 else rand(4)) {
                0 -> PatchOp.Insert(rand(n + 1).toUInt(), "s${item++}")
                1 -> PatchOp.Remove(rand(n).toUInt())
                2 -> PatchOp.Update(rand(n).toUInt(), "u${item++}")
                else -> PatchOp.Move(rand(n).toUInt(), rand(n).toUInt())
            }
            ops.add(op)
            s.strings = KeyedPatch.applyPatch(s.strings, listOf(op))
        }
        if (again) return full(h, STRING_LIST)
        return patch(h, STRING_LIST.toUInt(), patchBytes(ops, Codecs.string))
    }

    /** Clear, then 4,200 inserts of about 270 bytes: past both patch bounds on its own. */
    private fun bulkPatch(h: Long, again: Boolean): Payloads.ChangeEntry {
        val tag = item++
        val items = List(Mirror.MAX_MERGED_PATCH_OPS.toInt() + 104) { "$tag:$it:" + "y".repeat(256) }
        truth.getValue(h).strings = items
        if (again) return full(h, STRING_LIST)
        val ops = ArrayList<PatchOp<String>>()
        ops.add(PatchOp.Clear)
        items.forEachIndexed { i, s -> ops.add(PatchOp.Insert(i.toUInt(), s)) }
        return patch(h, STRING_LIST.toUInt(), patchBytes(ops, Codecs.string))
    }

    fun answerResyncs(max: Int) {
        var n = 0
        while (n < max && resyncRequests.isNotEmpty()) {
            val (h, id) = resyncRequests.removeFirst()
            txn++
            wire.addLast(Payloads.ChangeSet(txn, listOf(full(h, id))).toByteArray())
            n++
        }
    }
}

private class ModelHost(val core: ModelCore) {
    val state = HANDLES.associateWith { StoreState() }
    val progressSeen = ArrayList<UInt>()

    fun apply(h: Long): (UInt, ChangeOp, UndraReader) -> Unit = { signal, op, reader ->
        val s = state.getValue(h)
        val id = signal.toInt()
        when {
            id == LAZY -> if (op == ChangeOp.FULL) {
                val v = Payloads.LazyValue.decode(reader)
                reader.finish()
                s.lazyHandle = v.handle.raw
                s.lazyLen = v.len.toInt()
                s.lazyVersion = v.version
            } else {
                // An invalidation is relative to the page server a full value named: it keeps the handle.
                val v = Payloads.LazyInvalidated.decode(reader)
                reader.finish()
                s.lazyLen = v.len.toInt()
                s.lazyVersion = v.version
            }
            id == STRING_LIST -> if (op == ChangeOp.FULL) {
                s.strings = strList.decode(reader)
                reader.finish()
            } else {
                val ops = KeyedPatch.decodePatch(reader, Codecs.string)
                reader.finish()
                try {
                    s.strings = KeyedPatch.applyPatch(s.strings, ops)
                } catch (e: WireException.PatchOutOfBounds) {
                    core.resyncRequests.addLast(h to id)
                }
            }
            id in U32_LISTS -> if (op == ChangeOp.FULL) {
                s.lists[id] = u32List.decode(reader)
                reader.finish()
            } else {
                val ops = KeyedPatch.decodePatch(reader, Codecs.u32)
                reader.finish()
                try {
                    s.lists[id] = KeyedPatch.applyPatch(s.lists.getValue(id), ops)
                } catch (e: WireException.PatchOutOfBounds) {
                    // What generated code does: re-observe (the core answers later).
                    core.resyncRequests.addLast(h to id)
                }
            }
            else -> {
                val v = Codecs.u32.decode(reader)
                reader.finish()
                s.scalars[id] = v
                if (h == NO_COALESCE_HANDLE && id == NO_COALESCE_SIGNAL) progressSeen.add(v)
            }
        }
    }
}

private fun isSubsequence(sub: List<UInt>, of: List<UInt>): Boolean {
    var at = 0
    for (v in sub) {
        while (at < of.size && of[at] != v) at++
        if (at == of.size) return false
        at++
    }
    return true
}

/** Frames that never come: only explicit drains apply anything. */
private val NO_FRAMES = FramePacer { }

class CoalesceModelTests : Suite() {
    init {
        case("review: random histories over two stores, partial delivery, random drains, small bounds, asynchronous resyncs") {
            val seeds = System.getenv("UNDRA_MODEL_SEEDS")?.toIntOrNull() ?: 1500
            var compactions = 0L
            var checks = 0
            for (seed in 1..seeds) {
                val r = runHistory(seed, bulk = false)
                compactions += r.first
                checks += r.second
            }
            assertTrue(compactions > 100, "only $compactions compactions")
            assertTrue(checks > seeds, "only $checks settled checks")
        }

        case("review: histories with patches past both bounds (dropped by a compaction, re-observed)") {
            var resyncs = 0L
            for (seed in 10_001..10_120) resyncs += runHistory(seed, bulk = true).third
            assertTrue(resyncs > 0, "no merged patch was dropped")
        }

        case("review: a drain does not chase change-sets that keep arriving from another thread") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            val applies = AtomicInteger()
            mirror.register(1L) { _, _, r ->
                Codecs.u32.decode(r)
                applies.incrementAndGet()
                // What applying a merged patch to a list of 10,000 rows costs (the copy): about 20 us.
                val until = System.nanoTime() + 20_000
                while (System.nanoTime() < until) Thread.onSpinWait()
            }
            val payloads = List(1024) { Payloads.ChangeSet(it.toULong() + 1uL, listOf(full(1L, 0u, Codecs.u32.encodeToByteArray(it.toUInt())))).toByteArray() }
            val stop = AtomicBoolean(false)
            val running = CountDownLatch(1)
            val producer = Thread {
                var i = 0
                while (!stop.get()) {
                    mirror.submit(payloads[i and 1023])
                    if (i++ == 2000) running.countDown()
                }
            }
            producer.start()
            val drains = CopyOnWriteArrayList<DrainStats>()
            try {
                running.await()
                mirror.addDrainListener { drains.add(it) }.use { mirror.flushOnThisThread(main) }
            } finally {
                stop.set(true)
                producer.join()
            }
            assertEq(1, drains.size)
            val drain = drains[0]
            // One signal: a drain that takes the queue once applies it once. Each further round it runs for a
            // producer on another thread is one more application (and, for a keyed list, one more list copy).
            assertTrue(drain.appliedEntries <= 2, "one drain applied ${drain.appliedEntries} times over ${drain.changeSets} change-sets in ${drain.duration}")
        }
    }

    /** Returns (compactions, settled checks, resyncs). */
    private fun runHistory(seed: Int, bulk: Boolean): Triple<Long, Int, Long> {
        val rand = ModelRng(seed)
        val corrupt = rand(2) == 0
        val core = ModelCore(rand, bulk, corrupt)
        val host = ModelHost(core)
        val main = ManualMainThread()
        val small = rand(2) == 0
        val mirror = manualMirror(
            main,
            NO_FRAMES,
            maxPendingEntries = if (small) 2 + rand(24) else 65_536,
            maxPendingBytes = if (small) 64L + rand(4096) else 16L * 1024 * 1024,
            resync = { h, id -> core.resyncRequests.addLast(h to id.toInt()) },
        )
        for (h in HANDLES) {
            if (h == NO_COALESCE_HANDLE) mirror.register(h, setOf(NO_COALESCE_SIGNAL.toUInt()), host.apply(h)) else mirror.register(h, host.apply(h))
        }
        fun deliver(n: Int) {
            var i = 0
            while (i < n && core.wire.isNotEmpty()) {
                mirror.submit(core.wire.removeFirst())
                i++
            }
        }
        var checks = 0
        fun settle(context: String) {
            for (guard in 0 until 100) {
                core.answerResyncs(Int.MAX_VALUE)
                deliver(Int.MAX_VALUE)
                mirror.flushOnThisThread(main)
                if (core.wire.isEmpty() && core.resyncRequests.isEmpty() && mirror.stats().pendingEntries == 0) break
            }
            checks++
            for (h in HANDLES) assertEq(core.truth.getValue(h).render(), host.state.getValue(h).render(), "$context handle $h")
        }
        val steps = 20 + rand(120)
        for (step in 0 until steps) {
            val pick = rand(100)
            when {
                pick < 55 -> core.commit()
                pick < 75 -> deliver(1 + rand(6))
                pick < 87 -> {
                    mirror.flushOnThisThread(main)
                    if (!corrupt) {
                        for (h in HANDLES) for (id in ALL_KEYS) {
                            val shown = host.state.getValue(h).render(id)
                            assertTrue(core.history.getValue("$h/$id").contains(shown), "seed $seed step $step: $h/$id shows ${shown.take(80)}")
                        }
                    }
                }
                pick < 95 -> core.answerResyncs(1 + rand(2))
                else -> settle("seed $seed step $step")
            }
        }
        settle("seed $seed end")
        val stats = mirror.stats()
        val committed = core.progressCommitted
        if (committed.isNotEmpty()) {
            assertEq(committed.last(), host.progressSeen.lastOrNull(), "seed $seed: the no_coalesce signal ends on the last value")
            assertTrue(isSubsequence(host.progressSeen, committed), "seed $seed: no_coalesce values in commit order")
            if (stats.compactions == 0L) assertEq(committed.toList(), host.progressSeen.toList(), "seed $seed: every no_coalesce value applied")
        }
        return Triple(stats.compactions, checks, stats.resyncs)
    }
}
