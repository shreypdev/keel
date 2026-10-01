package dev.undra.runtime

import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.ManualFramePacer
import dev.undra.runtime.support.ManualMainThread
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.attach
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.flushOnThisThread
import dev.undra.runtime.support.full
import dev.undra.runtime.support.invalidated
import dev.undra.runtime.support.manualMirror
import dev.undra.runtime.support.patch
import dev.undra.runtime.support.replyPayload
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.KeyedPatch
import dev.undra.runtime.wire.PatchOp
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.logging.Level
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

/*
 * Frame-coalesced delivery (ADR-031, SPEC section 11): the merge rules of a drain, its equivalence with
 * applying every change-set in order (property test), the bounded backlog, read-your-writes for replies
 * and synchronous calls, frame pacing, the drain listener and the counters, and the `no_coalesce` opt-out.
 */

// ----- helpers ---------------------------------------------------------------------------------------

private var txn = 0uL

/** One change-set payload. */
private fun cs(vararg entries: Payloads.ChangeEntry): ByteArray = Payloads.ChangeSet(++txn, entries.toList()).toByteArray()

private fun u32(v: Int): ByteArray = Codecs.u32.encodeToByteArray(v.toUInt())

/** A keyed patch of `u32` items. */
private fun patchBytes(ops: List<PatchOp<UInt>>): ByteArray = KeyedPatch.encodePatch(ops, Codecs.u32)

private val listCodec = Codecs.vec(Codecs.u32)

/** A tiny deterministic PRNG (xorshift32), so a failing case can be replayed from its seed. */
private class Rng(seed: Int) {
    private var x = if (seed == 0) 0x9e3779b9.toInt() else seed

    /** A number in `0 until n`. */
    operator fun invoke(n: Int): Int {
        x = x xor (x shl 13)
        x = x xor (x ushr 17)
        x = x xor (x shl 5)
        return Integer.remainderUnsigned(x, n)
    }
}

/** A frame pacer whose frames never come: only a reply, a sync call or a flush can apply anything. */
private val NEVER = FramePacer { }

/**
 * A host-side store of keyed `u32` lists and scalars that applies entries the way generated code does
 * (`Stores.kt`): a full value decodes, a keyed patch goes through [KeyedPatch.applyPatch] (one copy of
 * the list per entry), and an out-of-bounds patch asks for a resync.
 */
private class ListHost(listIds: List<Int>, scalarIds: List<Int>, private val onResync: (Int) -> Unit) {
    val lists = HashMap<Int, List<UInt>>()
    val scalars = HashMap<Int, UInt>()
    var applies = 0

    init {
        for (id in listIds) lists[id] = emptyList()
        for (id in scalarIds) scalars[id] = 0u
    }

    val apply: (UInt, ChangeOp, UndraReader) -> Unit = { signalId, op, reader ->
        applies++
        val id = signalId.toInt()
        val list = lists[id]
        if (list != null) {
            if (op == ChangeOp.FULL) {
                lists[id] = listCodec.decode(reader)
                reader.finish()
            } else if (op == ChangeOp.PATCH) {
                val ops = KeyedPatch.decodePatch(reader, Codecs.u32)
                reader.finish()
                try {
                    lists[id] = KeyedPatch.applyPatch(list, ops)
                } catch (e: WireException.PatchOutOfBounds) {
                    onResync(id)
                }
            }
        } else if (scalars.containsKey(id) && op == ChangeOp.FULL) {
            scalars[id] = Codecs.u32.decode(reader)
            reader.finish()
        }
    }

    fun snapshot(): String = "lists=${lists.toSortedMap()} scalars=${scalars.toSortedMap()}"
}

/** The core's side of a [ListHost]: the truth, and the change-sets that move it. */
private class ListCore(listIds: List<Int>, scalarIds: List<Int>, private val handle: Long = 1L) {
    val lists = HashMap<Int, List<UInt>>()
    val scalars = HashMap<Int, UInt>()
    private var nextItem = 1u

    init {
        for (id in listIds) lists[id] = emptyList()
        for (id in scalarIds) scalars[id] = 0u
    }

    fun full(signalId: Int): ByteArray {
        val list = lists[signalId]
        val value = if (list != null) listCodec.encodeToByteArray(list) else Codecs.u32.encodeToByteArray(scalars.getValue(signalId))
        return cs(full(handle, signalId.toUInt(), value))
    }

    /** Random valid ops on list [signalId] (applied to the truth); [corrupt] appends one out-of-bounds op the core never made. */
    fun patch(signalId: Int, rand: Rng, count: Int, corrupt: Boolean): ByteArray {
        val ops = ArrayList<PatchOp<UInt>>()
        repeat(count) {
            val list = lists.getValue(signalId)
            val size = list.size
            val op: PatchOp<UInt> = when (if (size == 0) 0 else rand(5)) {
                0 -> PatchOp.Insert(rand(size + 1).toUInt(), nextItem++)
                1 -> PatchOp.Remove(rand(size).toUInt())
                2 -> PatchOp.Update(rand(size).toUInt(), nextItem++)
                3 -> PatchOp.Move(rand(size).toUInt(), rand(size).toUInt())
                else -> if (rand(10) == 0) PatchOp.Clear else PatchOp.Insert(size.toUInt(), nextItem++)
            }
            ops.add(op)
            lists[signalId] = KeyedPatch.applyPatch(list, listOf(op))
        }
        if (corrupt) ops.add(PatchOp.Remove((lists.getValue(signalId).size + 3).toUInt()))
        return cs(patch(handle, signalId.toUInt(), patchBytes(ops)))
    }

