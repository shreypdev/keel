package dev.undra.runtime

import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.ManualMainThread
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.changeSet
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.flushOnThisThread
import dev.undra.runtime.support.full
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import kotlin.time.Duration.Companion.seconds
import org.junit.jupiter.api.Test

/*
 * One wrapper per handle (ADR-040): `UndraCore.adopt` and its helpers, closing and finalizers that give back exactly
 * the reference a wrapper owns, the refusal of another core's object, and a measured micro-benchmark of adopt.
 */

/** A plain object, as generated: the bindings' constructor, made only through `adopt`. */
private class Box internal constructor(core: UndraCore, handle: Long) : UndraObject(core, handle)

/** A store with one `u32` signal that records what it is given. */
private class Shelf internal constructor(core: UndraCore, handle: Long) : UndraStore(core, handle) {
    val seen = CopyOnWriteArrayList<UInt>()

    override fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader) {
        seen.add(Codecs.u32.decode(reader))
        reader.finish()
    }
}

private val NEVER = FramePacer { }

private fun u32(v: Int): ByteArray = Codecs.u32.encodeToByteArray(v.toUInt())

private fun handles(vararg raw: Long): ByteArray = Codecs.vec(Codecs.handle).encodeToByteArray(raw.toList())

/** A transport that only counts what the benchmark makes it do. */
@OptIn(UndraEmbeddingApi::class)
private class CountingTransport : Transport {
    val releases = AtomicLong()
    override val mode: Mode get() = Mode.INPROC
    override val isSynchronous: Boolean get() = true
    override fun connect(events: TransportEvents, expectedSchemaHash: ULong): ULong = expectedSchemaHash
    override fun call(payload: ByteArray): Int = 0
    override fun callSync(payload: ByteArray): ByteArray = throw UnsupportedOperationException()
    override fun cancel(callId: UInt) = Unit
    override fun streamCredit(callId: UInt, credit: UInt) = Unit
    override fun observe(handle: Long, signalId: UInt, on: Boolean) = Unit
    override fun release(handle: Long) {
        releases.incrementAndGet()
    }
    override fun portReply(payload: ByteArray) = Unit
    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) = Unit
    override fun timerFired(timerId: UInt) = Unit
    override fun snapshot(): ByteArray = NO_BYTES
    override fun restore(snapshot: ByteArray): Int = 0
    override fun statsJson(): String? = null
    override fun close() = Unit
}

class ObjectIdentityTests : Suite() {
    private fun core(t: FakeTransport = FakeTransport(), main: ManualMainThread = ManualMainThread()): ConnectedCore {
        val core = ConnectedCore(t, 5.seconds, mirrorOptions = MirrorOptions(framePacer = NEVER), main = main)
        t.connect(core, HASH)
        return core
    }

