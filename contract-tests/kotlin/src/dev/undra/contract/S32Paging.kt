package dev.undra.contract

import dev.undra.playground.core.FeedQueryHandle
import dev.undra.playground.core.Item
import dev.undra.playground.core.Library
import dev.undra.playground.core.ListError
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.touchFeed
import dev.undra.runtime.UndraDispatchers
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.KeyedPatch
import dev.undra.runtime.wire.PatchOp
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import kotlin.coroutines.EmptyCoroutineContext

private val LIB = UndraIds.Objects.Library
private val FEED = UndraIds.Objects.FeedQueryHandle

/**
 * Counts the page calls (target 3) the runner's reads made. They are counted where they cross, in the core: `crossings.calls`
 * counts every call that reaches it, so a page call is what is left once the calls the scenario made itself ([own]) are taken
 * away. Nothing else calls the core while a step only reads rows.
 */
private class PageCalls(private val w: World) {
    private var base = w.stats().calls
    private var own = 0L

    /** Starts counting from now. */
    fun reset() {
        base = w.stats().calls
        own = 0L
    }

    /** The scenario itself made [n] calls: they are not page calls. */
    fun own(n: Long = 1L) {
        own += n
    }

    /** The page calls since [reset]. */
    val pages: Long get() = w.stats().calls - base - own
}

/**
 * Runs [block] while the main thread is held: what the core delivers meanwhile reaches the mirror's queue but nothing is
 * applied until [block] returns (the frame is held, ADR-031), so everything [block] does is one drain.
 */
private fun <T> holdingTheFrame(block: () -> T): T {
    val entered = CountDownLatch(1)
    val release = CountDownLatch(1)
    UndraDispatchers.main.dispatch(
        EmptyCoroutineContext,
        Runnable {
            entered.countDown()
            release.await(WAIT_MS * 2, TimeUnit.MILLISECONDS)
        },
    )
    if (!entered.await(WAIT_MS, TimeUnit.MILLISECONDS)) fail("the main thread did not take the task that holds the frame")
    try {
        return block()
    } finally {
        release.countDown()
    }
}

private fun u32(value: UInt): ByteArray = UndraWriter().also { it.writeU32(value) }.toByteArray()

/** The reply of a raw page call: its header (the rows after it are not needed). */
private fun pageReply(w: World, pageServer: Handle, offset: UInt, limit: UInt): Payloads.LazyPageHeader {
    val body = w.core.callSync(CallTarget.LazyListPage(pageServer, offset, limit), 0u, ByteArray(0))
    return Payloads.LazyPageHeader.decode(UndraReader(body))
}

/**
 * S32 (ADR-043): paged queries and lazy lists. The lists go through the generated `Library`, the page calls are counted at the
 * boundary the runner's transport crosses (the core's own `crossings.calls`, less the calls the scenario made), and a second,
 * raw `Library` (or feed handle) records the change-set entries a generated store decodes and throws away.
 */
