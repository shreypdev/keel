package dev.undra.runtime

import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.ManualFramePacer
import dev.undra.runtime.support.ManualMainThread
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.changeSet
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.full
import dev.undra.runtime.support.invalidated
import dev.undra.runtime.support.flushOnThisThread
import dev.undra.runtime.support.manualMirror
import dev.undra.runtime.support.patch
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.logging.Level
import org.junit.jupiter.api.Test

private fun u32(v: Int): ByteArray = Codecs.u32.encodeToByteArray(v.toUInt())

/** What a registered callback saw: handle, signal, op, decoded value. */
private data class Applied(val handle: Long, val signal: UInt, val op: ChangeOp, val value: Int)

private class Recorder(private val mirror: Mirror) {
    val applied = CopyOnWriteArrayList<Applied>()
    val threads = CopyOnWriteArrayList<String>()

    fun register(handle: Long) {
        mirror.register(handle) { signal, op, reader ->
            threads.add(Thread.currentThread().name)
            val value = if (op == ChangeOp.INVALIDATED) -1 else Codecs.u32.decode(reader).toInt().also { reader.finish() }
            applied.add(Applied(handle, signal, op, value))
        }
    }
}

class MirrorTests : Suite() {
    init {
        case("change-sets are applied on the main thread, at the next frame, never on the thread that submitted them") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            val rec = Recorder(mirror)
            rec.register(1L)
            val submitter = Thread { mirror.submit(changeSet(1uL, full(1L, 0u, u32(5)))) }
            submitter.start()
            submitter.join()
            assertEq(0, rec.applied.size, "nothing is applied until the frame")
            assertEq(1, pacer.pending)
            assertEq(0, main.pending, "the frame pacer, not a main-thread post, schedules the drain")
            pacer.frame()
            assertEq(listOf(Applied(1L, 0u, ChangeOp.FULL, 5)), rec.applied.toList())
            assertEq(listOf(Thread.currentThread().name), rec.threads.toList())
        }

