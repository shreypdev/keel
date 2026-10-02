package dev.undra.contract

import dev.undra.playground.core.Counter
import dev.undra.playground.core.FeedQueryHandle
import dev.undra.playground.core.Library
import dev.undra.playground.core.Probe
import dev.undra.playground.core.QueryStatus
import dev.undra.playground.core.RemoteTodo
import dev.undra.playground.core.RemoteTodosQueryHandle
import dev.undra.playground.core.RosterQueryHandle
import dev.undra.playground.core.TickerQueryHandle
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraStore
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.UndraReader
import java.lang.reflect.InvocationTargetException
import java.util.concurrent.CopyOnWriteArrayList

/**
 * The entries the mirror applies to a generated wrapper, recorded (S35 steps 3 and 8: "the entries the mirror applied to the remote and
 * feed wrappers during the restore are none"). The mirror keeps one registration per handle and a generated store's `apply` is
 * protected, so the tap takes the wrapper's registration over with one that records the entry and then hands it to the wrapper's own
 * `apply` (the generated override of [UndraStore.apply], found by reflection: its name is mangled, as it takes a `UInt`), which is
 * what the mirror did before. The wrapper sees exactly what it saw without the tap; closing it unregisters the tap with it.
 */
private class Tap(core: UndraCore, wrapper: UndraStore) {
    /** The entries applied since the tap was made (or last cleared), oldest first. */
    val entries = CopyOnWriteArrayList<RawStore.Entry>()

    init {
        val apply = wrapper.javaClass.declaredMethods.single {
            it.name.startsWith("apply") && it.parameterTypes.size == 3 && it.parameterTypes[2] == UndraReader::class.java
        }
        apply.isAccessible = true
        core.mirror.register(wrapper.handle) { signalId, op, reader ->
            val value = reader.readRemaining()
            entries.add(RawStore.Entry(signalId, op, value))
            try {
                apply.invoke(wrapper, signalId.toInt(), op, UndraReader(value))
            } catch (e: InvocationTargetException) {
                throw e.targetException
            }
        }
    }

    /** The newest entry of signal [signalId] with op [op], or `null`. */
    fun last(signalId: UInt, op: ChangeOp = ChangeOp.FULL): RawStore.Entry? = entries.lastOrNull { it.signalId == signalId && it.op == op }
}

/** "For [millis] ms nothing happens" without a condition of its own: the mirror is drained at every look, then the caller asserts. */
internal fun quietFor(millis: Long = 200L) {
    holdsFor("a quiet window", millis) { true }
}

/** A raw page call to the page server [server] (the header of the reply; the rows after it are not needed): what a lazy list issues. */
internal fun pageHeader(core: UndraCore, server: Handle, offset: UInt, limit: UInt): Payloads.LazyPageHeader {
    val body = core.callSync(CallTarget.LazyListPage(server, offset, limit), 0u, ByteArray(0))
    return Payloads.LazyPageHeader.decode(UndraReader(body))
}

/**
 * S35 (ADR-059): query handles across a restore. A snapshot keeps what a query handle is made of, and a restore re-issues the handle
 * under the same value, so the app's wrapper keeps working. Steps 1 to 9 restore into this core, through the generated bindings;
 * step 2 hands the snapshot and the handles over to the build-B process, which restores it into a fresh core ([migrationBuildB],
 * step 10). The server serves `GET /lists/s35/todos`.
 *
 * What the mirror applies to the remote and feed wrappers is recorded by a [Tap] on each (their registrations carry the entries
 * the restore would have sent); the remote's `data`, `status` and the feed's `data` are also recorded as the UI sees them
 * ([Recorder]), where a blink (data absent, status fetching) would show.
 */
