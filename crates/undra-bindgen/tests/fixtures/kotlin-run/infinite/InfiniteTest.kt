// Runs the generated Kotlin of the `infinite` golden case (ADR-043): the handle of an infinite query is an
// `InfiniteQuery`, pages and polls through commands, and its rows arrive as keyed patches.

package golden.infinite

import dev.undra.runtime.InfiniteQuery
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.KeyedPatch
import dev.undra.runtime.wire.PatchOp
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.encodeToByteArray
import golden.support.FakeCore
import golden.support.expect
import golden.support.expectEq
import kotlin.time.Duration.Companion.milliseconds

fun main() {
    val core = FakeCore()
    core.nextHandle = 3L
    val feed = FeedQueryHandle.create(Filter.ALL, core)
    val ids = UndraIds.Objects.FeedQueryHandle
    expectEq(core.constructed.last(), Triple(ids.TYPE_ID, ids.NEW, "0000"), "constructor")

    // It is what the Compose helper takes, with the two signals and the command of the interface.
    val query: InfiniteQuery = feed
    expectEq(query.hasNextPage.value, false, "no next page yet")
    expectEq(query.fetchingNextPage.value, false, "nothing loading")
    expectEq(feed.data.value, emptyList(), "no rows yet")

    // The commands: each is a call of its fixed method id.
    query.fetchNextPage()
    expectEq(core.calls.last().methodId, ids.FETCH_NEXT_PAGE, "fetchNextPage id")
    expectEq(core.calls.last().args, "", "fetchNextPage takes nothing")
    feed.setPollInterval(1500.milliseconds)
    expectEq(core.calls.last().methodId, ids.SET_POLL_INTERVAL, "setPollInterval id")
    expectEq(core.calls.last().args, "01" + "002f685900000000", "an interval is an option of a duration")
    feed.setPollInterval(null)
    expectEq(core.calls.last().args, "00", "no interval")
    feed.refetch()
    expectEq(core.calls.last().methodId, ids.REFETCH, "refetch id")
    expect(core.calls.last().target is CallTarget.ObjectMethod, "a call on the handle")
    expect(core.reports.isEmpty(), "no command failed: ${core.reports}")

    // The rows: a full value, then a next page that arrives as a patch appending to it.
    fun post(id: ULong) = Post(PostId(id), "a", "b$id")
    core.deliver(3L, 0, ChangeOp.FULL, Codecs.vec(Post).encodeToByteArray(listOf(post(1uL), post(2uL))))
    expectEq(feed.data.value, listOf(post(1uL), post(2uL)), "first page")
    core.deliver(
        3L, 0, ChangeOp.PATCH,
        KeyedPatch.encodePatch(listOf(PatchOp.Insert(2u, post(3uL)), PatchOp.Insert(3u, post(4uL))), Post),
    )
    expectEq(feed.data.value.map { it.id.value }, listOf(1uL, 2uL, 3uL, 4uL), "next page appended")
    core.deliver(3L, 5, ChangeOp.FULL, Codecs.bool.encodeToByteArray(true))
    core.deliver(3L, 6, ChangeOp.FULL, Codecs.bool.encodeToByteArray(true))
    expectEq(feed.hasNextPage.value, true, "has next page")
    expectEq(feed.fetchingNextPage.value, true, "fetching next page")

    // An ordinary handle is not an `InfiniteQuery` and has no paging signals.
    val profile = ProfileQueryHandle.create(core)
    expect((profile as Any) !is InfiniteQuery, "a plain handle does not page")
    profile.setPollInterval(2000.milliseconds)
    expectEq(core.calls.last().methodId, UndraIds.Objects.ProfileQueryHandle.SET_POLL_INTERVAL, "poll id")
    println("ok")
}