        case("change-sets that arrive before a frame are applied in one drain, in commit order") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            val rec = Recorder(mirror)
            rec.register(1L)
            rec.register(2L)
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1)), full(2L, 0u, u32(10))))
            mirror.submit(changeSet(2uL, full(1L, 1u, u32(2))))
            mirror.submit(changeSet(3uL, full(2L, 1u, u32(20)), full(1L, 2u, u32(3))))
            assertEq(1, pacer.requests.get(), "only the first submission asks for a frame")
            pacer.frame()
            assertEq(1L, mirror.stats().drains)
            assertEq(
                listOf(
                    Applied(1L, 0u, ChangeOp.FULL, 1),
                    Applied(2L, 0u, ChangeOp.FULL, 10),
                    Applied(1L, 1u, ChangeOp.FULL, 2),
                    Applied(2L, 1u, ChangeOp.FULL, 20),
                    Applied(1L, 2u, ChangeOp.FULL, 3),
                ),
                rec.applied.toList(),
            )
        }

        case("a later full value of the same signal before the frame makes earlier entries for it obsolete") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            val rec = Recorder(mirror)
            rec.register(1L)
            for (i in 1..50) mirror.submit(changeSet(i.toULong(), full(1L, 0u, u32(i)), full(1L, 1u, u32(1000 + i))))
            pacer.frame()
            assertEq(listOf(Applied(1L, 0u, ChangeOp.FULL, 50), Applied(1L, 1u, ChangeOp.FULL, 1050)), rec.applied.toList())
        }

        case("the patches after the last full value are merged into one patch applied after it") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val rec = Recorder(mirror)
            rec.register(1L)
            // FULL(1), PATCH, FULL(3), PATCH, PATCH: the first two are obsolete; the last two patches become one.
            // (These patches carry an op count and no ops: the merged count is the sum, 4 + 5.)
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1))))
            mirror.submit(changeSet(2uL, patch(1L, 0u, u32(2))))
            mirror.submit(changeSet(3uL, full(1L, 0u, u32(3))))
            mirror.submit(changeSet(4uL, patch(1L, 0u, u32(4))))
            mirror.submit(changeSet(5uL, patch(1L, 0u, u32(5))))
            mirror.flushOnThisThread(main)
            assertEq(
                listOf(
                    Applied(1L, 0u, ChangeOp.FULL, 3),
                    Applied(1L, 0u, ChangeOp.PATCH, 9),
                ),
                rec.applied.toList(),
            )
        }

        case("merging is per handle and per signal, in the order of each signal's first entry; an invalidation is kept") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val rec = Recorder(mirror)
            rec.register(1L)
            rec.register(2L)
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1)), full(2L, 0u, u32(2)), invalidated(1L, 3u)))
            mirror.submit(changeSet(2uL, full(1L, 0u, u32(3))))
            mirror.flushOnThisThread(main)
            assertEq(
                listOf(
                    Applied(1L, 0u, ChangeOp.FULL, 3),
                    Applied(2L, 0u, ChangeOp.FULL, 2),
                    Applied(1L, 3u, ChangeOp.INVALIDATED, -1),
                ),
                rec.applied.toList(),
            )
        }

        case("two full values of one signal are merged even inside one change-set") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val rec = Recorder(mirror)
            rec.register(1L)
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1)), full(1L, 0u, u32(2))))
            mirror.flushOnThisThread(main)
            assertEq(listOf(Applied(1L, 0u, ChangeOp.FULL, 2)), rec.applied.toList(), "the core never sends this, but the rule holds")
        }

        case("entries for handles that are not registered, or were unregistered meanwhile, are dropped") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val rec = Recorder(mirror)
            rec.register(1L)
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1)), full(99L, 0u, u32(2))))
            mirror.unregister(1L)
            mirror.submit(changeSet(2uL, full(1L, 0u, u32(3))))
            mirror.flushOnThisThread(main)
            assertEq(0, rec.applied.size)
            assertEq(0, mirror.registeredCount)
            assertEq(3L, mirror.stats().droppedEntries, "every dropped entry is counted, merged or not")
        }

        case("registering a handle again replaces the callback") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val first = CopyOnWriteArrayList<Int>()
            val second = CopyOnWriteArrayList<Int>()
            mirror.register(1L) { _, _, r -> first.add(Codecs.u32.decode(r).toInt()) }
            mirror.register(1L) { _, _, r -> second.add(Codecs.u32.decode(r).toInt()) }
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(7))))
            mirror.flushOnThisThread(main)
            assertEq(emptyList<Int>(), first.toList())
            assertEq(listOf(7), second.toList())
        }

        case("a callback that throws is logged and skipped; the rest of the batch is still applied") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val rec = Recorder(mirror)
            rec.register(2L)
            mirror.register(1L) { _, _, _ -> throw IllegalStateException("store bug") }
            mirror.register(3L) { _, _, _ -> throw NotImplementedError("TODO() in generated code") }
            LogCapture("dev.undra.runtime").use { log ->
                mirror.submit(changeSet(1uL, full(1L, 0u, u32(1)), full(3L, 0u, u32(1)), full(2L, 0u, u32(2))))
                mirror.submit(changeSet(2uL, full(2L, 1u, u32(3))))
                mirror.flushOnThisThread(main)
                assertEq(listOf(Applied(2L, 0u, ChangeOp.FULL, 2), Applied(2L, 1u, ChangeOp.FULL, 3)), rec.applied.toList())
                assertEq(2, log.records.count { it.level == Level.WARNING })
                assertTrue(log.records.any { it.thrown is IllegalStateException }, "the failure is logged with its exception")
            }
        }

        case("a malformed change-set is dropped as a whole; the ones around it still apply") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val rec = Recorder(mirror)
            rec.register(1L)
            val good = changeSet(1uL, full(1L, 0u, u32(1)))
            val truncated = good.copyOf(good.size - 2)
            val trailing = good + byteArrayOf(0)
            val badOp = good.copyOf().also { it[8 + 4 + 8 + 4] = 9 } // the op byte of the first entry
            val hostileCount = ByteArray(12).also { it[8] = -1; it[9] = -1; it[10] = -1; it[11] = 0x7f }
            LogCapture("dev.undra.runtime").use { log ->
                mirror.submit(changeSet(0uL, full(1L, 0u, u32(100))))
                for (bad in listOf(truncated, trailing, badOp, hostileCount, NO_BYTES, byteArrayOf(1))) mirror.submit(bad)
                mirror.submit(changeSet(2uL, full(1L, 0u, u32(200))))
                mirror.flushOnThisThread(main)
                // The first and last are applied (merged into the last, since both are FULL of the same signal).
                assertEq(listOf(Applied(1L, 0u, ChangeOp.FULL, 200)), rec.applied.toList())
                assertEq(6, log.records.count { it.level == Level.WARNING && it.message.contains("malformed") })
            }
        }

        case("the reader handed to a callback covers exactly the entry's value") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val seen = CopyOnWriteArrayList<Triple<Int, Int, Boolean>>()
            mirror.register(1L) { signal, _, reader ->
                val remaining = reader.remaining
                val value = Codecs.string.decode(reader)
                seen.add(Triple(signal.toInt(), remaining, value.isNotEmpty()))
                reader.finish() // would throw TrailingBytes if the reader ran into the next entry
            }
            val a = Codecs.string.encodeToByteArray("héllo")
            val b = Codecs.string.encodeToByteArray("")
            mirror.submit(changeSet(1uL, full(1L, 0u, a), full(1L, 1u, b), full(1L, 2u, a)))
            mirror.flushOnThisThread(main)
            assertEq(listOf(Triple(0, a.size, true), Triple(1, b.size, false), Triple(2, a.size, true)), seen.toList())
        }

        case("a callback may read the value after the batch's other entries were parsed (no shared reader state)") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val readers = ArrayList<UndraReader>()
            mirror.register(1L) { _, _, reader -> readers.add(reader) }
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1)), full(1L, 1u, u32(2))))
            mirror.flushOnThisThread(main)
            assertEq(1u, Codecs.u32.decode(readers[0]))
            assertEq(2u, Codecs.u32.decode(readers[1]))
        }

        case("awaitApplied on the main thread applies pending change-sets right away") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            val rec = Recorder(mirror)
            rec.register(1L)
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1))))
            main.mainThread = Thread.currentThread() // pretend this is the main thread
            assertTrue(mirror.awaitApplied(1000))
            assertEq(1, rec.applied.size)
            main.mainThread = null
            pacer.frame() // the frame that was requested finds nothing left
            assertEq(1, rec.applied.size)
            assertEq(1L, mirror.stats().drains, "a frame that finds nothing is not a drain")
        }

        case("awaitApplied off the main thread waits for the main thread, and gives up after its timeout") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val rec = Recorder(mirror)
            rec.register(1L)
            assertTrue(mirror.awaitApplied(10), "with nothing submitted there is nothing to wait for")
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1))))
            val started = System.nanoTime()
            assertEq(false, mirror.awaitApplied(120), "the main thread never runs, so the wait times out")
            assertTrue(System.nanoTime() - started >= 100_000_000L, "it did wait")
            val result = CopyOnWriteArrayList<Boolean>()
            val waiter = Thread { result.add(mirror.awaitApplied(10_000)) }
            waiter.start()
            Thread.sleep(50)
            assertEq(0, result.size, "still waiting")
            assertTrue(main.pending > 0, "the waiter asked the main thread for an immediate drain, not a frame")
            main.runPending()
            waiter.join(10_000)
            assertEq(listOf(true), result.toList())
            assertEq(1, rec.applied.size)
        }

        case("awaitApplied called from inside a callback does not re-enter the batch, so order is kept") {
            val main = ManualMainThread()
            val mirror = manualMirror(main)
            val order = CopyOnWriteArrayList<Int>()
            mirror.register(1L) { _, _, r ->
                val v = Codecs.u32.decode(r).toInt()
                order.add(v)
                if (v == 1) {
                    // Like a store re-observing a desynchronised signal: the core answers with a newer change-set.
                    mirror.submit(changeSet(9uL, full(1L, 5u, u32(99))))
                    // Must neither apply the newer change-set before the rest of this batch nor report a timeout.
                    assertTrue(mirror.awaitApplied(1000))
                    order.add(-1)
                }
            }
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1)), full(1L, 1u, u32(2)), full(1L, 2u, u32(3))))
            mirror.flushOnThisThread(main)
            assertEq(listOf(1, -1, 2, 3, 99), order.toList())
            assertEq(1L, mirror.stats().drains, "the nested change-set is a further round of the same drain")
        }

        case("a burst of distinct signals is applied completely, in arrival order, in one drain") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            val rec = Recorder(mirror)
            rec.register(1L)
            // Distinct signals, so nothing is merged away.
            val n = 5000
            for (i in 0 until n) mirror.submit(changeSet(i.toULong(), full(1L, i.toUInt(), u32(i))))
            pacer.frame()
            assertEq(n, rec.applied.size)
            assertEq((0 until n).toList(), rec.applied.map { it.value })
            assertEq(1L, mirror.stats().drains)
        }

        case("submissions racing from many threads are all applied, each thread's in order") {
            val main = ManualMainThread()
            val pacer = ManualFramePacer(main)
            val mirror = manualMirror(main, pacer)
            val perThread = 2000
            val threads = 4
            val seen = Array(threads) { CopyOnWriteArrayList<Int>() }
            for (t in 0 until threads) mirror.register(t.toLong() + 1) { _, _, r -> seen[t].add(Codecs.u32.decode(r).toInt()) }
            val start = CountDownLatch(1)
            val workers = List(threads) { t ->
                Thread {
                    start.await()
                    // A signal per change-set, so that merging keeps every one of them.
                    for (i in 0 until perThread) mirror.submit(changeSet(i.toULong(), full(t.toLong() + 1, i.toUInt(), u32(i))))
                }.also { it.start() }
            }
            start.countDown()
            workers.forEach { it.join(20_000) }
            // Run frames until everything has arrived (a frame is requested whenever entries wait).
            var guard = 0
            while (seen.sumOf { it.size } < perThread * threads && guard++ < 1000) pacer.frame()
            assertEq(0, pacer.pending, "no frame is left outstanding once the queue is empty")
            for (t in 0 until threads) assertEq((0 until perThread).toList(), seen[t].toList(), "thread $t")
        }

        case("the default mirror applies on the undra-main thread, paced by the undra-frame thread") {
            val mirror = Mirror()
            val names = CopyOnWriteArrayList<String>()
            mirror.register(1L) { _, _, r ->
                Codecs.u32.decode(r)
                names.add(Thread.currentThread().name)
            }
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1))))
            eventually("the change is applied") { names.isNotEmpty() }
            assertEq(listOf("undra-main"), names.toList())
        }

        case("awaitApplied on the real main thread applies inline; from another thread it waits for it") {
            val mirror = Mirror()
            val names = CopyOnWriteArrayList<String>()
            mirror.register(1L) { _, _, r ->
                Codecs.u32.decode(r)
                names.add(Thread.currentThread().name)
            }
            mirror.submit(changeSet(1uL, full(1L, 0u, u32(1))))
            assertTrue(mirror.awaitApplied(10_000))
            assertEq(listOf("undra-main"), names.toList())
            val done = CountDownLatch(1)
            UndraDispatchers.main.dispatch(kotlin.coroutines.EmptyCoroutineContext, Runnable {
                mirror.submit(changeSet(2uL, full(1L, 0u, u32(2))))
                names.add("before-await:" + UndraDispatchers.isMainThread())
                mirror.awaitApplied(10_000)
                names.add("after-await")
                done.countDown()
            })
            assertTrue(done.await(10, TimeUnit.SECONDS))
            // Inline on undra-main: the callback ran between "before-await" and "after-await".
            assertEq(listOf("undra-main", "before-await:true", "undra-main", "after-await"), names.toList())
        }
    }

    @Test
    fun allCases() = assertPassed()
}
