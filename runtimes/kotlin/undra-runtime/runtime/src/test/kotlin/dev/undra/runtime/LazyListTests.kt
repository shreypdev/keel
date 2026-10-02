package dev.undra.runtime

import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.changeSet
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.full
import dev.undra.runtime.support.lazyInvalidated
import dev.undra.runtime.support.lazyValue
import dev.undra.runtime.support.replyPayload
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.ConcurrentLinkedQueue
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.StateFlow
import org.junit.jupiter.api.Test

/** The main thread's turns, made by hand: what [UndraLazyList] posts waits until [run]. */
private class LazyTurns {
    private val tasks = ConcurrentLinkedQueue<Runnable>()
    val posted = AtomicInteger()

    fun post(task: Runnable) {
        posted.incrementAndGet()
        tasks.add(task)
    }

    val pending: Int get() = tasks.size

    /** Runs what is posted (and what that posts) on the calling thread. */
    fun run() {
        while (true) (tasks.poll() ?: break).run()
    }
}

/** The core's page server: [total] rows, row `i` holding `base + i`; every page request is recorded. */
private class LazyPageServer(var total: Int, var version: ULong = 1uL, var base: Int = 0) {
    val requests = CopyOnWriteArrayList<CallTarget.LazyListPage>()

    /** Answers with these bytes (the reply body) instead of a page. */
    @Volatile var body: ((CallTarget.LazyListPage) -> ByteArray)? = null

    /** Answers with this status (and the body above) when set. */
    @Volatile var status: ReplyStatus = ReplyStatus.OK

    fun row(i: Int): Int = base + i

    fun page(offset: Int, limit: Int, version: ULong = this.version, total: Int = this.total): ByteArray {
        val count = maxOf(0, minOf(limit, total - offset))
        val w = UndraWriter()
        Payloads.LazyPageHeader(version, total.toUInt(), count.toUInt()).encode(w)
        for (i in 0 until count) Codecs.i32.encode(w, row(offset + i))
        return w.toByteArray()
    }

    /** A whole `Reply` payload, as the transport hands it back. */
    fun answer(call: Payloads.Call): ByteArray {
        val target = call.target as CallTarget.LazyListPage
        requests.add(target)
        val bytes = body?.invoke(target) ?: page(target.offset.toInt(), target.limit.toInt())
        return replyPayload(call.callId, status, bytes)
    }

    /** The page numbers requested so far, in order, for a page size of 50. */
    fun pages(size: Int = 50): List<Int> = requests.map { it.offset.toInt() / size }
}

/** A core over a fake transport, a page server behind it and a list whose turns are [turns]. */
private class LazyRig(val sync: Boolean = true, total: Int = 1000) : AutoCloseable {
    /** The runtime's log, kept out of the test output (what a report logs is checked where it matters). */
    val log = LogCapture("dev.undra.runtime")
    val server = LazyPageServer(total)
    val transport = FakeTransport(isSynchronous = sync)
    val errors = CopyOnWriteArrayList<UndraUnhandledError>()
    val core: UndraCore = UndraCore.attach(
        transport,
        LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false, onError = { errors.add(it) }),
        makeShared = false,
    )
    val turns = LazyTurns()
    val list = UndraLazyList(core, Codecs.i32, turns::post) { Dispatchers.Unconfined }

    /** Asynchronous calls wait here instead of being answered, while [hold] is set. */
    val held = CopyOnWriteArrayList<Payloads.Call>()
    @Volatile var hold = false

    init {
        transport.onCallSync = server::answer
        transport.onCall = { call -> if (hold) held.add(call) else answerLater(call) }
    }

    private fun answerLater(call: Payloads.Call) {
        val reply = Payloads.Reply.decode(server.answer(call))
        transport.replyOnCore(call.callId, reply.status, reply.body)
    }

    /** Answers a held call now (the core's reply, with the server's current state). */
    fun release(call: Payloads.Call) {
        held.remove(call)
        answerLater(call)
    }

    fun value(handle: Long = 7L, len: Int = server.total, version: ULong = server.version): UndraReader =
        UndraReader(Payloads.LazyValue(Handle(handle), len.toUInt(), version).toByteArray())

    fun full(handle: Long = 7L, len: Int = server.total, version: ULong = server.version) = list.applyFull(value(handle, len, version))

    fun invalidated(len: Int, version: ULong) =
        list.applyInvalidated(UndraReader(Payloads.LazyInvalidated(len.toUInt(), version).toByteArray()))

    /** The core changes the list: the server moves to the next version (and `base`), then the op 2 reaches the list. */
    fun change(len: Int = server.total, base: Int = server.base) {
        server.version = server.version + 1uL
        server.total = len
        server.base = base
        invalidated(len, server.version)
    }

    /** Waits until the asynchronous replies sent so far have been handled by the list. */
    fun settle() {
        transport.awaitCore()
        val latch = CountDownLatch(1)
        UndraDispatchers.delivery.execute { latch.countDown() }
        assertTrue(latch.await(10, TimeUnit.SECONDS), "the delivery thread did not catch up")
    }

    /** Runs the turn and, over an asynchronous transport, waits for its replies. */
    fun turn() {
        turns.run()
        if (!sync) settle()
    }

    override fun close() {
        list.close()
        core.close()
        log.close()
    }
}

