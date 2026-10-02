package dev.undra.playground.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.undra.compose.LoadMoreWhenNearEnd
import dev.undra.playground.core.FeedQueryHandle
import dev.undra.playground.core.Item
import dev.undra.playground.core.QueryStatus

/** Owns the query handle of the feed: the core fetches a page of fifty rows at a time and the handle's `data` grows by them. */
class FeedViewModel : ViewModel() {
    /** The infinite query over every row of the big list; constructing it fetches the first page. */
    val query = FeedQueryHandle.create(evenOnly = false)

    override fun onCleared() {
        query.close()
    }
}

/**
 * An infinite feed: a `LazyColumn` over the rows the core's infinite query has loaded. Scrolling within five rows of the end asks
 * the core for the next page (`LoadMoreWhenNearEnd`), the footer says what it is doing, and Refresh fetches the pages again: only the
 * rows that changed cross the boundary.
 */
@Composable
fun FeedScreen(vm: FeedViewModel = viewModel()) {
    val query = vm.query
    val rows by query.data.collectAsState()
    val status by query.status.collectAsState()
    val fetching by query.fetching.collectAsState()
    val hasNextPage by query.hasNextPage.collectAsState()
    val fetchingNextPage by query.fetchingNextPage.collectAsState()
    val error by query.error.collectAsState()
    val listState = rememberLazyListState()

    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
        ScreenHeader("Feed", "%,d rows".format(rows.size.toLong()), "feed-count")
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            if (fetching) {
                CircularProgressIndicator(Modifier.size(20.dp).testTag("feed-fetching"), strokeWidth = 2.dp)
            } else {
                Spacer(Modifier.size(20.dp))
            }
            Text(statusWord(status), modifier = Modifier.weight(1f).testTag("feed-status"), style = MaterialTheme.typography.bodyMedium)
            OutlinedButton(onClick = query::refetch, modifier = Modifier.testTag("feed-refresh")) { Text("Refresh") }
        }
        error?.let { Text(it.message.orEmpty(), color = MaterialTheme.colorScheme.error, modifier = Modifier.testTag("feed-error")) }
        HorizontalDivider(Modifier.padding(top = 8.dp))
        if (rows.isEmpty()) {
            Text(
                if (error == null) "Loading the first page…" else "The first page did not load.",
                modifier = Modifier.padding(top = 16.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        LazyColumn(state = listState, modifier = Modifier.fillMaxSize().testTag("feed-list")) {
            // Keyed by the row's identity (a Long: a LazyColumn key has to fit in a Bundle, which a UInt does not).
            items(rows, key = { it.id.toLong() }) { item -> FeedRow(item) }
            // Not before the first page: a footer that is the only item is the one the list keeps in place by its key when the
            // rows arrive above it, which would scroll the screen to the end of the first page.
            if (rows.isNotEmpty()) item(key = "footer") { FeedFooter(hasNextPage, fetchingNextPage) }
        }
        listState.LoadMoreWhenNearEnd(query)
    }
}

@Composable
private fun FeedRow(item: Item) {
    Column {
        Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                "#${item.id}",
                style = MaterialTheme.typography.labelLarge,
                fontFamily = FontFamily.Monospace,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.width(72.dp),
            )
            Text(item.label, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f).testTag("feed-row"))
            Text(
                "v${item.version}",
                style = MaterialTheme.typography.labelLarge,
                color = if (item.version > 0u) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
    }
}

/** What the next page is doing: on its way, there to be had by scrolling, or no more. */
@Composable
private fun FeedFooter(hasNextPage: Boolean, fetchingNextPage: Boolean) {
    Row(
        Modifier.fillMaxWidth().padding(vertical = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.Center,
    ) {
        if (fetchingNextPage) {
            CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
            Spacer(Modifier.width(12.dp))
        }
        Text(
            when {
                fetchingNextPage -> "Loading more…"
                hasNextPage -> "Scroll for more"
                else -> "That is every row"
            },
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.testTag("feed-footer"),
        )
    }
}

private fun statusWord(status: QueryStatus): String =
    when (status) {
        QueryStatus.IDLE -> "Idle"
        QueryStatus.FETCHING -> "Fetching"
        QueryStatus.SUCCESS -> "Success"
        QueryStatus.ERROR -> "Error"
    }