fun s32Paging(w: World) {
    val calls = PageCalls(w)

    // 1. A lazy list is not sent whole.
    val raw = RawStore(w.core, LIB.TYPE_ID, LIB.NEW)
    raw.observe()
    val initialBooks = raw.entries.single { it.signalId == 0u }
    expectEq("the op of the initial books entry", ChangeOp.FULL, initialBooks.op)
    expectEq("the initial books entry (a LazyValue, not 10,000 rows)", 20, initialBooks.value.size)
    val rawValue = Payloads.LazyValue.decode(initialBooks.value)
    expectEq("the length in the books LazyValue", 10_000u, rawValue.len)
    check(!rawValue.handle.isNull) { "the books LazyValue names no page server" }
    val initialEvens = raw.entries.single { it.signalId == 2u }
    expectEq("the initial evens entry", 20, initialEvens.value.size)
    expectEq("the length in the evens LazyValue", 10u, Payloads.LazyValue.decode(initialEvens.value).len)

    val lib = Library.create()
    awaitEq("books.count after the first drain", 10_000) { lib.books.size.value }
    calls.reset()
    expectEq("books[0] while its page loads", null, lib.books[0])
    awaitUntil("books[0] after its page arrived") { lib.books[0] != null }
    expectEq("books[0].id", 1u, lib.books[0]!!.id)
    expectEq("books[9999] while its page loads", null, lib.books[9_999])
    awaitUntil("books[9999] after its page arrived") { lib.books[9_999] != null }
    expectEq("books[9999].id (the last page)", 10_000u, lib.books[9_999]!!.id)
    expectEq("the page calls of rows 0 and 9999 (two pages each, one of them prefetch)", 4L, calls.pages)
    val outside = calls.pages
    for (index in listOf(-1, 10_000, 10_001, Int.MAX_VALUE, Int.MIN_VALUE)) expectEq("books[$index]", null, lib.books[index])
    holdsFor("the page calls after reads outside 0 until count") { calls.pages == outside }
    lib.close()

    // 2. A page is requested once, with one page of prefetch each side.
    val second = Library.create()
    awaitEq("books.count of a second library", 10_000) { second.books.size.value }
    calls.reset()
    expectEq("books[120] while its page loads", null, second.books[120])
    awaitUntil("books[120] after its page arrived") { second.books[120] != null }
    expectEq("books[120].id", 121u, second.books[120]!!.id)
    awaitEq("the page calls of the first read of row 120 (pages 2, 1 and 3)", 3L) { calls.pages }
    for (index in 100..149 step 7) check(second.books[index] != null) { "books[$index] is on a loaded page but reads as nothing" }
    holdsFor("the page calls of reads inside the loaded pages") { calls.pages == 3L }
    val reply = pageReply(w, rawValue.handle, 100u, 50u)
    calls.own()
    expectEq("the list's version is the version of the page reply", reply.version, second.books.version)
    expectEq("the total in the page reply", 10_000u, reply.total)
    expectEq("the rows in the page reply", 50u, reply.count)

    // 3. A change is one 12-byte entry and the host re-pages only its window.
    var mark = raw.mark()
    raw.callSync(LIB.ADD_ROWS, u32(1u))
    calls.own()
    flushMainThread()
    val invalidation = raw.since(mark).single { it.signalId == 0u }
    expectEq("the op of the books entry of add_rows(1)", ChangeOp.INVALIDATED, invalidation.op)
    expectEq("the books entry of add_rows(1) is 12 bytes", 12, invalidation.value.size)
    val invalidated = Payloads.LazyInvalidated.decode(invalidation.value)
    expectEq("the length after add_rows(1)", 10_001u, invalidated.len)
    check(invalidated.version > reply.version) { "the version did not grow: ${invalidated.version} after ${reply.version}" }

    val before = calls.pages
    second.addRows(1u)
    calls.own()
    awaitEq("books.count after add_rows(1)", 10_001) { second.books.size.value }
    check(second.books[120] != null) { "the rows on screen are not readable while their pages are asked for again" }
    awaitEq("the page calls of re-paging the window (pages 1, 2 and 3), not 200", before + 3L) { calls.pages }
    holdsFor("the page calls after the window was re-paged") { calls.pages == before + 3L }

    second.rename(121u, "x")
    calls.own()
    awaitUntil("books[121] after rename(121, \"x\")") { second.books[121]?.label == "x" }
    expectEq("books[121].version", 1u, second.books[121]!!.version)
    val renameFailure = expectFails<ListError.OutOfRange>("rename(10_001, \"x\")") { second.rename(10_001u, "x") }
    expectEq("the index in the refusal", 10_001u, renameFailure.index)
    calls.own()

    second.removeAt(0u)
    calls.own()
    awaitEq("books.count after remove_at(0)", 10_000) { second.books.size.value }
    awaitUntil("books[0] after remove_at(0)") { second.books[0]?.id == 2u }
    second.close()

    // 4. A drain folds without losing the page server (ADR-031, amendment 2026-10-02).
    mark = raw.mark()
    holdingTheFrame {
        repeat(3) { raw.callSync(LIB.ADD_ROWS, u32(1u)) }
    }
    flushMainThread()
    val folded = raw.since(mark).filter { it.signalId == 0u }
    expectEq("the books entries delivered for three add_rows(1) with the frame held", 1, folded.size)
    expectEq("the op of that entry", ChangeOp.INVALIDATED, folded.single().op)
    val last = Payloads.LazyInvalidated.decode(folded.single().value)
    expectEq("the length of that entry (the last)", 10_004u, last.len)
    val afterThree = pageReply(w, rawValue.handle, 0u, 1u)
    expectEq("the version of that entry is the last version", afterThree.version, last.version)
    expectEq("the total the last page reply carries", 10_004u, afterThree.total)

    // A library whose add_rows runs before its first drain: the drain holds the op 0 that names the page server, then the op 2.
    val early = Library.create()
    early.addRows(1u)
    awaitEq("count of a library that grew right after it was made", 10_001) { early.books.size.value }
    awaitUntil("books[0] of that library") { early.books[0] != null }
    var observingThread: Thread? = null
    var observeFailureOf: (() -> Throwable?)? = null
    // The Kotlin constructor returns only after its first drain, so the same timing is made by observing again: the core re-sends
    // its values, add_rows commits behind them, and the held frame applies both in one drain.
    w.core.observe(early.handle, RawStore.ALL_SIGNALS, false)
    val sent = w.core.mirror.stats().changeSetsReceived
    holdingTheFrame {
        var observeFailure: Throwable? = null
        val observing = thread(name = "s32-observe") {
            try {
                w.core.observe(early.handle, RawStore.ALL_SIGNALS, true)
            } catch (e: Throwable) {
                observeFailure = e
            }
        }
        awaitUntilPlain("the core's values reached the held mirror") { w.core.mirror.stats().changeSetsReceived > sent }
        early.addRows(1u)
        // The observing thread waits for the main thread, which is released when this block ends.
        check(observing.isAlive) { "the observe call returned while the frame was held" }
        observingThread = observing
        observeFailureOf = { observeFailure }
    }
    observingThread!!.join(WAIT_MS)
    observeFailureOf!!()?.let { throw Mismatch("observing again failed: $it") }
    awaitEq("count after the values and the add_rows were applied in one drain", 10_002) { early.books.size.value }
    awaitUntil("books[0] after the drain that held the op 0 and the op 2") { early.books[0] != null }
    expectEq("books[0].id", 1u, early.books[0]!!.id)
    early.close()

    // 5. A view pages through the derived index.
    val view = Library.create()
    awaitEq("evens.count", 10) { view.evens.size.value }
    awaitUntil("evens[0]") { view.evens[0] != null && view.evens[9] != null }
    expectEq("evens[0].id", 2u, view.evens[0]!!.id)
    expectEq("evens[9].id", 20u, view.evens[9]!!.id)
    view.addRows(2u)
    awaitEq("evens.count after add_rows(2)", 11) { view.evens.size.value }
    awaitUntil("evens[10]") { view.evens[10]?.id == 10_002u }
    view.dropSource(20u)
    awaitEq("evens.count after drop_source(20)", 1) { view.evens.size.value }
    awaitUntil("evens[0] after drop_source(20)") { view.evens[0]?.id == 10_002u }
    view.close()
    raw.close()

    feed(w)
}

