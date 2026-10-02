package dev.undra.contract

import dev.undra.playground.core.Workshop
import dev.undra.playground.core.WorkshopError
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraUnhandledError
import dev.undra.twocores.a.UndraPlaygroundA
import dev.undra.twocores.b.UndraPlaygroundB
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import dev.undra.twocores.a.Shelf as ShelfA
import dev.undra.twocores.a.Workshop as WorkshopA

/**
 * S27 (ADR-040): objects cross as parameters and returns. A `Workshop` (a store) hands out `Shelf`s (child stores)
 * through the generated classes: one handle is one wrapper, a parameter is borrowed, closing gives back exactly the
 * reference a wrapper owns, and a cancelled call, a restore and a collected wrapper owe nothing.
 */
fun s27ObjectsCross(w: World) {
    val core = w.core
    w.takeUnhandled()
    val start = w.stats()

    // 1. A parent returns a child: a store, observed when its wrapper was made.
    val workshop = onMain { Workshop() }
    val afterWorkshop = w.stats()
    expectEq("live_handles for the workshop", 1L, afterWorkshop.liveHandles - start.liveHandles)
    expectEq("host_refs for the workshop", 1L, afterWorkshop.hostRefs - start.hostRefs)
    val a = onMain { workshop.shelf("a") }
    expectEq("a.label", "a", a.label.value)
    expectEq("a.items", 0u, a.items.value)
    expectEq("live_handles for the shelf", 1L, w.stats().liveHandles - afterWorkshop.liveHandles)
    expectEq("host_refs for the shelf", 1L, w.stats().hostRefs - afterWorkshop.hostRefs)

    // 2. One object, one handle, one wrapper; a store returned twice is mirrored once.
    val refs = w.stats().hostRefs
    val mirrored = core.stats().hostMirrorHandles
    val again = onMain { workshop.shelf("a") }
    check(again === a) { "shelf(\"a\") twice gave two wrappers" }
    expectEq("the handle of shelf(\"a\") twice", a.handle, again.handle)
    expectEq("host_refs after shelf(\"a\") again (the duplicate given back)", refs, w.stats().hostRefs)
    expectEq("stores the mirror routes to after shelf(\"a\") again", mirrored, core.stats().hostMirrorHandles)
    check(workshop.find("a") === a) { "find(\"a\") is not the same wrapper" }
    expectEq("find(\"zz\")", null, workshop.find("zz"))
    expectEq("shelves() lists a once", 1, workshop.shelves().count { it === a })
    expectEq("host_refs after find and shelves", refs, w.stats().hostRefs)
    val applied = core.mirror.stats().entriesApplied
    val changeSets = core.mirror.stats().changeSetsReceived
    onMain { a.stock(2u) }
    expectEq("a.items after stock(2)", 2u, a.items.value)
    expectEq("change-sets for stock(2)", 1L, core.mirror.stats().changeSetsReceived - changeSets)
    expectEq("entries applied for stock(2): one, not two", 1L, core.mirror.stats().entriesApplied - applied)

    // 3. A child is passed back as a parameter (borrowed: host_refs does not move).
    val b = onMain { workshop.shelf("b") }
    onMain { b.stock(3u) }
    val borrowed = w.stats()
    onMain { workshop.merge(a, b) }
    expectEq("a.items after merge", 0u, a.items.value)
    expectEq("b.items after merge", 5u, b.items.value)
    // One commit, one change-set per store (`transactions` counts change-sets), both applied when merge returned.
    expectEq("change-sets for merge (one per shelf)", 2L, w.stats().transactions - borrowed.transactions)
    expectEq("total([a, b])", 5u, workshop.total(listOf(a, b)))
    expectEq("describe(a)", "a", workshop.describe(a))
    expectEq("describe(null)", "none", workshop.describe(null))
    expectEq("host_refs after the calls that borrowed shelves", borrowed.hostRefs, w.stats().hostRefs)
    expectEq("reports so far", emptyList<String>(), w.takeUnhandled().map { it.operation })

    // 4. Closing releases exactly one reference; the closed handle is stale; a new wrapper comes next.
    val beforeC = w.stats().hostRefs
    val c = onMain { workshop.shelf("c") }
    check(onMain { workshop.shelf("c") } === c) { "shelf(\"c\") twice gave two wrappers" }
    expectEq("host_refs with c", beforeC + 1, w.stats().hostRefs)
    c.close()
    expectEq("host_refs after closing c", beforeC, w.stats().hostRefs)
    c.stock(1u)
    val stale = w.takeUnhandled()
    expectEq("the report of stock(1) on the closed shelf", listOf("Shelf.stock"), stale.map { it.operation })
    check(stale.single().error is UndraCallError.Refused) { "stock(1) on a closed shelf was reported as ${stale.single().error}" }
    val c2 = onMain { workshop.shelf("c") }
    check(c2 !== c) { "shelf(\"c\") after close is the closed wrapper" }
    check(c2.handle != c.handle) { "shelf(\"c\") after close has the closed handle" }
    expectEq("the new c's label", "c", c2.label.value)

    // 5. A call cancelled before it finishes owes nothing; a typed error owes nothing either.
    val beforeOpen = w.stats()
    val cancelled = runBlocking {
        val slow = async(Dispatchers.Default, start = CoroutineStart.UNDISPATCHED) { workshop.open("slow", 60_000u) }
        awaitUntil("open(\"slow\") to be running in the core") { w.stats().activeCalls > beforeOpen.activeCalls }
        slow.cancel()
        try {
            slow.await()
            null
        } catch (e: CancellationException) {
            e
        }
    }
    check(cancelled != null) { "open(\"slow\") returned although it was cancelled" }
    awaitUntil("the cancelled open to end in the core") { w.stats().activeCalls == beforeOpen.activeCalls }
    expectEq("host_refs after the cancelled open", beforeOpen.hostRefs, w.stats().hostRefs)
    expectEq("live_handles after the cancelled open", beforeOpen.liveHandles, w.stats().liveHandles)
    val fast = runBlocking { workshop.open("fast", 10u) }
    expectEq("open(\"fast\").label", "fast", fast.label.value)
    expectFailsAsync<WorkshopError.NoName>("open(\"\")") { workshop.open("", 0u) }
    fast.close()
    expectEq("host_refs after open(fast) was closed and open(\"\") failed", beforeOpen.hostRefs, w.stats().hostRefs)

    // 6. A restore keeps the workshop (a store) and makes the derived shelves stale.
    val snapshot = core.snapshot()
    onMain { core.restore(snapshot) }
    expectEq("the restored workshop answers", 0u, workshop.watching())
    a.stock(1u)
    val afterRestore = w.takeUnhandled()
    check(afterRestore.map { it.operation } == listOf("Shelf.stock") && afterRestore.single().error is UndraCallError.Refused) {
        "stock(1) on a shelf from before the restore: $afterRestore"
    }
    val freshA = onMain { workshop.shelf("a") }
    check(freshA !== a && freshA.handle != a.handle) { "shelf(\"a\") after the restore is the stale wrapper" }
    expectEq("the fresh a's items", 0u, freshA.items.value)
    for (shelf in listOf(a, b, c2, freshA)) shelf.close()

    // 7. Dropping the last reference to a wrapper releases it (the cleaner, after a GC).
    val beforeDrop = w.stats().hostRefs
    makeAndDrop(workshop)
    expectEq("host_refs with the dropped shelf alive", beforeDrop + 1, w.stats().hostRefs)
    awaitUntil("the dropped shelf's reference to be given back") {
        System.gc()
        w.stats().hostRefs == beforeDrop
    }

    // 8. A foreign object is refused before anything is sent (S26's two cores, package A's classes on both).
    foreignObjects()

    // 8a. A stream takes a child (objects-followups O1): it borrows it while it runs, a stale one refuses the stream.
    val streamShelf = onMain { workshop.shelf("stream") }
    onMain { streamShelf.stock(3u) }
    val beforeStream = w.stats().hostRefs
    expectEq("tally(shelf, 3)", listOf(3u, 3u, 3u), runBlocking { workshop.tally(streamShelf, 3u).toList() })
    expectEq("host_refs after a stream borrowed a shelf", beforeStream, w.stats().hostRefs)
    val closedShelf = onMain { workshop.shelf("closed-for-stream") }
    closedShelf.close()
    expectFailsAsync<UndraCallError.Refused>("tally on a closed shelf") { workshop.tally(closedShelf, 1u).toList() }
    expectEq("host_refs after a refused stream", beforeStream, w.stats().hostRefs)
    streamShelf.close()

    // 9. Statistics: UndraStats reports host_refs; closing twice gives back one reference, a raw double release none more.
    expectEq("UndraStats.hostRefs", w.stats().hostRefs, core.stats().hostRefs)
    val before9 = w.stats().hostRefs
    val s9 = onMain { workshop.shelf("s9") }
    expectEq("host_refs with s9", before9 + 1, w.stats().hostRefs)
    s9.close()
    s9.close()
    core.release(s9.handle)
    expectEq("host_refs after closing twice and a raw release of the stale handle", before9, w.stats().hostRefs)
    val beforeClose = w.stats()
    workshop.close()
    expectEq("host_refs after closing the workshop", beforeClose.hostRefs - 1, w.stats().hostRefs)
    expectEq("live_handles after closing the workshop", beforeClose.liveHandles - 1, w.stats().liveHandles)
    expectEq("reports at the end", emptyList<String>(), w.takeUnhandled().map { it.operation })
}

