package dev.undra.compose

import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.State
import androidx.compose.runtime.collectAsState
import dev.undra.runtime.InfiniteQuery

/**
 * Fetches the next page of [query] when this list is scrolled to within [threshold] items of its end, and keeps doing
 * so while the list is still near its end after a page arrived (a list that is too short to scroll loads until it
 * fills). Nothing is fetched while there is no next page ([InfiniteQuery.hasNextPage]) or one is on its way
 * ([InfiniteQuery.fetchingNextPage]); a failed fetch is reported by the query's command and not retried by this helper
 * until the list is scrolled or the query's state changes.
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
    val fetchingNextPage by query.fetchingNextPage.collectAsState()
    val nearEnd: State<Boolean> = remember(state, threshold) {
        derivedStateOf {
            val info = state.layoutInfo
            isNearEnd(info.visibleItemsInfo.lastOrNull()?.index, info.totalItemsCount, threshold)
        }
    }
    val near = nearEnd.value
    LaunchedEffect(near, hasNextPage, fetchingNextPage, query) {
        if (shouldFetchNextPage(near, hasNextPage, fetchingNextPage)) query.fetchNextPage()
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
