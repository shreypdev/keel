package dev.undra.playground.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.AddCircle
import androidx.compose.material.icons.filled.Build
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Star
import androidx.compose.material3.Icon
import androidx.compose.material3.Scaffold
import androidx.compose.material3.ScrollableTabRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab as TabItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.unit.dp

/** The screens, in the order of the navigation bar. [id] is what the `tab` launch extra names. */
enum class Tab(val id: String, val label: String, val icon: ImageVector) {
    /** The to-do list: a store with a filter and two computed values. */
    TODOS("todos", "Todos", Icons.Filled.CheckCircle),

    /** The counter: one transaction moves three signals. */
    COUNTER("counter", "Counter", Icons.Filled.AddCircle),

    /** Ten thousand keyed rows and one-row patches. */
    BIGLIST("biglist", "10k list", Icons.Filled.Menu),

    /** A cached server list with optimistic updates and an offline queue. */
    REMOTE("remote", "Remote", Icons.Filled.Refresh),

    /** Notes kept in SQLite through the opt-in `Db` port (ADR-048). */
    NOTES("notes", "Notes", Icons.Filled.Edit),

    /** A workshop that hands out shelves and takes them back, and calls the app's reporter. */
    WORKSHOP("workshop", "Workshop", Icons.Filled.Build),

    /** Ten thousand books the core keeps and the screen pages through (`Lazy<T>`, ADR-043). */
    LIBRARY("library", "Library", Icons.Filled.Favorite),

    /** An infinite query: a feed that grows a page at a time as it is scrolled. */
    FEED("feed", "Feed", Icons.Filled.Star),

    /** A query that polls: a counter the core fetches again every second while the screen is shown. */
    TICKER("ticker", "Ticker", Icons.Filled.PlayArrow),
    ;

    /** Finds tabs by the name in the launch extra. */
    companion object {
        /** The tab called [id], or the first tab for anything else. */
        fun fromId(id: String?): Tab = entries.firstOrNull { it.id == id } ?: TODOS
    }
}

/** The app: a bottom navigation bar over the screens. Every screen keeps its store while another tab is shown. */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
fun PlaygroundApp(tab: Tab, onTab: (Tab) -> Unit) {
    Scaffold(
        // The test tags double as resource ids, so `uiautomator dump` and UI tests can find the controls.
        modifier = Modifier.semantics { testTagsAsResourceId = true },
        bottomBar = {
            // Nine tabs do not fit a phone's width as a NavigationBar (it gives each item an equal share), so the bar scrolls.
            Surface(tonalElevation = 3.dp) {
                ScrollableTabRow(
                    selectedTabIndex = tab.ordinal,
                    edgePadding = 0.dp,
                    containerColor = Color.Transparent,
                    modifier = Modifier.navigationBarsPadding(),
                ) {
                    for (entry in Tab.entries) {
                        TabItem(
                            selected = tab == entry,
                            onClick = { onTab(entry) },
                            icon = { Icon(entry.icon, contentDescription = null) },
                            text = { Text(entry.label, maxLines = 1) },
                            modifier = Modifier.testTag("tab-${entry.id}"),
                        )
                    }
                }
            }
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            when (tab) {
                Tab.TODOS -> TodosScreen()
                Tab.COUNTER -> CounterScreen()
                Tab.BIGLIST -> BigListScreen()
                Tab.REMOTE -> RemoteScreen()
                Tab.NOTES -> NotesScreen()
                Tab.WORKSHOP -> WorkshopScreen()
                Tab.LIBRARY -> LibraryScreen()
                Tab.FEED -> FeedScreen()
                Tab.TICKER -> TickerScreen()
            }
        }
    }
}