fun s35QueryHandles(w: World) {
    val core = w.core
    w.configureRemoteOnce()
    Handover.discard("s35")
    val list = "s35"
    val url = "${World.BASE_URL}/lists/$list/todos"
    val milk = RemoteTodo(1u, "Buy milk", false)
    val dog = RemoteTodo(2u, "Walk the dog", false)
    val gets = { w.server.count(HttpMethod.GET, url) }
    w.server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"Buy milk","done":false}]""")

    // Closed however the steps end, so that a failure here does not leave handles that later scenarios (S17) would count.
    val opened = ArrayList<AutoCloseable>()
    fun <T : AutoCloseable> T.closedLast(): T = also { opened += it }
    try {
        // 1. Opening. live_handles first; the remote handle shows the server's one item, the ticker has ticked, the feed has two pages,
        // the library has read books[0], the roster handle is observed, a counter has 5 and a probe exists.
        val liveBefore = w.stats().liveHandles
        val remote = RemoteTodosQueryHandle.create(list).closedLast()
        awaitEq("the data of the remote handle", listOf(milk)) { remote.data.value }
        awaitEq("the status of the remote handle", QueryStatus.SUCCESS) { remote.status.value }
        awaitEq("fetching of the remote handle", false) { remote.fetching.value }
        val ticker = TickerQueryHandle.create().closedLast()
        awaitUntil("the ticker to have ticked") { ticker.data.value != null && ticker.status.value == QueryStatus.SUCCESS }
        val feed = FeedQueryHandle.create(evenOnly = false).closedLast()
        awaitUntil("the first page of the feed") { feed.data.value.size >= 50 }
        // S32 left the entry with its pages (the cache outlives its observers), so the handle may show them at once: ask for pages only
        // until there are two.
        while (feed.data.value.size < 100) {
            val rows = feed.data.value.size
            awaitEq("fetchingNextPage before asking for a page", false) { feed.fetchingNextPage.value }
            feed.fetchNextPage()
            awaitUntil("the next page of the feed") { feed.data.value.size > rows }
        }
        expectEq("the rows of the feed after step 1", 100, feed.data.value.size)
        val library = Library.create().closedLast()
        awaitEq("books.count", 10_000) { library.books.size.value }
        awaitUntil("books[0]") { library.books[0] != null }
        val roster = RosterQueryHandle.create(7u).closedLast()
        awaitEq("the data of the roster handle", listOf("team 7")) { roster.data.value }
        val counter = Counter.create().closedLast()
        counter.add(5)
        awaitEq("Counter.count after add(5)", 5) { counter.count.value }
        val probe = Probe.create().closedLast()
        expectEq("GET count after step 1", 1, gets())
        val remoteHandle = remote.handle
        val feedHandle = feed.handle

        // 2. The snapshot (handed over to build B with the handles of step 1), a change to the store and to the server, then the restore.
        val snapshot = core.snapshot()
        check(snapshot.isNotEmpty()) { "the snapshot is empty" }
        Handover.write(Handover.QueryHandles(snapshot, remote.handle, ticker.handle, feed.handle, library.handle, roster.handle))
        counter.add(10)
        awaitEq("Counter.count after add(10)", 15) { counter.count.value }
        w.server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"Buy milk","done":false},{"id":2,"title":"Walk the dog","done":false}]""")
        val remoteTap = Tap(core, remote)
        val feedTap = Tap(core, feed)
        val libraryTap = Tap(core, library)
        val remoteData = Recorder(remote.data).closedLast()
        val remoteStatus = Recorder(remote.status).closedLast()
        val feedData = Recorder(feed.data).closedLast()
        val liveBeforeRestore = w.stats().liveHandles
        val getsBeforeRestore = gets()
        w.takeUnhandled()
        core.restore(snapshot)

        // 3. Same wrappers, nothing blinked.
        awaitEq("Counter.count after the restore", 5) { counter.count.value }
        quietFor()
        expectEq("the handle of the remote wrapper after the restore", remoteHandle, remote.handle)
        expectEq("the handle of the feed wrapper after the restore", feedHandle, feed.handle)
        expectEq("the entries the mirror applied to the remote wrapper during the restore", emptyList<String>(), remoteTap.entries.map { it.toString() })
        expectEq("the entries the mirror applied to the feed wrapper during the restore", emptyList<String>(), feedTap.entries.map { it.toString() })
        expectEq("the values the remote's data took", listOf<List<RemoteTodo>?>(listOf(milk)), remoteData.values.toList())
        expectEq("the values the remote's status took", listOf(QueryStatus.SUCCESS), remoteStatus.values.toList())
        expectEq("the sizes the feed's data took", listOf(100), feedData.values.map { it.size })
        expectEq("the data of the remote wrapper after the restore", listOf(milk), remote.data.value)
        expectEq("the rows of the feed after the restore", 100, feed.data.value.size)
        expectEq("GET count across the restore", getsBeforeRestore, gets())
        val liveAfterRestore = w.stats().liveHandles
        expectEq("live_handles after the restore (the probe is the one object it makes stale)", liveBeforeRestore - 1, liveAfterRestore)
        expectEq("the reports of the restore", emptyList<String>(), w.takeUnhandled().map { it.operation })

        // 4. refetch is accepted on the same remote wrapper: one more GET, and the two items.
        remote.refetch()
        awaitUntil("the GET of refetch()") { gets() == getsBeforeRestore + 1 }
        awaitEq("the data after refetch()", listOf(milk, dog)) { remote.data.value }
        awaitEq("fetching after refetch()", false) { remote.fetching.value }
        expectEq("the reports of refetch() (a refused command would be reported)", emptyList<String>(), w.takeUnhandled().map { it.operation })
        expectEq("the GET count after refetch()", getsBeforeRestore + 1, gets())

        // 5. Polling continues, with no call from the runner.
        val shown = ticker.data.value ?: 0u
        awaitUntil("the ticker to advance", 2_500L) { (ticker.data.value ?: 0u) > shown }

        // 6. The next page loads on the same feed wrapper, and the library's rows are readable again (the restore rebuilt the store
        // and its page servers: its books entry names a new one, and a page call through it succeeds).
        feed.fetchNextPage()
        awaitEq("the rows of the feed after fetchNextPage()", 150) { feed.data.value.size }
        awaitUntil("books[0] after the restore") { library.books[0] != null }
        expectEq("books[0].id", 1u, library.books[0]!!.id)
        val books = libraryTap.last(0u) ?: fail("the restore delivered no books entry to the library: ${libraryTap.entries}")
        val page = pageHeader(core, Payloads.LazyValue.decode(books.value).handle, 0u, 3u)
        expectEq("the total of a page call through the page server the restore named", 10_000u, page.total)
        expectEq("the rows of that page", 3u, page.count)

        // 7. What is not re-creatable stays stale: the probe is refused, as in S15 step 9.
        expectRefused("probe.counters() after the restore", expectFails<UndraCallError.Refused>("probe.counters() after the restore") { probe.counters() })

        // 8. Idempotent: a second restore changes nothing more. The counter is moved first, so that the restore has something to
        // put back (it is the one thing a restore does change).
        counter.add(1)
        awaitEq("Counter.count before the second restore", 6) { counter.count.value }
        val getsBeforeSecond = gets()
        remoteTap.entries.clear()
        feedTap.entries.clear()
        core.restore(snapshot)
        awaitEq("Counter.count after the second restore", 5) { counter.count.value }
        quietFor()
        expectEq("the entries the mirror applied to the remote wrapper during the second restore", emptyList<String>(), remoteTap.entries.map { it.toString() })
        expectEq("the entries the mirror applied to the feed wrapper during the second restore", emptyList<String>(), feedTap.entries.map { it.toString() })
        expectEq("the values the remote's data took", listOf<List<RemoteTodo>?>(listOf(milk), listOf(milk, dog)), remoteData.values.toList())
        check(remoteStatus.values.all { it == QueryStatus.SUCCESS || it == QueryStatus.FETCHING }) { "the remote's status took the values ${remoteStatus.values}" }
        expectEq("the remote's status now", QueryStatus.SUCCESS, remoteStatus.values.last())
        expectEq("the data of the remote wrapper after the second restore", listOf(milk, dog), remote.data.value)
        expectEq("the rows of the feed after the second restore", 150, feed.data.value.size)
        expectEq("GET count across the second restore", getsBeforeSecond, gets())
        expectEq("live_handles after the second restore", liveAfterRestore, w.stats().liveHandles)

        // 9. Closing the wrappers gives every handle back.
        listOf<AutoCloseable>(remote, ticker, feed, library, roster, counter, probe).forEach { it.close() }
        awaitEq("live_handles after closing the wrappers", liveBefore) { w.stats().liveHandles }
    } finally {
        opened.forEach { runCatching { it.close() } }
    }
}