private fun <T> StateFlow<T>.now(): T = value

/** The two-step item of a store, shaped like generated code (signal 3 is the lazy list). */
private class LazyLibraryStore(core: UndraCore, handle: Long) : UndraStore(core, handle) {
    val books = UndraLazyList(core, Codecs.i32)
    val seen = CopyOnWriteArrayList<String>()

    init {
        core.observe(handle, UInt.MAX_VALUE, true)
    }

    override fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader) {
        when (signalId) {
            3u -> when (op) {
                ChangeOp.FULL -> books.applyFull(reader).also { seen.add("full on " + Thread.currentThread().name) }
                ChangeOp.INVALIDATED -> books.applyInvalidated(reader).also { seen.add("invalidated on " + Thread.currentThread().name) }
                ChangeOp.PATCH -> Unit
            }
            else -> Unit
        }
    }
}

class LazyListTests : Suite() {
    init {
        // ---- reading --------------------------------------------------------------------------------

        case("a new list is empty and reading it requests nothing") {
            LazyRig().use { r ->
                assertEq(0, r.list.size.now())
                assertEq(0L, r.list.revision.now())
                assertEq(null, r.list[0])
                assertEq(0, r.turns.posted.get())
                assertEq(0, r.transport.syncCalls.size + r.transport.calls.size)
            }
        }

        case("the first value sets the size and bumps the revision, and requests nothing until a row is read") {
            LazyRig(total = 1000).use { r ->
                r.full()
                assertEq(1000, r.list.size.now())
                assertEq(1L, r.list.revision.now())
                r.turn()
                assertEq(0, r.server.requests.size)
            }
        }

        case("a row is null while its page loads; reading requests its page and one page on each side, once, in the next turn") {
            LazyRig().use { r ->
                r.full()
                assertEq(null, r.list[120])
                assertEq(1, r.turns.pending)
                assertEq(0, r.server.requests.size) // reading does not call: the request goes out in the next turn
                r.turn()
                assertEq(listOf(1, 2, 3), r.server.pages())
                assertEq(r.server.row(120), r.list[120])
                assertEq(r.server.row(100), r.list[100])
                assertEq(r.server.row(149), r.list[149])
                assertEq(3, r.server.requests.size) // all cached: nothing more was asked
                assertEq(0, r.turns.pending)
            }
        }

        case("a page request is a LazyListPage call to the page server with the page's offset and the page size") {
            LazyRig().use { r ->
                r.full(handle = 0x0000_0100_0000_0003L)
                r.list.pageSize = 20
                r.full(handle = 0x0000_0100_0000_0003L, len = 1000, version = 1uL) // the same handle: nothing new
                r.list[45]
                r.turn()
                val targets = r.server.requests
                assertEq(listOf(CallTarget.LazyListPage(Handle(0x0000_0100_0000_0003L), 20u, 20u), CallTarget.LazyListPage(Handle(0x0000_0100_0000_0003L), 40u, 20u), CallTarget.LazyListPage(Handle(0x0000_0100_0000_0003L), 60u, 20u)), targets.toList())
            }
        }

        case("the first page has no neighbour before it and the last none after it") {
            LazyRig(total = 120).use { r ->
                r.full()
                r.list[0]
                r.turn()
                assertEq(listOf(0, 1), r.server.pages())
                r.list[119]
                r.turn()
                assertEq(listOf(0, 1, 2), r.server.pages()) // page 1 is cached; the last page is 2
                assertEq(r.server.row(119), r.list[119])
                assertEq(null, r.list[120])
            }
        }

        case("reads in one turn are coalesced into one posted flush, and every page goes out once") {
            LazyRig().use { r ->
                r.full()
                for (i in 0 until 300) r.list[i]
                assertEq(1, r.turns.posted.get())
                r.turn()
                assertEq((0..6).toList(), r.server.pages().sorted())
                assertEq(7, r.server.requests.size)
                for (i in 0 until 300) assertEq(r.server.row(i), r.list[i])
                assertEq(1, r.turns.posted.get())
            }
        }

        case("a cached page that is current is not requested again, and reading it posts nothing") {
            LazyRig().use { r ->
                r.full()
                r.list[10]
                r.turn()
                val posted = r.turns.posted.get()
                assertEq(r.server.row(10), r.list[10])
                assertEq(r.server.row(20), r.list[20])
                assertEq(posted, r.turns.posted.get())
                assertEq(2, r.server.requests.size)
            }
        }

        case("an index outside the list returns null and requests nothing") {
            LazyRig(total = 100).use { r ->
                r.full()
                for (i in listOf(-1, 100, 101, Int.MAX_VALUE, Int.MIN_VALUE)) assertEq(null, r.list[i])
                assertEq(0, r.turns.posted.get())
                r.turn()
                assertEq(0, r.server.requests.size)
                assertEq(emptyList<Int>(), r.list.windowPages())
            }
        }

        case("an empty list returns null for every index") {
            LazyRig(total = 0).use { r ->
                r.full()
                assertEq(null, r.list[0])
                assertEq(0, r.turns.posted.get())
            }
        }

        case("reading before the first value requests nothing") {
            LazyRig().use { r ->
                assertEq(null, r.list[3])
                assertEq(0, r.turns.posted.get())
            }
        }

        case("prefetch requests the pages of a range, clamped to the list, and no more than maxCachedPages of them") {
            LazyRig(total = 1000).use { r ->
                r.full()
                r.list.prefetch(120..230)
                r.turn()
                assertEq(listOf(2, 3, 4), r.server.pages())
                r.list.prefetch(-50..60)
                r.list.prefetch(990..5000)
                r.list.prefetch(2000..3000)
                r.list.prefetch(10..5)
                r.turn()
                assertEq(listOf(2, 3, 4, 0, 1, 19), r.server.pages())
            }
            LazyRig(total = 1000).use { r ->
                r.list.maxCachedPages = 3
                r.full()
                r.list.prefetch(0..999)
                r.turn()
                assertEq(listOf(0, 1, 2), r.server.pages()) // 20 pages in the range, 3 kept
            }
        }

        case("prefetch asks again for a page whose request failed; reading does not") {
            LazyRig().use { r ->
                r.full()
                r.server.status = ReplyStatus.BAD_REQUEST
                r.list[0]
                r.turn()
                val failures = r.errors.size
                assertEq(2, failures) // pages 0 and 1
                r.list[0]
                r.list[10]
                r.turn()
                assertEq(2, r.server.requests.size) // reading does not retry what failed
                r.server.status = ReplyStatus.OK
                r.list.prefetch(0..0)
                r.turn()
                assertEq(3, r.server.requests.size)
                assertEq(r.server.row(0), r.list[0])
            }
        }

        case("pageSize is validated, and changing it drops the cache and asks for the rows again") {
            LazyRig().use { r ->
                assertEq(50, r.list.pageSize)
                assertThrows<IllegalArgumentException> { r.list.pageSize = 0 }
                assertThrows<IllegalArgumentException> { r.list.pageSize = 65_537 }
                r.full()
                r.list[0]
                r.turn()
                val before = r.list.revision.now()
                r.list.pageSize = 100
                assertEq(100, r.list.pageSize)
                assertTrue(r.list.revision.now() > before)
                assertEq(emptyList<Int>(), r.list.cachedPages())
                assertEq(null, r.list[0])
                r.turn()
                assertEq(r.server.row(0), r.list[0])
                assertEq(r.server.row(199), r.list[199])
                assertEq(100u, r.server.requests.last().limit)
            }
        }

        case("maxCachedPages is validated and evicts at once when lowered") {
            LazyRig().use { r ->
                assertEq(24, r.list.maxCachedPages)
                assertThrows<IllegalArgumentException> { r.list.maxCachedPages = 0 }
                r.full()
                r.list.prefetch(0..399)
                r.turn()
                assertEq(8, r.list.cachedPages().size)
                r.list.maxCachedPages = 3
                assertEq(3, r.list.cachedPages().size)
                assertTrue(r.list.windowPages().size <= 3)
            }
        }

        // ---- eviction -------------------------------------------------------------------------------

        case("at most maxCachedPages pages are kept: the least recently read go first") {
            LazyRig().use { r ->
                r.list.maxCachedPages = 4
                r.full()
                r.list[0]
                r.turn() // pages 0, 1
                r.list[100]
                r.turn() // pages 2, 3 (1 is cached)
                assertEq(listOf(0, 1, 2, 3), r.list.cachedPages().sorted())
                r.list[0] // page 0 and 1 are read again: the oldest are now 2 and 3
                r.list[250]
                r.turn() // pages 4, 5, 6 arrive
                val cached = r.list.cachedPages()
                assertEq(4, cached.size)
                assertTrue(5 in cached, "the page of the row just read stays")
                assertEq(r.server.row(250), r.list[250])
            }
        }

        case("a stale page outside the window is evicted before a page that was read since the last change") {
            LazyRig(total = 2000).use { r ->
                r.list.maxCachedPages = 4
                r.full()
                r.list[0]
                r.list[100]
                r.turn() // pages 0..3
                r.change() // the window is cleared; its four pages are re-paged
                r.turn()
                assertEq(listOf(0, 1, 2, 3), r.list.cachedPages().sorted())
                assertEq(emptyList<Int>(), r.list.windowPages())
                r.list[1000] // pages 19, 20, 21 are read
                r.turn()
                val cached = r.list.cachedPages()
                assertEq(4, cached.size)
                assertTrue(cached.containsAll(listOf(19, 20, 21)), "the pages read since the change are kept: $cached")
            }
        }

        // ---- changes and versions -------------------------------------------------------------------

        case("an invalidation takes the new length at once, keeps the rows visible and re-pages the window, O(window)") {
            LazyRig(total = 100_000).use { r ->
                r.full()
                r.list[50_000]
                r.turn()
                assertEq(listOf(999, 1000, 1001), r.server.pages())
                r.change(len = 100_001, base = 1000)
                assertEq(100_001, r.list.size.now())
                assertEq(50_000, r.list[50_000]) // stale, but visible while the window is re-paged
                r.turn()
                assertEq(listOf(999, 1000, 1001, 999, 1000, 1001), r.server.pages()) // three more calls, never the list
                assertEq(51_000, r.list[50_000])
            }
        }

        case("a list nobody reads costs nothing: only the pages read since the previous invalidation are re-paged") {
            LazyRig(total = 10_000).use { r ->
                r.full()
                r.list[0]
                r.list[5000]
                r.turn()
                assertEq(5, r.server.requests.size)
                r.change()
                r.turn()
                assertEq(10, r.server.requests.size) // the window of five pages
                r.change() // nothing was read since: nothing to re-page
                r.turn()
                assertEq(10, r.server.requests.size)
                assertEq(3uL, r.list.currentVersion())
            }
        }

        case("an invalidation re-pages at most maxCachedPages pages however much was read") {
            LazyRig(total = 100_000).use { r ->
                r.list.maxCachedPages = 6
                r.full()
                for (page in 0 until 200 step 3) r.list[page * 50]
                r.turn()
                assertTrue(r.server.requests.size <= 6, "pages scrolled past within the turn are not requested: ${r.server.requests.size}")
                val before = r.server.requests.size
                r.change()
                r.turn()
                assertTrue(r.server.requests.size - before <= 6, "re-paged ${r.server.requests.size - before} pages")
                assertTrue(r.list.cachedPages().size <= 6)
            }
        }

        case("an invalidation that shrinks the list drops the pages past the end and the last page is re-paged") {
            LazyRig(total = 500).use { r ->
                r.full()
                r.list[480]
                r.list[0]
                r.turn()
                r.change(len = 120)
                assertEq(null, r.list[480])
                assertEq(120, r.list.size.now())
                assertTrue(r.list.cachedPages().all { it <= 2 }, "pages past the new end are gone: ${r.list.cachedPages()}")
                r.turn()
                r.list[119]
                r.turn()
                assertEq(r.server.row(119), r.list[119])
                assertEq(null, r.list[120])
            }
        }

        case("an invalidation that is not newer changes nothing") {
            LazyRig().use { r ->
                r.full()
                r.invalidated(1000, 5uL)
                val revision = r.list.revision.now()
                r.invalidated(1000, 5uL)
                r.invalidated(2000, 4uL)
                assertEq(revision, r.list.revision.now())
                assertEq(1000, r.list.size.now())
                assertEq(5uL, r.list.currentVersion())
            }
        }

        case("a page reply with a newer version raises the version and the length, and the invalidation that follows changes nothing") {
            LazyRig(total = 1000).use { r ->
                r.full()
                r.server.version = 2uL
                r.server.total = 1200
                r.list[0]
                r.turn()
                assertEq(1200, r.list.size.now())
                assertEq(2uL, r.list.currentVersion())
                assertEq(listOf(0, 1), r.server.pages())
                val revision = r.list.revision.now()
                r.invalidated(1200, 2uL)
                r.turn()
                assertEq(revision, r.list.revision.now())
                assertEq(2, r.server.requests.size)
                assertEq(r.server.row(60), r.list[60])
            }
        }

        case("a page reply raising the version re-pages the rest of the window") {
            LazyRig(total = 1000).use { r ->
                r.full()
                r.list[100]
                r.turn() // pages 1, 2, 3 at version 1
                r.server.version = 2uL
                r.server.base = 5000
                r.list.prefetch(0..0) // page 0, which is not cached: the reply says version 2
                r.turn()
                assertEq(2uL, r.list.currentVersion())
                // The window held pages 1, 2, 3 and 0: all but 0 are stale and asked for again.
                assertEq(listOf(1, 2, 3, 0, 1, 2, 3), r.server.pages())
                assertEq(5100, r.list[100])
            }
        }

        case("a reply older than the list's version is dropped and the page asked for again") {
            LazyRig(sync = false, total = 1000).use { r ->
                r.full()
                r.hold = true
                r.list[0]
                r.turn()
                assertEq(2, r.held.size) // pages 0 and 1, in flight
                r.invalidated(1000, 2uL)
                r.hold = false
                for (call in r.held.toList()) r.release(call) // answered at version 1, read before the change
                r.settle()
                assertEq(null, r.list[0]) // dropped: nothing is cached from a stale reply
                assertEq(emptyList<Int>(), r.list.cachedPages())
                assertEq(0, r.errors.size)
                r.server.version = 2uL
                r.server.base = 7000
                r.turn() // the pages are asked for again, at version 2
                assertEq(7000, r.list[0])
                assertEq(7050, r.list[50])
                assertEq(4, r.server.requests.size)
            }
        }

        case("a core that keeps answering with an old version is asked a few times, then reported, not forever") {
            LazyRig(total = 500).use { r ->
                r.full(version = 3uL)
                r.server.version = 2uL // every reply is older than the list
                r.list[0]
                for (i in 0 until 20) r.turn()
                val perPage = r.server.pages().groupBy { it }.mapValues { it.value.size }
                assertEq(UndraLazyList.MAX_STALE_REPLIES + 1, perPage.getValue(0))
                assertEq(2, r.errors.size)
                assertTrue(r.errors.all { it.error is UndraCallError.Malformed })
                assertEq(null, r.list[0])
            }
        }

        case("the same handle with a newer version or another length is an invalidation; the same value changes nothing") {
            LazyRig(total = 1000).use { r ->
                r.full()
                r.list[0]
                r.turn()
                val revision = r.list.revision.now()
                r.full(version = 1uL) // observed again, nothing new
                assertEq(revision, r.list.revision.now())
                r.server.version = 2uL
                r.server.total = 1010
                r.full(len = 1010, version = 2uL)
                assertEq(1010, r.list.size.now())
                assertTrue(r.list.revision.now() > revision)
                r.turn()
                assertEq(listOf(0, 1, 0, 1), r.server.pages())
                r.full(len = 900, version = 1uL) // older than what the list knows: ignored
                assertEq(1010, r.list.size.now())
            }
        }

        // ---- restart --------------------------------------------------------------------------------

        case("a value with a new page server (a restore) drops the cache and ignores what was in flight") {
            LazyRig(sync = false, total = 1000).use { r ->
                r.full(handle = 5L)
                r.list[0]
                r.turn()
                assertEq(r.server.row(0), r.list[0])
                r.hold = true
                r.list[500]
                r.turn()
                assertEq(3, r.held.size)
                r.full(handle = 6L, len = 40, version = 1uL)
                assertEq(40, r.list.size.now())
                assertEq(emptyList<Int>(), r.list.cachedPages())
                assertEq(null, r.list[0]) // the cache was dropped
                r.hold = false
                for (call in r.held.toList()) r.release(call) // the old page server's answers are ignored
                r.settle()
                assertEq(null, r.list[7])
                r.server.total = 40
                r.turn()
                assertEq(r.server.row(7), r.list[7])
                assertEq(Handle(6L), r.server.requests.last().handle)
                assertEq(0, r.errors.size)
            }
        }

        // ---- the transport --------------------------------------------------------------------------

        case("over an in-process core pages are fetched with callSync; over a remote one with the suspending call") {
            LazyRig(sync = true).use { r ->
                r.full()
                r.list[0]
                r.turn()
                assertEq(2, r.transport.syncCalls.size)
                assertEq(0, r.transport.calls.size)
                assertTrue(r.transport.syncCalls.all { it.target is CallTarget.LazyListPage })
            }
            LazyRig(sync = false).use { r ->
                r.full()
                r.list[0]
                r.turn()
                assertEq(2, r.transport.calls.size)
                assertEq(0, r.transport.syncCalls.size)
                assertEq(r.server.row(0), r.list[0])
            }
        }

        case("a page already in flight is not requested again; its reply fills it") {
            LazyRig(sync = false).use { r ->
                r.full()
                r.hold = true
                r.list[0]
                r.turn()
                r.list[0]
                r.list[10]
                r.turn()
                assertEq(2, r.held.size)
                assertEq(2, r.transport.calls.size)
                r.hold = false
                for (call in r.held.toList()) r.release(call)
                r.settle()
                assertEq(r.server.row(0), r.list[0])
                assertEq(r.server.row(60), r.list[60])
                assertEq(2, r.transport.calls.size)
            }
        }

        case("a version race over an asynchronous transport: the reply with the newer version wins") {
            LazyRig(sync = false, total = 1000).use { r ->
                r.full()
                r.hold = true
                r.list[0]
                r.turn()
                r.server.version = 2uL
                r.server.total = 1100
                r.hold = false
                for (call in r.held.toList()) r.release(call) // answered at version 2, before the op 2 arrives
                r.settle()
                assertEq(1100, r.list.size.now())
                val calls = r.server.requests.size
                r.invalidated(1100, 2uL)
                r.turn()
                assertEq(calls, r.server.requests.size)
                assertEq(r.server.row(0), r.list[0])
            }
        }

        // ---- hostile replies ------------------------------------------------------------------------

        val hostile = listOf<Pair<String, (CallTarget.LazyListPage, LazyPageServer) -> ByteArray>>(
            "an empty body" to { _, _ -> ByteArray(0) },
            "a truncated header" to { _, _ -> ByteArray(11) },
            "a page whose items are cut off" to { t, s -> s.page(t.offset.toInt(), t.limit.toInt()).let { it.copyOf(it.size - 3) } },
            "a count above the limit" to { t, s ->
                UndraWriter().also {
                    Payloads.LazyPageHeader(s.version, s.total.toUInt(), t.limit + 1u).encode(it)
                    for (i in 0..t.limit.toInt()) it.writeI32(i)
                }.toByteArray()
            },
            "a count of four billion" to { _, s -> UndraWriter().also { Payloads.LazyPageHeader(s.version, s.total.toUInt(), UInt.MAX_VALUE).encode(it) }.toByteArray() },
            "trailing bytes" to { t, s -> s.page(t.offset.toInt(), t.limit.toInt()) + byteArrayOf(1) },
            "fewer rows than the list has there" to { t, s -> s.page(t.offset.toInt(), 3) },
            "a total that is not the list's at the same version" to { t, s -> s.page(t.offset.toInt(), t.limit.toInt(), total = s.total + 7) },
            "a total that does not fit an Int" to { _, s -> UndraWriter().also { Payloads.LazyPageHeader(s.version, UInt.MAX_VALUE, 0u).encode(it) }.toByteArray() },
        )
        for ((name, make) in hostile) {
            case("a hostile reply ($name) is reported, leaves the rows null and never throws into get") {
                LazyRig(total = 400).use { r ->
                    r.full()
                    r.server.body = { t -> make(t, r.server) }
                    assertEq(null, r.list[120])
                    r.turn()
                    assertEq(3, r.errors.size, "one report per page asked: ${r.errors.map { it.message }}")
                    assertTrue(r.errors.all { it.error is UndraCallError.Malformed }, "typed errors: ${r.errors.map { it.error }}")
                    assertEq(null, r.list[120])
                    assertEq(emptyList<Int>(), r.list.cachedPages())
                    // Reading again does not hammer the core.
                    r.list[120]
                    r.turn()
                    assertEq(3, r.server.requests.size)
                    // A change clears what failed; a good core then fills the rows.
                    r.server.body = null
                    r.change()
                    r.list[120]
                    r.turn()
                    assertEq(r.server.row(120), r.list[120])
                }
            }
        }

        case("a failed call (an error status, a closed transport) is reported once per page and does not throw into get") {
            LazyRig(total = 400).use { r ->
                r.full()
                r.server.status = ReplyStatus.PANIC
                r.server.body = { _ -> UndraWriter().also { it.writeStr("boom"); it.writeStr("") }.toByteArray() }
                r.list[0]
                r.turn()
                assertEq(2, r.errors.size)
                assertTrue(r.errors.all { it.operation.startsWith("UndraLazyList.page(") })
                assertEq(2, r.log.messages().count { it.startsWith("UndraLazyList.page(") }) // and logged, as every report is
                assertEq(null, r.list[0])
            }
            LazyRig(sync = false, total = 400).use { r ->
                r.full()
                r.transport.callFailure = UndraTransportException(UndraTransportException.Reason.CLOSED, "gone")
                r.list[0]
                r.turn()
                assertEq(2, r.errors.size)
                assertTrue(r.errors.all { it.error is UndraCallError.Unavailable })
            }
        }

        case("a value that is not a lazy value is a typed exception for the mirror to report") {
            LazyRig().use { r ->
                assertThrows<WireException.UnexpectedEof> { r.list.applyFull(UndraReader(ByteArray(7))) }
                assertThrows<WireException.TrailingBytes> {
                    r.list.applyFull(UndraReader(Payloads.LazyValue(Handle(3L), 1u, 1uL).toByteArray() + 0))
                }
                assertThrows<UndraProtocolException>("the null handle") {
                    r.list.applyFull(UndraReader(Payloads.LazyValue(Handle(0L), 1u, 1uL).toByteArray()))
                }
                assertThrows<UndraProtocolException>("a length no Int can index") {
                    r.list.applyFull(UndraReader(Payloads.LazyValue(Handle(3L), UInt.MAX_VALUE, 1uL).toByteArray()))
                }
                assertThrows<WireException.UnexpectedEof> { r.list.applyInvalidated(UndraReader(ByteArray(11))) }
                assertThrows<WireException.TrailingBytes> {
                    r.list.applyInvalidated(UndraReader(Payloads.LazyInvalidated(1u, 1uL).toByteArray() + 0))
                }
                assertThrows<UndraProtocolException> {
                    r.list.applyInvalidated(UndraReader(Payloads.LazyInvalidated(UInt.MAX_VALUE, 2uL).toByteArray()))
                }
                assertEq(0, r.list.size.now())
            }
        }

        case("an invalidation before the first value is ignored") {
            LazyRig().use { r ->
                r.invalidated(10, 4uL)
                assertEq(0, r.list.size.now())
                r.full(len = 20, version = 5uL)
                assertEq(20, r.list.size.now())
            }
        }

        // ---- observation and lifetime ---------------------------------------------------------------

        case("the revision is bumped when pages arrive or are replaced and when the length changes, not when a row is read") {
            LazyRig().use { r ->
                val seen = ArrayList<Long>()
                r.full()
                seen.add(r.list.revision.now())
                r.list[0]
                assertEq(seen.last(), r.list.revision.now())
                r.turn()
                assertTrue(r.list.revision.now() > seen.last(), "pages arrived")
                seen.add(r.list.revision.now())
                r.list[0]
                r.list[10]
                assertEq(seen.last(), r.list.revision.now())
                r.change(len = 1001)
                assertTrue(r.list.revision.now() > seen.last(), "the length changed")
                seen.add(r.list.revision.now())
                r.turn()
                assertTrue(r.list.revision.now() > seen.last(), "pages were replaced")
            }
        }

        case("a change listener is called after every change, outside the lock, and can be removed; one that throws does not stop the others") {
            LazyRig().use { r ->
                val calls = CopyOnWriteArrayList<Int>()
                val a = r.list.addChangeListener { calls.add(r.list.size.now()) } // reads the list: no deadlock
                r.list.addChangeListener { throw IllegalStateException("a bad listener") }
                val b = r.list.addChangeListener { calls.add(-1) }
                r.full(len = 500)
                assertEq(listOf(500, -1), calls.toList())
                b.close()
                r.invalidated(501, 2uL)
                assertEq(listOf(500, -1, 501), calls.toList())
                a.close()
                r.invalidated(502, 3uL)
                assertEq(3, calls.size)
            }
        }

        case("a closed list returns null, requests nothing and ignores what is still in flight") {
            LazyRig(sync = false).use { r ->
                r.full()
                r.hold = true
                r.list[0]
                r.turn()
                r.list.close()
                r.hold = false
                for (call in r.held.toList()) r.release(call)
                r.settle()
                assertEq(null, r.list[0])
                r.list.prefetch(0..10)
                r.full(handle = 9L)
                r.invalidated(10, 9uL)
                r.turn()
                assertEq(2, r.server.requests.size)
                assertEq(0, r.errors.size)
                r.list.close() // twice is fine
            }
        }

        // ---- through a store, the way generated code does it ----------------------------------------

        case("a store hands the Full value and the invalidation of its lazy signal to the list, on the main thread") {
            val server = LazyPageServer(300)
            val t = FakeTransport()
            t.onCallSync = server::answer
            val handle = 11L
            t.onObserve = { h, _, on ->
                if (on) t.events.onChangeSet(changeSet(1uL, full(h, 3u, Payloads.LazyValue(Handle(0x0000_0100_0000_0009L), 300u, 1uL).toByteArray())))
            }
            dev.undra.runtime.support.attach(t).use { core ->
                val store = LazyLibraryStore(core, handle)
                assertEq(300, store.books.size.value) // applied before observe returned
                assertEq(null, store.books[120])
                eventually("the page arrives (requested on the main thread, in the next turn)") { store.books[120] != null }
                assertEq(server.row(120), store.books[120])
                assertEq(CallTarget.LazyListPage(Handle(0x0000_0100_0000_0009L), 100u, 50u), server.requests.first { it.offset == 100u })

                server.total = 310
                server.version = 2uL
                server.base = 9000
                t.events.onChangeSet(
                    changeSet(
                        2uL,
                        Payloads.ChangeEntry(Handle(handle), 3u, ChangeOp.INVALIDATED, Payloads.LazyInvalidated(310u, 2uL).toByteArray()),
                    ),
                )
                core.mirror.flush()
                eventually("the new length and the re-paged window") { store.books.size.value == 310 && store.books[120] == 9120 }
                assertEq(listOf("full on undra-main", "invalidated on undra-main"), store.seen.toList())
                // A keyed patch for a lazy signal is ignored, not an error.
                t.events.onChangeSet(changeSet(3uL, dev.undra.runtime.support.patch(handle, 3u, ByteArray(4))))
                core.mirror.flush()
                assertEq(310, store.books.size.value)
                store.close()
                store.books.close()
            }
        }

        case("a drain that folded [Full(handle), Inv] gives the list its page server and the newer length; a restore's new handle starts over") {
            val log = LogCapture("dev.undra.runtime")
            val server = LazyPageServer(300)
            val t = FakeTransport()
            t.onCallSync = server::answer
            val a = 0x0000_0100_0000_0009L
            val b = 0x0000_0200_0000_0004L
            dev.undra.runtime.support.attach(t).use { core ->
                val store = LazyLibraryStore(core, 11L)
                // One change-set, one drain: the full value and an invalidation (the mirror keeps both, in order).
                t.events.onChangeSet(changeSet(1uL, full(11L, 3u, lazyValue(a, 300, 1uL)), lazyInvalidated(11L, 3u, 305, 2uL)))
                // And, in the next change-sets of the same drain, more invalidations: the last one wins.
                t.events.onChangeSet(changeSet(2uL, lazyInvalidated(11L, 3u, 308, 3uL)))
                t.events.onChangeSet(changeSet(3uL, lazyInvalidated(11L, 3u, 310, 4uL)))
                server.total = 310
                server.version = 4uL
                core.mirror.flush()
                eventually("the drain was applied") { store.books.size.value == 310 }
                assertEq(4uL, store.books.currentVersion())
                assertEq(listOf("full on undra-main", "invalidated on undra-main"), store.seen.toList(), "[Full, Inv, Inv] was folded to [Full, Inv]")
                assertEq(null, store.books[0])
                eventually("the first page arrives, asked of the page server the full value named") { store.books[0] != null }
                assertEq(Handle(a), server.requests.first().handle)

                // A restore: a new page server, then an invalidation, in one drain: [Full(b), Inv].
                server.requests.clear()
                server.total = 25
                server.version = 6uL
                t.events.onChangeSet(changeSet(4uL, full(11L, 3u, lazyValue(b, 20, 5uL)), lazyInvalidated(11L, 3u, 25, 6uL)))
                core.mirror.flush()
                eventually("the restore was applied") { store.books.size.value == 25 }
                assertEq(6uL, store.books.currentVersion())
                assertEq(null, store.books[0], "the cache of the old page server is gone")
                eventually("rows come from the new page server") { store.books[0] != null }
                assertTrue(server.requests.all { it.handle == Handle(b) }, "every page call goes to the new page server: ${server.requests}")
                store.close()
                store.books.close()
            }
            log.close()
        }

        // ---- threads --------------------------------------------------------------------------------

        case("reads from several threads while the core invalidates and the main thread pages: no failure, and the rows converge") {
            val log = LogCapture("dev.undra.runtime")
            val server = LazyPageServer(5000)
            val t = FakeTransport()
            val lock = Any()
            t.onCallSync = { call -> synchronized(lock) { server.answer(call) } }
            val errors = CopyOnWriteArrayList<UndraUnhandledError>()
            val core = UndraCore.attach(t, LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false, onError = { errors.add(it) }), makeShared = false)
            val main = Executors.newSingleThreadExecutor { r -> Thread(r, "test-main").also { it.isDaemon = true } }
            val list = UndraLazyList(core, Codecs.i32, { task -> main.execute(task) }) { Dispatchers.Unconfined }
            list.applyFull(UndraReader(Payloads.LazyValue(Handle(7L), 5000u, 1uL).toByteArray()))
            val failures = CopyOnWriteArrayList<Throwable>()
            val readers = List(4) { n ->
                Thread {
                    try {
                        val rnd = java.util.Random(n.toLong())
                        repeat(20_000) { list[rnd.nextInt(5200) - 100] }
                    } catch (e: Throwable) {
                        failures.add(e)
                    }
                }
            }
            val invalidator = Thread {
                try {
                    for (v in 2..300) {
                        synchronized(lock) {
                            server.version = v.toULong()
                            server.base = v * 10
                        }
                        list.applyInvalidated(UndraReader(Payloads.LazyInvalidated(5000u, v.toULong()).toByteArray()))
                    }
                } catch (e: Throwable) {
                    failures.add(e)
                }
            }
            (readers + invalidator).forEach { it.start() }
            (readers + invalidator).forEach { it.join(60_000) }
            assertEq(emptyList<Throwable>(), failures.toList())
            // Settle: a last read, then the main thread's queue.
            list[2500]
            main.submit {}.get(10, TimeUnit.SECONDS)
            main.submit {}.get(10, TimeUnit.SECONDS)
            eventually("the rows converge on the last version") { list[2500] == server.row(2500) }
            assertEq(emptyList<UndraUnhandledError>(), errors.toList())
            assertTrue(list.cachedPages().size <= 24)
            list.close()
            core.close()
            main.shutdownNow()
            log.close()
        }
    }

    @Test
    fun allCases() = assertPassed()
}