    init {
        case("a handle adopted again while its wrapper lives is that wrapper, and the extra reference is given back at once") {
            val t = FakeTransport()
            core(t).use { core ->
                val a = core.adopt(Handle.make(1u, 1u).raw, ::Box)
                val b = core.adopt(Handle.make(1u, 1u).raw, ::Box)
                assertTrue(a === b, "one wrapper per handle")
                assertEq(a, b)
                assertEq(listOf(Handle.make(1u, 1u).raw), t.releases.toList(), "the duplicate's reference, once")
                val other = core.adopt(Handle.make(2u, 1u).raw, ::Box)
                assertTrue(other !== a)
                assertEq(1, t.releases.size)
            }
        }

        case("a store returned twice is mirrored and observed once, and its change-sets reach the one wrapper") {
            val t = FakeTransport()
            val main = ManualMainThread()
            core(t, main).use { core ->
                val h = Handle.make(3u, 1u).raw
                val shelf = core.adopt(h, ::Shelf)
                assertTrue(core.adopt(h, ::Shelf) === shelf)
                assertEq(1, t.observes.count { it.first == h && it.third }, "observed when the wrapper was made, once")
                assertEq(1, core.mirror.registeredCount, "mirrored once")
                t.events.onChangeSet(changeSet(1uL, full(h, 0u, u32(2))))
                core.mirror.flushOnThisThread(main)
                assertEq(listOf(2u), shelf.seen.toList(), "one change-set, one apply")
            }
        }

        case("closing gives back exactly the wrapper's reference; the next adoption makes a new wrapper") {
            val t = FakeTransport()
            core(t).use { core ->
                val h = Handle.make(4u, 1u).raw
                val first = core.adopt(h, ::Shelf)
                core.adopt(h, ::Shelf) // a duplicate: given back at once
                assertEq(1, t.releases.size)
                first.close()
                first.close()
                assertEq(2, t.releases.size, "close gives back one reference, once")
                assertEq(0, core.mirror.registeredCount, "no open wrapper: routing stops")
                val again = core.adopt(h, ::Shelf)
                assertTrue(again !== first, "a closed wrapper is replaced")
                assertTrue(!again.isClosed)
                assertEq(1, core.mirror.registeredCount)
            }
        }

        case("a reference given back while an open wrapper owns the handle leaves its routing alone") {
            val t = FakeTransport()
            val main = ManualMainThread()
            core(t, main).use { core ->
                val h = Handle.make(5u, 1u).raw
                val shelf = core.adopt(h, ::Shelf)
                // What a dead wrapper's cleaner, or a duplicate, does: one more release of the same handle.
                core.release(h)
                assertEq(1, core.mirror.registeredCount)
                t.events.onChangeSet(changeSet(2uL, full(h, 0u, u32(9))))
                core.mirror.flushOnThisThread(main)
                assertEq(listOf(9u), shelf.seen.toList())
            }
        }

        case("a wrapper that is collected without close is released once by its cleaner, and its entry goes") {
            val t = FakeTransport()
            core(t).use { core ->
                val h = Handle.make(6u, 1u).raw
                fun adoptAndDrop() {
                    core.adopt(h, ::Box)
                }
                adoptAndDrop()
                eventually("the cleaner released the collected wrapper", timeoutMs = 20_000) {
                    System.gc()
                    t.releases.contains(h)
                }
                Thread.sleep(50)
                System.gc()
                Thread.sleep(50)
                assertEq(1, t.releases.count { it == h }, "at most once")
                eventually("the identity entry is cleaned") { core.identity.size == 0 }
                val fresh = core.adopt(h, ::Box)
                assertEq(1, t.releases.count { it == h }, "a new wrapper owns the new reference")
                fresh.close()
                assertEq(2, t.releases.count { it == h })
            }
        }

        case("closed and then collected releases once (the finalizer runs at most once)") {
            val t = FakeTransport()
            core(t).use { core ->
                val h = Handle.make(7u, 1u).raw
                fun adoptAndClose() {
                    core.adopt(h, ::Box).close()
                }
                adoptAndClose()
                repeat(5) {
                    System.gc()
                    Thread.sleep(20)
                }
                assertEq(1, t.releases.count { it == h })
            }
        }

        case("adoptObject, adoptOptional and adoptList read the reply body and adopt each handle as it is read") {
            val t = FakeTransport()
            core(t).use { core ->
                val one = core.adoptObject(Codecs.handle.encodeToByteArray(11L), ::Box)
                assertEq(11L, one.handle)
                assertEq(null, core.adoptOptional(Codecs.option(Codecs.handle).encodeToByteArray(null), ::Box))
                assertTrue(core.adoptOptional(Codecs.option(Codecs.handle).encodeToByteArray(11L), ::Box) === one)
                val list = core.adoptList(handles(11L, 12L, 11L), ::Box)
                assertEq(listOf(11L, 12L, 11L), list.map { it.handle })
                assertTrue(list[0] === one && list[2] === one, "a handle listed twice is one wrapper")
                assertEq(listOf(11L, 11L, 11L), t.releases.toList(), "every duplicate crossing given back")
                assertThrows<WireException> { core.adoptObject(ByteArray(3), ::Box) }
                assertThrows<UndraProtocolException> { core.adopt(0L, ::Box) }
                assertTrue(list[1].handle == 12L)
            }
        }

        case("requireOwn refuses an object of another core, naming its class, and lets its own and null pass") {
            core().use { a ->
                core().use { b ->
                    val mine = a.adopt(21L, ::Box)
                    val theirs = b.adopt(21L, ::Box) // the same handle number in another core
                    a.requireOwn(mine)
                    a.requireOwn(null)
                    a.requireOwn(listOf(mine))
                    val refused = assertThrows<UndraCallError.Refused> { a.requireOwn(theirs) }
                    assertTrue(refused.reason.contains("Box") && refused.reason.contains("another core"), refused.reason)
                    assertThrows<UndraCallError.Refused> { a.requireOwn(listOf(mine, theirs)) }
                }
            }
        }

        case("a wrapper whose making fails gives its reference back") {
            val t = FakeTransport()
            core(t).use { core ->
                assertThrows<IllegalStateException> { core.adopt(31L) { _, _ -> throw IllegalStateException("no") } }
                assertEq(listOf(31L), t.releases.toList())
                assertEq(0, core.identity.size)
            }
        }

        case("threads adopting the same handle at once share one wrapper and give back every other reference") {
            val t = FakeTransport()
            core(t).use { core ->
                val threads = 8
                val each = 200
                val pool = Executors.newFixedThreadPool(threads)
                val start = CountDownLatch(1)
                val seen = CopyOnWriteArrayList<Box>()
                repeat(threads) {
                    pool.execute {
                        start.await()
                        repeat(each) { seen.add(core.adopt(41L, ::Box)) }
                    }
                }
                start.countDown()
                pool.shutdown()
                assertTrue(pool.awaitTermination(20, TimeUnit.SECONDS))
                assertEq(1, seen.toSet().size, "one wrapper")
                assertEq(threads * each - 1, t.releases.count { it == 41L }, "every other crossing given back")
            }
        }

        case("stats report the host references and live callbacks of the core, and unknown when absent") {
            val t = FakeTransport()
            core(t).use { core ->
                t.statsJson = "{\"live_handles\":3,\"host_refs\":7,\"live_callbacks\":2}"
                val stats = core.stats()
                assertEq(7L, stats.hostRefs)
                assertEq(2, stats.liveCallbacks)
                t.statsJson = "{\"live_handles\":3}"
                assertEq(UndraStats.UNKNOWN.toLong(), core.stats().hostRefs)
            }
        }

        case("reachabilityFence takes anything, null included") {
            reachabilityFence(null)
            reachabilityFence(Any())
            reachabilityFence(UndraWriter())
        }

        case("bench: host-side adopt") {
            val transport = CountingTransport()
            val core = ConnectedCore(transport, 5.seconds, mirrorOptions = MirrorOptions(framePacer = NEVER), main = ManualMainThread())
            transport.connect(core, HASH)
            core.use {
                val live = core.adopt(1L, ::Box)
                fun duplicates(n: Int): Long {
                    val started = System.nanoTime()
                    for (i in 0 until n) core.adopt(1L, ::Box)
                    return System.nanoTime() - started
                }
                fun fresh(n: Int, base: Long): Long {
                    val started = System.nanoTime()
                    for (i in 0 until n) core.adopt(base + i, ::Box).close()
                    return System.nanoTime() - started
                }
                duplicates(50_000) // warm up
                fresh(20_000, 1_000_000L)
                val n = 200_000
                val dup = duplicates(n).toDouble() / n
                val made = fresh(n, 10_000_000L).toDouble() / n
                println("BENCH kotlin host adopt (live wrapper, extra reference given back): ${"%.1f".format(dup)} ns/op")
                println("BENCH kotlin host adopt (new wrapper, then close): ${"%.1f".format(made)} ns/op")
                assertTrue(live.handle == 1L)
                assertEq((n + 50_000).toLong() + (n + 20_000).toLong(), transport.releases.get())
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