    /** A new full value of list [signalId]. */
    fun replace(signalId: Int, rand: Rng): ByteArray {
        lists[signalId] = List(rand(6)) { nextItem++ }
        return cs(full(handle, signalId.toUInt(), listCodec.encodeToByteArray(lists.getValue(signalId))))
    }

    fun scalar(signalId: Int, value: Int): ByteArray {
        scalars[signalId] = value.toUInt()
        return cs(full(handle, signalId.toUInt(), u32(value)))
    }

    fun snapshot(): String = "lists=${lists.toSortedMap()} scalars=${scalars.toSortedMap()}"
}

/** A store shaped like a generated one, with signal 1 declared `no_coalesce`: it records every value it is given. */
private class ProgressStore(core: UndraCore, handle: Long) : UndraStore(core, handle, noCoalesce = setOf(1u)) {
    val seen = CopyOnWriteArrayList<String>()

    override fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader) {
        seen.add("$signalId=${Codecs.u32.decode(reader)}")
        reader.finish()
    }
}

class CoalesceTests : Suite() {
    init {
        mergeRules()
        noCoalesce()
        equivalence()
        backlog()
        readYourWrites()
        framePacing()
        countersAndListeners()
    }

    // ----- the merge rules -------------------------------------------------------------------------

    private fun mergeRules() {
        case("a drain applies only the last full value of a signal, once") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            val seen = CopyOnWriteArrayList<UInt>()
            mirror.register(1L) { _, _, r -> seen.add(Codecs.u32.decode(r)) }
            for (i in 1..1000) mirror.submit(cs(full(1L, 0u, u32(i))))
            assertEq(1, pacer.requests.get(), "one frame request for the whole burst")
            pacer.frame()
            assertEq(listOf(1000u), seen.toList())
            val s = mirror.stats()
            assertEq(1000L, s.changeSetsReceived)
            assertEq(1000L, s.entriesReceived)
            assertEq(1L, s.entriesApplied)
            assertEq(1L, s.drains)
        }

