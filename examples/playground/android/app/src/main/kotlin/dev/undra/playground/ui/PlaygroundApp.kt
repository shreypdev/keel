package dev.undra.playground.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.AddCircle
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId

/** The five screens, in the order of the navigation bar. [id] is what the `tab` launch extra names. */
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
    ;

    /** Finds tabs by the name in the launch extra. */
    companion object {
        /** The tab called [id], or the first tab for anything else. */
        fun fromId(id: String?): Tab = entries.firstOrNull { it.id == id } ?: TODOS
    }
}

/** The app: a bottom navigation bar over the five screens. Every screen keeps its store while another tab is shown. */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
fun PlaygroundApp(tab: Tab, onTab: (Tab) -> Unit) {
    Scaffold(
        // The test tags double as resource ids, so `uiautomator dump` and UI tests can find the controls.
        modifier = Modifier.semantics { testTagsAsResourceId = true },
        bottomBar = {
            NavigationBar {
                for (entry in Tab.entries) {
                    NavigationBarItem(
                        selected = tab == entry,
                        onClick = { onTab(entry) },
                        icon = { Icon(entry.icon, contentDescription = null) },
                        label = { Text(entry.label) },
                        modifier = Modifier.testTag("tab-${entry.id}"),
                    )
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
            }
        }
    }
}
