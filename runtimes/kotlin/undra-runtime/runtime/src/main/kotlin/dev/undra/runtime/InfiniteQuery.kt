package dev.undra.runtime

import kotlinx.coroutines.flow.StateFlow

/**
 * What every generated infinite-query handle offers besides its `data` (ADR-043 decision 4): whether the core has
 * another page, whether it is fetching one, and the command that asks for it.
 *
 * ```kotlin
 * val feed = FeedQuery(filter = Filter.All)           // implements InfiniteQuery
 * val posts by feed.data.collectAsState()
 * LazyColumn(state) { items(posts) { PostRow(it) } }
 * state.LoadMoreWhenNearEnd(feed)                      // from the optional undra-compose module
 * ```
 *
 * The interface is the seam for helpers that load more rows without knowing the query: the Compose helper
 * `LoadMoreWhenNearEnd` of the `undra-compose` module takes one.
 */
public interface InfiniteQuery {
    /** `true` while the core knows a cursor for a next page: `fetchNextPage()` will do something. */
    public val hasNextPage: StateFlow<Boolean>

    /** `true` while a next page is being fetched; the rows already loaded stay in `data`. */
    public val fetchingNextPage: StateFlow<Boolean>

    /**
     * Asks the core to fetch the next page and append it to `data`. A command (ADR-032): it returns at once, does
     * nothing when there is no next page or one is already being fetched, and reports a failure through the core's
     * error handler instead of throwing.
     */
    public fun fetchNextPage()
}