/** Makes a shelf and lets go of it: nothing but the identity map's weak entry remembers it. */
private fun makeAndDrop(workshop: Workshop) {
    onMain { workshop.shelf("dropped").label.value }
}

/** Step 8: package A's classes on core A and core B; a shelf of B passed to A's workshop is refused, sending nothing. */
private fun foreignObjects() {
    val reports = CopyOnWriteArrayList<UndraUnhandledError>()
    val options = { LoadOptions(onError = { reports.add(it) }) }
    val coreA = UndraPlaygroundA.load(options())
    try {
        val coreB = UndraPlaygroundB.load(options())
        try {
            val wA = onMain { WorkshopA.create(coreA) }
            val wB = onMain { WorkshopA.create(coreB) }
            val shelfOfA: ShelfA = onMain { wA.shelf("x") }
            val shelfOfB: ShelfA = onMain { wB.shelf("x") }
            expectEq("the handle numbers of the two cores' shelves (the same history)", shelfOfA.handle, shelfOfB.handle)
            val beforeA = coreA.readStats()
            val beforeB = coreB.readStats()
            // A command reports the refusal (ADR-032, amendment A); a method that returns throws it.
            onMain { wA.merge(shelfOfB, shelfOfA) }
            val refused = reports.toList()
            expectEq("reports of merge with B's shelf", listOf("Workshop.merge"), refused.map { it.operation })
            val error = refused.single().error
            check(error is UndraCallError.Refused && error.reason.contains("Shelf") && error.reason.contains("another core")) {
                "merge with B's shelf was reported as $error"
            }
            val thrown = expectFails<UndraCallError.Refused>("total([B's shelf]) on A") { wA.total(listOf(shelfOfB)) }
            check(thrown.reason.contains("another core")) { "the refusal says ${thrown.reason}" }
            expectEq("calls A counted for the refused calls", beforeA.calls, coreA.readStats().calls)
            expectEq("calls B counted for the refused calls", beforeB.calls, coreB.readStats().calls)
            expectEq("host_refs of A after the refused calls", beforeA.hostRefs, coreA.readStats().hostRefs)
            expectEq("host_refs of B after the refused calls", beforeB.hostRefs, coreB.readStats().hostRefs)
            expectEq("A's shelf after the refused merge", 0u, shelfOfA.items.value)
            for (o in listOf(shelfOfA, shelfOfB, wA, wB)) o.close()
        } finally {
            coreB.close()
        }
    } finally {
        coreA.close()
    }
}