/** 6. An infinite query grows a page at a time. */
private fun feed(w: World) {
    val tap = RawStore(w.core, FEED.TYPE_ID, FEED.NEW, UndraWriter().also { it.writeBool(false) }.toByteArray())
    tap.observe()
    val feed = FeedQueryHandle.create(evenOnly = false)
    try {
        awaitEq("data.count after the first fetch", 50) { feed.data.value.size }
        awaitEq("hasNextPage after the first fetch", true) { feed.hasNextPage.value }
        awaitEq("fetchingNextPage after the first fetch", false) { feed.fetchingNextPage.value }

        val mark = tap.mark()
        val fetching = onMain {
            feed.fetchNextPage()
            feed.fetchingNextPage.value
        }
        expectEq("fetchingNextPage right after fetchNextPage()", true, fetching)
        awaitEq("fetchingNextPage when the page arrived", false) { feed.fetchingNextPage.value }
        awaitEq("data.count after fetchNextPage()", 100) { feed.data.value.size }
        expectEq("the ids of the rows", (1u..100u).toList(), feed.data.value.map { it.id })
        val grown = tap.since(mark).filter { it.signalId == 0u }
        expectEq("the data entries of fetchNextPage()", 1, grown.size)
        expectEq("the op of the data entry (a keyed patch, not a full value)", ChangeOp.PATCH, grown.single().op)
        check(grown.single().value.size < 5 * 1024) { "the data patch is ${grown.single().value.size} bytes" }
        val inserts = KeyedPatch.decodePatch(grown.single().value, Item)
        expectEq("the operations of the data patch", 50, inserts.size)
        check(inserts.all { it is PatchOp.Insert<*> }) { "the data patch holds more than inserts: ${inserts.map { it::class.simpleName }.distinct()}" }

        // A second handle with the same parameter shares the entry: it has the rows at once and fetches nothing.
        val shared = FeedQueryHandle.create(evenOnly = false)
        try {
            expectEq("the rows of a second handle with the same parameter, at once", 100, shared.data.value.size)
            expectEq("a second handle fetches nothing of its own", false, shared.fetching.value)
            holdsFor("the rows of the second handle") { shared.data.value.size == 100 && !shared.fetching.value }
        } finally {
            shared.close()
        }

        // A refetch finds only what changed: the even rows now show revision 7.
        touchFeed(7u)
        val refetchMark = tap.mark()
        feed.refetch()
        awaitUntil("the even rows to show revision 7") {
            feed.data.value.size == 100 && feed.data.value.all { it.version == if (it.id % 2u == 0u) 7u else 0u }
        }
        val changed = tap.since(refetchMark).filter { it.signalId == 0u }
        check(changed.isNotEmpty()) { "the refetch delivered nothing for data" }
        check(changed.all { it.op == ChangeOp.PATCH }) { "the refetch delivered a full value: ${changed.map { it.op }}" }
        val updates = changed.flatMap { KeyedPatch.decodePatch(it.value, Item) }
        check(updates.all { it is PatchOp.Update<*> }) { "the refetch's patch holds more than updates: ${updates.map { it::class.simpleName }.distinct()}" }
        check(updates.size <= 50) { "the refetch's patch holds ${updates.size} updates" }
        expectEq("the rows after the refetch", 100, feed.data.value.size)
    } finally {
        feed.close()
        tap.close()
    }

    val evens = FeedQueryHandle.create(evenOnly = true)
    try {
        awaitEq("data.count of the even-only feed", 50) { evens.data.value.size }
        expectEq("the ids of the even-only feed", (1u..50u).map { it * 2u }, evens.data.value.map { it.id })
    } finally {
        evens.close()
    }
}

/** [awaitUntil] for a wait on the main thread's own state, with no drain of the mirror (the frame is held). */
private fun awaitUntilPlain(what: String, timeoutMs: Long = WAIT_MS, condition: () -> Boolean) {
    val deadline = System.nanoTime() + timeoutMs * 1_000_000L
    while (!condition()) {
        if (System.nanoTime() > deadline) fail("timed out after $timeoutMs ms waiting for $what")
        Thread.sleep(2L)
    }
}