        case("consecutive keyed patches are concatenated into one: counts add up, ops keep their order") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val values = CopyOnWriteArrayList<ByteArray>()
            mirror.register(1L) { _, op, r ->
                assertEq(ChangeOp.PATCH, op)
                values.add(r.readRemaining())
            }
            val a = listOf<PatchOp<UInt>>(PatchOp.Insert(0u, 7u))
            val b = listOf<PatchOp<UInt>>(PatchOp.Move(0u, 0u), PatchOp.Update(0u, 8u))
            val c = listOf<PatchOp<UInt>>(PatchOp.Clear)
            for (ops in listOf(a, b, c)) mirror.submit(cs(patch(1L, 3u, patchBytes(ops))))
            mirror.flushOnThisThread(main)
            assertEq(1, values.size)
            assertEq(patchBytes(a + b + c), values[0], "the count is 1 + 2 + 1 and the ops follow in arrival order")
            assertEq(a + b + c, KeyedPatch.decodePatch(values[0], Codecs.u32))
        }

        case("a single keyed patch is handed over as it arrived") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val values = CopyOnWriteArrayList<ByteArray>()
            mirror.register(1L) { _, _, r -> values.add(r.readRemaining()) }
            val bytes = patchBytes(listOf(PatchOp.Insert(0u, 1u), PatchOp.Remove(0u)))
            mirror.submit(cs(patch(1L, 0u, bytes)))
            mirror.flushOnThisThread(main)
            assertEq(listOf(bytes.toList()), values.map { it.toList() })
        }

        case("a signal is applied at most twice: its last full value, then the patches after it, merged") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val seen = CopyOnWriteArrayList<Pair<ChangeOp, List<UInt>>>()
            mirror.register(1L) { _, op, r ->
                if (op == ChangeOp.FULL) {
                    seen.add(op to listCodec.decode(r))
                } else {
                    seen.add(op to KeyedPatch.decodePatch(r, Codecs.u32).map { (it as PatchOp.Insert).item })
                }
            }
            mirror.submit(cs(patch(1L, 0u, patchBytes(listOf(PatchOp.Insert(0u, 1u))))))
            mirror.submit(cs(full(1L, 0u, listCodec.encodeToByteArray(listOf(5u)))))
            mirror.submit(cs(patch(1L, 0u, patchBytes(listOf(PatchOp.Insert(1u, 6u))))))
            mirror.submit(cs(patch(1L, 0u, patchBytes(listOf(PatchOp.Insert(2u, 7u))))))
            mirror.flushOnThisThread(main)
            assertEq(listOf(ChangeOp.FULL to listOf(5u), ChangeOp.PATCH to listOf(6u, 7u)), seen.toList())
            assertEq(4L, mirror.stats().entriesReceived)
            assertEq(2L, mirror.stats().entriesApplied)
        }

        case("a lazy invalidation supersedes what came before it") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val ops = CopyOnWriteArrayList<ChangeOp>()
            mirror.register(1L) { _, op, _ -> ops.add(op) }
            mirror.submit(cs(full(1L, 0u, u32(1))))
            mirror.submit(cs(patch(1L, 0u, patchBytes(listOf(PatchOp.Clear)))))
            mirror.submit(cs(invalidated(1L, 0u)))
            mirror.flushOnThisThread(main)
            assertEq(listOf(ChangeOp.INVALIDATED), ops.toList())
        }

        case("signals are applied in the order of their first entry") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val order = CopyOnWriteArrayList<String>()
            mirror.register(1L) { _, _, r -> order.add("a=${Codecs.u32.decode(r)}") }
            mirror.register(2L) { _, _, r -> order.add("b=${Codecs.u32.decode(r)}") }
            mirror.submit(cs(full(2L, 0u, u32(1))))
            mirror.submit(cs(full(1L, 0u, u32(1))))
            mirror.submit(cs(full(2L, 0u, u32(2)), full(1L, 0u, u32(2))))
            mirror.flushOnThisThread(main)
            assertEq(listOf("b=2", "a=2"), order.toList())
        }

        case("the entries of a handle nobody registered are dropped, every one counted") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            for (i in 0 until 5) mirror.submit(cs(full(9L, 0u, u32(i))))
            mirror.flushOnThisThread(main)
            assertEq(5L, mirror.stats().droppedEntries)
            assertEq(0L, mirror.stats().entriesApplied)
        }

        case("a keyed patch too short to hold a count cannot be merged: the signal is re-observed and waits for a full value") {
            val main = ManualMainThread()
            val resyncs = CopyOnWriteArrayList<Pair<Long, UInt>>()
            lateinit var mirror: Mirror
            mirror = manualMirror(main, resync = { h, s -> resyncs.add(h to s) })
            val seen = CopyOnWriteArrayList<String>()
            mirror.register(1L) { s, op, r -> seen.add("$s:$op:${r.remaining}") }
            LogCapture("dev.undra.runtime").use { log ->
                mirror.submit(cs(full(1L, 0u, u32(1)), patch(1L, 0u, byteArrayOf(1, 0)), full(1L, 1u, u32(5))))
                mirror.flushOnThisThread(main)
                assertTrue(log.records.any { it.level == Level.WARNING && it.message.contains("cannot be merged") }, "the drop is logged")
            }
            assertEq(listOf("1:FULL:4"), seen.toList(), "signal 0 lost its content; signal 1 is unaffected")
            assertEq(listOf(1L to 0u), resyncs.toList())
            assertEq(1L, mirror.stats().resyncs)
            // Patches that arrive before the full value are relative to a list this host never saw: discarded.
            mirror.submit(cs(patch(1L, 0u, patchBytes(listOf(PatchOp.Clear)))))
            mirror.flushOnThisThread(main)
            assertEq(1, seen.size)
            mirror.submit(cs(full(1L, 0u, u32(9)), patch(1L, 0u, patchBytes(listOf(PatchOp.Clear)))))
            mirror.flushOnThisThread(main)
            assertEq(listOf("1:FULL:4", "0:FULL:4", "0:PATCH:5"), seen.toList())
            assertEq(1, resyncs.size, "re-observed once")
        }
    }

    // ----- no_coalesce -----------------------------------------------------------------------------

    private fun noCoalesce() {
        case("no_coalesce signals are applied entry by entry, in order, at their arrival position") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val heard = CopyOnWriteArrayList<String>()
            mirror.register(1L, setOf(1u)) { id, _, r -> heard.add("${if (id == 1u) "progress" else "other"} ${Codecs.u32.decode(r)}") }
            for (i in 1..3) mirror.submit(cs(full(1L, 0u, u32(10 * i)), full(1L, 1u, u32(i))))
            mirror.flushOnThisThread(main)
            assertEq(listOf("other 30", "progress 1", "progress 2", "progress 3"), heard.toList())
            assertEq(4L, mirror.stats().entriesApplied)
        }

        case("no_coalesce signals are folded like any other when the backlog passes its bound (the bound wins)") {
            val main = ManualMainThread()
            val mirror = manualMirror(main, maxPendingEntries = 10)
            val seen = CopyOnWriteArrayList<UInt>()
            mirror.register(1L, setOf(0u)) { _, _, r -> seen.add(Codecs.u32.decode(r)) }
            for (i in 1..25) mirror.submit(cs(full(1L, 0u, u32(i))))
            mirror.flushOnThisThread(main)
            assertTrue(mirror.stats().compactions > 0, "compacted")
            assertEq(25u, seen.last())
            assertTrue(seen.size < 25, "folded: ${seen.size} applies")
        }

        case("a generated-style store passes its no_coalesce signals through UndraStore") {
            val main = ManualMainThread()
            val t = FakeTransport()
            val core = ConnectedCore(t, 5.seconds, mirrorOptions = MirrorOptions(framePacer = NEVER), main = main)
            t.connect(core, HASH)
            core.use {
                val store = ProgressStore(core, 4L)
                for (i in 1..3) t.events.onChangeSet(cs(full(4L, 0u, u32(10 * i)), full(4L, 1u, u32(i))))
                core.mirror.flushOnThisThread(main)
                assertEq(listOf("0=30", "1=1", "1=2", "1=3"), store.seen.toList())
                store.close()
            }
        }

        case("a test double's mirror sees a store through whichever register it overrides") {
            val calls = CopyOnWriteArrayList<String>()
            val fake = object : Mirror() {
                override fun register(handle: Long, apply: (UInt, ChangeOp, UndraReader) -> Unit) {
                    calls.add("register($handle)")
                }

                override fun register(handle: Long, noCoalesce: Set<UInt>, apply: (UInt, ChangeOp, UndraReader) -> Unit) {
                    calls.add("register($handle, $noCoalesce)")
                }
            }
            val double = object : UndraCore() {
                override val mirror: Mirror get() = fake

                override fun release(handle: Long) = Unit
            }
            object : UndraStore(double, 1L) {
                override fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader) = Unit
            }.close()
            ProgressStore(double, 2L).close()
            assertEq(listOf("register(1)", "register(2, [1])"), calls.toList())
        }
    }

    // ----- equivalence with sequential application (property test) --------------------------------

    private fun equivalence() {
        case("for 600 random histories, one merged drain equals one drain per change-set, out-of-bounds patches included") {
            val lists = listOf(0, 2)
            val scalars = listOf(1)
            var corrupted = 0
            for (seed in 1..600) {
                val rand = Rng(seed)
                val core = ListCore(lists, scalars)

                /** A host whose resync is answered at once with the core's state as it is then (an in-process core). */
                fun host(): Triple<ListHost, Mirror, ManualMainThread> {
                    val main = ManualMainThread()
                    lateinit var mirror: Mirror
                    val h = ListHost(lists, scalars) { signalId -> mirror.submit(core.full(signalId)) }
                    mirror = manualMirror(main, NEVER)
                    mirror.register(1L, h.apply)
                    return Triple(h, mirror, main)
                }
                val (seqHost, seqMirror, seqMain) = host()
                val (mergedHost, mergedMirror, mergedMain) = host()
                fun deliver(payload: ByteArray) {
                    seqMirror.submit(payload)
                    seqMirror.flushOnThisThread(seqMain)
                    mergedMirror.submit(payload)
                }
                for (id in lists + scalars) deliver(core.full(id))
                var corrupt = false
                val steps = 1 + rand(40)
                repeat(steps) {
                    val pick = rand(100)
                    val list = lists[rand(lists.size)]
                    when {
                        pick < 10 -> deliver(core.replace(list, rand))
                        pick < 25 -> deliver(core.scalar(1, rand(1000)))
                        else -> {
                            val bad = rand(25) == 0
                            corrupt = corrupt || bad
                            deliver(core.patch(list, rand, 1 + rand(4), bad))
                        }
                    }
                }
                // Every change-set committed before one drain; a resync gets the final state.
                mergedMirror.flushOnThisThread(mergedMain)

                val truth = core.snapshot()
                val context = "seed $seed"
                if (corrupt) corrupted++
                assertEq(truth, seqHost.snapshot(), "$context (one drain per change-set)")
                assertEq(truth, mergedHost.snapshot(), "$context (one merged drain)")
                assertEq(0, mergedMirror.stats().pendingEntries, context)
                if (!corrupt) {
                    assertTrue(mergedHost.applies <= 2 * (lists.size + scalars.size), "$context: ${mergedHost.applies} applies")
                }
            }
            assertTrue(corrupted > 10, "only $corrupted histories carried an out-of-bounds patch")
        }
    }

    // ----- the bounded backlog ---------------------------------------------------------------------

    private fun backlog() {
        case("1,000,000 change-sets with the main thread blocked stay under the bound, and one drain converges") {
            val lists = listOf(0, 1)
            val scalars = listOf(2, 3, 4, 5)
            val rand = Rng(31337)
            val truth = ListCore(lists, scalars)
            val main = ManualMainThread()
            val t = FakeTransport()
            val options = MirrorOptions(framePacer = NEVER)
            val core = ConnectedCore(t, 5.seconds, mirrorOptions = options, main = main)
            t.connect(core, HASH)
            // A resync is answered the way an in-process core answers observe: the current value, at once.
            t.onObserve = { _, signal, on -> if (on) t.events.onChangeSet(truth.full(signal.toInt())) }
            core.use {
                val host = ListHost(lists, scalars) { error("the host never sees an out-of-bounds patch here") }
                core.mirror.register(1L, host.apply)
                var maxEntries = 0
                var maxBytes = 0L
                val producer = Thread {
                    for (id in lists + scalars) t.events.onChangeSet(truth.full(id))
                    for (i in 0 until 1_000_000) {
                        val payload = if (rand(10) < 6) truth.patch(lists[i and 1], rand, 1, false) else truth.scalar(scalars[i % scalars.size], i)
                        t.events.onChangeSet(payload)
                        if (i and 1023 == 0) {
                            val s = core.mirror.stats()
                            maxEntries = maxOf(maxEntries, s.pendingEntries)
                            maxBytes = maxOf(maxBytes, s.pendingBytes)
                        }
                    }
                }
                producer.start()
                producer.join(120_000)
                val before = core.mirror.stats()
                assertTrue(maxEntries <= options.maxPendingEntries, "pending entries peaked at $maxEntries")
                assertTrue(maxBytes <= options.maxPendingBytes, "pending bytes peaked at $maxBytes")
                assertTrue(before.compactions > 5, "compactions: ${before.compactions}")
                assertEq(1_000_006L, before.changeSetsReceived)
                assertEq(0L, before.drains, "the main thread never ran")

                core.mirror.flushOnThisThread(main)
                assertEq(truth.snapshot(), host.snapshot())
                assertEq(0, core.mirror.stats().pendingEntries)
                // Each list's patches passed both patch bounds (hundreds of thousands of ops), so they were dropped and re-observed.
                assertEq(listOf(Triple(1L, 0u, true), Triple(1L, 1u, true)), t.observes.sortedBy { it.second }.toList())
                assertEq(2L, core.stats().mirror.resyncs)
                assertEq(1L, core.stats().mirror.drains)
            }
        }

        case("a merged patch past both patch bounds is dropped and its signal re-observed once") {
            val main = ManualMainThread()
            val big: UndraCodec<String> = Codecs.string
            var truth: List<String> = emptyList()
            val resyncs = CopyOnWriteArrayList<UInt>()
            lateinit var mirror: Mirror
            val full = { cs(full(1L, 0u, Codecs.vec(big).encodeToByteArray(truth))) }
            mirror = manualMirror(main, NEVER, maxPendingEntries = 1000, resync = { _, s ->
                resyncs.add(s)
                mirror.submit(full())
            })
            var list: List<String> = emptyList()
            mirror.register(1L) { _, op, r ->
                list = if (op == ChangeOp.FULL) Codecs.vec(big).decode(r) else KeyedPatch.applyPatch(list, KeyedPatch.decodePatch(r, big))
            }
            val text = "x".repeat(300)
            // Compactions run every 1,000 entries; the one after the 4,096th op finds the merged patch past both bounds.
            for (i in 0 until Mirror.MAX_MERGED_PATCH_OPS.toInt() + 1500) {
                val op = PatchOp.Insert(truth.size.toUInt(), "$i$text")
                truth = truth + op.item
                mirror.submit(cs(patch(1L, 0u, KeyedPatch.encodePatch(listOf(op), big))))
            }
            assertTrue(mirror.stats().compactions > 0, "compacted")
            // Patches that arrive while the signal waits for its full value are discarded.
            assertTrue(mirror.stats().pendingEntries < 1000, "pending: ${mirror.stats().pendingEntries}")
            mirror.flushOnThisThread(main)
            assertEq(listOf(0u), resyncs.toList())
            assertEq(truth, list)
        }

        case("the backlog is folded when its bytes pass their bound too") {
            val main = ManualMainThread()
            val mirror = manualMirror(main, NEVER, maxPendingBytes = 64L * 1024)
            val seen = CopyOnWriteArrayList<Int>()
            mirror.register(1L) { _, _, r -> seen.add(r.remaining) }
            val blob = ByteArray(4096)
            for (i in 0 until 100) mirror.submit(cs(full(1L, 0u, blob)))
            assertTrue(mirror.stats().compactions > 0, "compacted")
            assertTrue(mirror.stats().pendingBytes <= 64L * 1024, "pending bytes: ${mirror.stats().pendingBytes}")
            mirror.flushOnThisThread(main)
            assertEq(listOf(4096), seen.toList())
        }

        case("a compaction copies a value out of a large payload instead of keeping the payload alive") {
            val main = ManualMainThread()
            val mirror = manualMirror(main, NEVER, maxPendingEntries = 4)
            val seen = CopyOnWriteArrayList<UInt>()
            mirror.register(1L) { _, _, r -> seen.add(Codecs.u32.decode(r)) }
            // One change-set with a large neighbour: after folding, only the small value is kept.
            mirror.submit(cs(full(1L, 0u, u32(7)), full(2L, 0u, ByteArray(100_000))))
            for (i in 0 until 5) mirror.submit(cs(full(2L, 0u, u32(i))))
            assertTrue(mirror.stats().compactions > 0, "compacted")
            assertTrue(mirror.stats().pendingBytes < 1000, "the large value was superseded: ${mirror.stats().pendingBytes}")
            mirror.flushOnThisThread(main)
            assertEq(listOf(7u), seen.toList())
        }
    }

    // ----- read-your-writes ------------------------------------------------------------------------

    /** A core over a fake in-process transport whose frames never come, with a store-like callback on [HANDLE]. */
    private class Rig(synchronous: Boolean = true, val main: MainThread = ExecutorMainThread()) : AutoCloseable {
        val t = FakeTransport(isSynchronous = synchronous)
        val core = ConnectedCore(t, 5.seconds, mirrorOptions = MirrorOptions(framePacer = NEVER), main = main)

        @Volatile var count: UInt = 1u

        init {
            t.connect(core, HASH)
            core.mirror.register(HANDLE) { _, _, r -> count = Codecs.u32.decode(r) }
        }

        /** Runs [block] on the main thread and returns what it returned. */
        fun <T> onMain(block: suspend () -> T): T = runBlocking(main.dispatcher) { block() }

        override fun close() = core.close()

        companion object {
            const val HANDLE: Long = 0x1_0000_0001L
            val TARGET: CallTarget = CallTarget.ObjectMethod(Handle(HANDLE), 0x51u)
        }
    }

    private fun readYourWrites() {
        case("read-your-writes holds when a call resumes on the main thread after an asynchronous reply") {
            Rig().use { rig ->
                // The method writes the counter in a change-set that arrives before its reply, from the core thread.
                rig.t.onCall = { call ->
                    rig.t.onCore {
                        rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, u32(7))))
                        rig.t.events.onReply(call.callId, ReplyStatus.OK, NO_BYTES)
                    }
                }
                val seen = rig.onMain {
                    rig.core.call(Rig.TARGET, 0x51u, NO_BYTES)
                    rig.count
                }
                assertEq(7u, seen)
                assertEq(0, rig.core.mirror.stats().pendingEntries)
            }
        }

        case("read-your-writes holds when the reply is delivered inside the call (in process)") {
            Rig().use { rig ->
                rig.t.onCall = { call ->
                    rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, u32(8))))
                    rig.t.events.onReply(call.callId, ReplyStatus.OK, NO_BYTES)
                }
                assertEq(8u, rig.onMain { rig.core.call(Rig.TARGET, 0x51u, NO_BYTES); rig.count })
            }
        }

        case("read-your-writes holds for a rejected call too") {
            Rig().use { rig ->
                rig.t.onCall = { call ->
                    rig.t.onCore {
                        rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, u32(13))))
                        rig.t.events.onReply(call.callId, ReplyStatus.ERROR, u32(0))
                    }
                }
                val seen = rig.onMain {
                    val status = try {
                        rig.core.call(Rig.TARGET, 0x51u, NO_BYTES)
                        null
                    } catch (e: UndraReplyException) {
                        e.status
                    }
                    assertEq(ReplyStatus.ERROR, status)
                    rig.count
                }
                assertEq(13u, seen)
            }
        }

        case("a reply posts an immediate drain to the main thread ahead of the caller, not a frame") {
            val main = ManualMainThread()
            Rig(main = main).use { rig ->
                rig.t.onCall = { call ->
                    rig.t.onCore {
                        rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, u32(5))))
                        rig.t.events.onReply(call.callId, ReplyStatus.OK, NO_BYTES)
                    }
                }
                runBlocking(Dispatchers.Default) { rig.core.call(Rig.TARGET, 0x51u, NO_BYTES) }
                assertEq(1u, rig.count, "nothing is applied off the main thread")
                assertEq(1, main.pending, "one immediate drain is waiting on the main thread")
                main.runPending()
                assertEq(5u, rig.count)
            }
        }

        case("callSync on the main thread returns after its change-sets were applied; elsewhere they wait for a frame") {
            Rig().use { rig ->
                // The method sets the counter to its argument, in a change-set delivered inside the call (in process).
                rig.t.onCallSync = { call ->
                    rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, call.args)))
                    replyPayload(call.callId, ReplyStatus.OK)
                }
                assertEq(9u, rig.onMain { rig.core.callSync(Rig.TARGET, 0x51u, u32(9)); rig.count })
                rig.core.callSync(Rig.TARGET, 0x51u, u32(10)) // from the test thread: left to the next frame
                assertEq(9u, rig.count)
                assertEq(1, rig.core.mirror.stats().pendingEntries)
                assertEq(11u, rig.onMain { rig.core.callSync(Rig.TARGET, 0x51u, u32(11)); rig.count })
                assertEq(2L, rig.core.mirror.stats().drains)
            }
        }

        case("a rejected callSync on the main thread applies its change-sets before it throws") {
            Rig().use { rig ->
                rig.t.onCallSync = { call ->
                    rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, u32(21))))
                    replyPayload(call.callId, ReplyStatus.ERROR, u32(1))
                }
                val seen = rig.onMain {
                    assertThrows<UndraReplyException> { rig.core.callSync(Rig.TARGET, 0x51u, NO_BYTES) }
                    rig.count
                }
                assertEq(21u, seen)
            }
        }

        case("construct on the main thread applies the change-sets the constructor produced") {
            Rig().use { rig ->
                rig.t.onCallSync = { call ->
                    rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, u32(3))))
                    replyPayload(call.callId, ReplyStatus.OK, Codecs.handle.encodeToByteArray(0x1_0000_0002L))
                }
                assertEq(3u, rig.onMain { rig.core.construct(1u, 2u, NO_BYTES); rig.count })
            }
        }

        case("a callSync made from inside a drain does not drain again; the drain's next round applies its change-sets") {
            Rig().use { rig ->
                val order = CopyOnWriteArrayList<String>()
                rig.core.mirror.register(Rig.HANDLE) { s, _, r ->
                    val v = Codecs.u32.decode(r)
                    order.add("$s=$v")
                    if (s == 0u && v == 1u) {
                        rig.core.callSync(Rig.TARGET, 0x51u, NO_BYTES)
                        order.add("returned")
                    }
                }
                rig.t.onCallSync = { call ->
                    rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 2u, u32(2))))
                    replyPayload(call.callId, ReplyStatus.OK)
                }
                rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, u32(1)), full(Rig.HANDLE, 1u, u32(1))))
                rig.onMain { rig.core.mirror.flush() }
                assertEq(listOf("0=1", "returned", "1=1", "2=2"), order.toList())
                assertEq(1L, rig.core.mirror.stats().drains)
            }
        }

        case("a change the core makes on its own is left to the next frame") {
            Rig().use { rig ->
                rig.t.onCore { rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, u32(21)))) }
                rig.t.awaitCore()
                Thread.sleep(30)
                assertEq(1u, rig.count, "not applied before a frame")
                assertEq(1, rig.core.mirror.stats().pendingEntries)
                rig.onMain { rig.core.mirror.flush() }
                assertEq(21u, rig.count)
            }
        }

        case("flush off the main thread requests an immediate drain and returns") {
            val main = ManualMainThread()
            Rig(main = main).use { rig ->
                rig.t.events.onChangeSet(cs(full(Rig.HANDLE, 0u, u32(4))))
                rig.core.mirror.flush()
                rig.core.mirror.flush()
                assertEq(1u, rig.count)
                assertEq(1, main.pending, "one immediate drain, however often it is asked for")
                main.runPending()
                assertEq(4u, rig.count)
            }
        }

        case("observe off the main thread waits for an immediate drain, not a frame") {
            Rig().use { rig ->
                rig.t.onObserve = { h, _, on -> if (on) rig.t.events.onChangeSet(cs(full(h, 0u, u32(41)))) }
                rig.core.observe(Rig.HANDLE, UInt.MAX_VALUE, true)
                assertEq(41u, rig.count, "applied before observe returned, though no frame ever comes")
            }
        }
    }

    // ----- frame pacing ----------------------------------------------------------------------------

    private fun framePacing() {
        case("change-sets queued before a frame cause one frame request and one drain") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            mirror.register(1L) { _, _, _ -> }
            for (i in 0 until 100) mirror.submit(cs(full(1L, (i % 3).toUInt(), u32(i))))
            assertEq(1, pacer.requests.get())
            pacer.frame()
            assertEq(1L, mirror.stats().drains)
            assertEq(3L, mirror.stats().entriesApplied)
            mirror.submit(cs(full(1L, 0u, u32(1))))
            assertEq(2, pacer.requests.get(), "the next change-set after a drain asks for the next frame")
            mirror.submit(cs(Payloads.ChangeEntry(Handle(1L), 0u, ChangeOp.FULL, u32(2))))
            assertEq(2, pacer.requests.get())
        }

        case("an empty change-set asks for no frame") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            mirror.submit(cs())
            assertEq(0, pacer.requests.get())
            assertEq(1L, mirror.stats().changeSetsReceived)
            assertTrue(mirror.awaitApplied(10).not(), "off the main thread it waits for a drain")
            main.runPending()
            assertTrue(mirror.awaitApplied(10), "an empty change-set is applied by the drain it asked for")
            assertEq(0L, mirror.stats().drains, "a drain with nothing to apply is not counted")
        }

        case("the paced default pacer posts the frame to the main thread from the undra-frame thread, on its grid") {
            val posts = CopyOnWriteArrayList<Pair<String, Long>>()
            val ran = CountDownLatch(3)
            val main = object : MainThread {
                override val dispatcher: CoroutineDispatcher get() = Dispatchers.Unconfined

                override fun post(task: Runnable) {
                    posts.add(Thread.currentThread().name to System.nanoTime())
                    task.run()
                }

                override fun isCurrent(): Boolean = false
            }
            val interval = 5_000_000L
            val pacer = PacedFramePacer(main, interval)
            val requested = System.nanoTime()
            repeat(3) { pacer.requestFrame(Runnable { ran.countDown() }) }
            assertTrue(ran.await(10, TimeUnit.SECONDS), "every requested frame ran")
            assertEq(listOf("undra-frame"), posts.map { it.first }.distinct())
            // Requests made within one interval all land on the same tick: no later than one interval after the request.
            val latest = posts.maxOf { it.second }
            assertTrue(latest - requested < interval + 200_000_000L, "posted ${(latest - requested) / 1_000_000} ms after the request")
        }

        case("a mirror without a pacer uses the paced default and drains on the main thread") {
            val main = ManualMainThread()
            val mirror = Mirror(main, MirrorOptions(), null)
            val seen = CopyOnWriteArrayList<UInt>()
            mirror.register(1L) { _, _, r -> seen.add(Codecs.u32.decode(r)) }
            mirror.submit(cs(full(1L, 0u, u32(1))))
            mirror.submit(cs(full(1L, 0u, u32(2))))
            eventually("the frame is posted to the main thread") { main.pending == 1 }
            main.runPending()
            assertEq(listOf(2u), seen.toList())
        }

        case("a pacer that throws is logged and the drain is posted to the main thread instead") {
            val main = ManualMainThread()
            val mirror = manualMirror(main, FramePacer { throw IllegalStateException("no display") })
            val seen = CopyOnWriteArrayList<UInt>()
            mirror.register(1L) { _, _, r -> seen.add(Codecs.u32.decode(r)) }
            LogCapture("dev.undra.runtime").use { log ->
                mirror.submit(cs(full(1L, 0u, u32(1))))
                assertTrue(log.records.any { it.thrown is IllegalStateException }, "logged")
            }
            assertEq(1, main.pending)
            main.runPending()
            assertEq(listOf(1u), seen.toList())
        }

        case("changes that arrive after the round cap are left to the next frame, never stranded") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            lateinit var mirror: Mirror
            mirror = manualMirror(main, pacer)
            val calls = AtomicInteger()
            // A callback that makes the core commit again every time it is applied: an endless loop of rounds.
            mirror.register(1L) { _, _, r ->
                val v = Codecs.u32.decode(r).toInt()
                calls.incrementAndGet()
                if (v < 1500) mirror.submit(cs(full(1L, 0u, u32(v + 1))))
            }
            mirror.submit(cs(full(1L, 0u, u32(0))))
            LogCapture("dev.undra.runtime").use { log ->
                pacer.frame()
                assertTrue(log.messages().any { it.contains("1000 rounds") }, "the cap is logged")
            }
            assertEq(Mirror.MAX_ROUNDS, calls.get())
            assertEq(1, pacer.pending, "the rest waits for the next frame")
            pacer.frame()
            assertEq(1501, calls.get())
        }
    }

    // ----- drain listener and counters -------------------------------------------------------------

    private fun countersAndListeners() {
        case("a drain listener hears each drain until it is closed") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            mirror.register(1L) { _, _, _ -> }
            val drains = CopyOnWriteArrayList<DrainStats>()
            val handle = mirror.addDrainListener { drains.add(it) }
            mirror.submit(cs(full(1L, 0u, u32(1)), full(1L, 1u, u32(1))))
            mirror.submit(cs(full(1L, 0u, u32(2))))
            mirror.flushOnThisThread(main)
            assertEq(1, drains.size)
            assertEq(2, drains[0].changeSets)
            assertEq(3, drains[0].entries)
            assertEq(2, drains[0].appliedEntries)
            assertTrue(!drains[0].duration.isNegative(), "duration ${drains[0].duration}")
            assertTrue(drains[0].toString().contains("appliedEntries=2"), drains[0].toString())
            handle.close()
            handle.close()
            mirror.submit(cs(full(1L, 0u, u32(3))))
            mirror.flushOnThisThread(main)
            assertEq(1, drains.size)
            val s = mirror.stats()
            assertEq(3L, s.changeSetsReceived)
            assertEq(4L, s.entriesReceived)
            assertEq(3L, s.entriesApplied)
            assertEq(2L, s.drains)
            assertEq(0L, s.compactions)
            assertEq(0L, s.resyncs)
            assertEq(0, s.pendingEntries)
            assertEq(0L, s.pendingBytes)
            assertEq(0L, s.droppedEntries)
        }

        case("a drain listener that throws is logged and does not stop the others") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            mirror.register(1L) { _, _, _ -> }
            val second = AtomicInteger()
            mirror.addDrainListener { throw IllegalStateException("listener failed") }
            mirror.addDrainListener { second.incrementAndGet() }
            LogCapture("dev.undra.runtime").use { log ->
                mirror.submit(cs(full(1L, 0u, u32(1))))
                mirror.flushOnThisThread(main)
                assertTrue(log.records.any { it.thrown is IllegalStateException }, "logged")
            }
            assertEq(1, second.get())
        }

        case("pending entries and bytes count each entry's value plus 17 bytes") {
            val main = ManualMainThread()
            val mirror = manualMirror(main, NEVER)
            mirror.submit(cs(full(1L, 0u, u32(1)), full(1L, 1u, ByteArray(10))))
            assertEq(2, mirror.stats().pendingEntries)
            assertEq((17L + 4) + (17L + 10), mirror.stats().pendingBytes)
            assertTrue(mirror.stats().toString().contains("pendingBytes=48"), mirror.stats().toString())
        }

        case("UndraCore.stats() carries the mirror's counters") {
            val t = FakeTransport()
            t.onObserve = { h, _, on -> if (on) t.events.onChangeSet(cs(full(h, 0u, u32(1)))) }
            attach(t).use { core ->
                core.mirror.register(5L) { _, _, _ -> }
                core.observe(5L, UInt.MAX_VALUE, true)
                val m = core.stats().mirror
                assertEq(1L, m.changeSetsReceived)
                assertEq(1L, m.entriesReceived)
                assertEq(1L, m.entriesApplied)
                assertEq(1L, m.drains)
                assertTrue(core.stats().toString().contains("mirror=MirrorStats("), core.stats().toString())
            }
            assertEq(0L, UndraStats(0).mirror.changeSetsReceived)
        }

        case("LoadOptions.mirror reaches the core's mirror") {
            val t = FakeTransport()
            val requests = AtomicInteger()
            val options = LoadOptions(
                expectedSchemaHash = HASH,
                defaultAdapters = false,
                mirror = MirrorOptions(framePacer = FramePacer { requests.incrementAndGet() }, maxPendingEntries = 4),
            )
            assertTrue(options.toString().contains("maxPendingEntries=4"), options.toString())
            UndraCore.attach(t, options, makeShared = false).use { core ->
                t.onCore { for (i in 0 until 10) t.events.onChangeSet(cs(full(1L, 0u, u32(i)))) }
                t.awaitCore()
                assertEq(1, requests.get())
                assertTrue(core.stats().mirror.compactions > 0, "compacted")
            }
        }

        case("MirrorOptions has the documented defaults and rejects bounds that are not positive") {
            val o = MirrorOptions()
            assertEq(null, o.framePacer)
            assertEq(65_536, o.maxPendingEntries)
            assertEq(16L * 1024 * 1024, o.maxPendingBytes)
            assertTrue(LoadOptions(expectedSchemaHash = 1uL).mirror.framePacer == null)
            assertThrows<IllegalArgumentException> { MirrorOptions(maxPendingEntries = 0) }
            assertThrows<IllegalArgumentException> { MirrorOptions(maxPendingBytes = -1) }
        }

        case("many threads enqueue while the main thread drains frame by frame: every final value arrives") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer, maxPendingEntries = 64)
            val last = Array(4) { AtomicInteger(-1) }
            for (h in 0 until 4) mirror.register(h + 1L) { _, _, r -> last[h].set(Codecs.u32.decode(r).toInt()) }
            val workers = List(4) { h ->
                Thread { for (i in 0 until 20_000) mirror.submit(cs(full(h + 1L, 0u, u32(i)))) }.also { it.start() }
            }
            while (workers.any { it.isAlive }) pacer.frame()
            workers.forEach { it.join(20_000) }
            pacer.frame()
            assertEq(List(4) { 19_999 }, last.map { it.get() })
            assertEq(0, mirror.stats().pendingEntries)
        }
    }

    @Test
    fun allCases() = assertPassed()
}
