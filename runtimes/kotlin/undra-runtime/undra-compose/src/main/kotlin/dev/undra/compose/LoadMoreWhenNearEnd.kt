package dev.undra.compose

import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.remember
import dev.undra.runtime.InfiniteQuery

/**
 * Fetches the next page of [query] when this list is scrolled to within [threshold] items of its end, and fetches again
 * after a page arrived if the list is still near its end (a list that is too short to scroll loads until it fills).
 * Nothing is fetched while there is no next page ([InfiniteQuery.hasNextPage]) or one is on its way
 * ([InfiniteQuery.fetchingNextPage]). A fetch that fails leaves the list as it was, so it is not retried in a loop: scroll
 * away from the end and back (or let the query's own retry policy run) to try again.
 *
 * ```kotlin
 * val state = rememberLazyListState()
 * val feed = remember { FeedQuery(filter) }                      // implements InfiniteQuery
 * val posts by feed.data.collectAsState()
 * LazyColumn(state = state) { items(posts, key = { it.id }) { PostRow(it) } }
 * state.LoadMoreWhenNearEnd(feed)
 * ```
 *
 * @param query the infinite query whose next page to fetch.
 * @param threshold how many items before the end the last visible one must be at, 5 by default.
 * @throws IllegalArgumentException if [threshold] is negative.
 */
@Composable
public fun LazyListState.LoadMoreWhenNearEnd(query: InfiniteQuery, threshold: Int = 5) {
    require(threshold >= 0) { "threshold must not be negative, was $threshold" }
    val state = this
    val hasNextPage by query.hasNextPage.collectAsState()
    // Read when the effect runs (the delegate reads the state's current value), not a key: see the effect below.
    val fetchingNextPage by query.fetchingNextPage.collectAsState()
    val itemCount by remember(state) { derivedStateOf { state.layoutInfo.totalItemsCount } }
    val nearEnd by remember(state, threshold) {
        derivedStateOf {
            val info = state.layoutInfo
            isNearEnd(info.visibleItemsInfo.lastOrNull()?.index, info.totalItemsCount, threshold)
        }
    }
    // Restarted when the list gets near its end or leaves it, when a next page appears or goes away, and when rows were
    // added (a page arrived), but not when a fetch ends without adding any: that is how a failure is not retried at once.
    LaunchedEffect(nearEnd, hasNextPage, itemCount, query) {
        if (shouldFetchNextPage(nearEnd, hasNextPage, fetchingNextPage)) query.fetchNextPage()
    }
}

/**
 * Whether the last visible item, [lastVisibleIndex] (`null` when nothing is laid out), is within [threshold] items of the
 * end of a list of [totalItemsCount]. An empty list is not near its end: the first page is the query's own business.
 */
internal fun isNearEnd(lastVisibleIndex: Int?, totalItemsCount: Int, threshold: Int): Boolean =
    lastVisibleIndex != null && totalItemsCount > 0 && lastVisibleIndex >= totalItemsCount - 1 - threshold

/** `true` when a next page should be fetched now: the list is near its end, the core has another page and none is on its way. */
internal fun shouldFetchNextPage(nearEnd: Boolean, hasNextPage: Boolean, fetchingNextPage: Boolean): Boolean =
    nearEnd && hasNextPage && !fetchingNextPage
