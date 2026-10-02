package dev.undra.playground.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.undra.compose.items
import dev.undra.playground.core.Item
import dev.undra.playground.core.Library
import dev.undra.runtime.UndraException
import kotlinx.coroutines.launch

/** Owns the `Library` store: ten thousand books the core keeps and a small list the app receives whole, with a lazy view of its even rows. */
class LibraryViewModel : ViewModel() {
    /** The store in the core. */
    val library = Library.create()

    /** Why the last operation was refused, if it was. */
    var problem by mutableStateOf<String?>(null)

    /** Runs a write; a refusal of the core (a typed `ListError`, or an `UndraCallError`) reads well in its message. */
    fun operate(operation: () -> Unit) {
        problem = try {
            operation()
            null
        } catch (e: UndraException) {
            e.message
        }
    }

    override fun onCleared() {
        library.close()
    }
}

/**
 * Two lazily paged lists. `books` has ten thousand rows that never cross the boundary whole: the screen holds the length and the
 * pages around what it shows, a row that is not here yet is drawn as a placeholder, and a change in the core costs the pages on
 * screen, not the list. `evens` is the core's read-only view of the even rows of a small list, paged the same way.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun LibraryScreen(vm: LibraryViewModel = viewModel()) {
    val library = vm.library
    val books by library.books.size.collectAsState()
    val evens by library.evens.size.collectAsState()
    val booksState = rememberLazyListState()
    val scope = rememberCoroutineScope()

    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
        ScreenHeader("Library", "%,d books".format(books.toLong()), "library-count")
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Action("Add 100", "library-add") { vm.operate { library.addRows(100u) } }
            Action("Rename", "library-rename") {
                val first = booksState.firstVisibleItemIndex
                vm.operate { library.rename(first.toUInt(), "Renamed at $first") }
            }
            Action("Remove", "library-remove") { vm.operate { library.removeAt(booksState.firstVisibleItemIndex.toUInt()) } }
            Action("Reset", "library-reset") {
                vm.operate { library.reset(LIBRARY_LEN) }
                scope.launch { booksState.scrollToItem(0) }
            }
            Action("Jump to the end", "library-end") { scope.launch { booksState.scrollToItem(books - 1) } }
        }
        vm.problem?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.testTag("library-error")) }
        HorizontalDivider()
        LazyColumn(state = booksState, modifier = Modifier.weight(1f).fillMaxWidth().testTag("library-list")) {
            // One item per row, however many there are; a row is `null` while its page loads.
            items(library.books) { index, book -> BookRow(index, book, "library-book") }
        }
        HorizontalDivider()
        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                "Evens: a view of the even rows of the source list (%d)".format(evens),
                style = MaterialTheme.typography.titleSmall,
                modifier = Modifier.weight(1f).testTag("library-evens-count"),
            )
            Action("Add 2", "library-evens-add") { vm.operate { library.addRows(2u) } }
            Action("Drop 5", "library-evens-drop") { vm.operate { library.dropSource(5u) } }
        }
        LazyColumn(Modifier.fillMaxWidth().height(EVENS_HEIGHT).testTag("library-evens-list")) {
            items(library.evens) { index, item -> BookRow(index, item, "library-even") }
        }
    }
}

@Composable
private fun Action(label: String, tag: String, onClick: () -> Unit) {
    FilledTonalButton(onClick = onClick, modifier = Modifier.testTag(tag)) { Text(label) }
}

/** One row of a lazily paged list: the item, or a placeholder of the same height while its page is on its way. */
@Composable
private fun BookRow(index: Int, item: Item?, tag: String) {
    Column {
        Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                "${index + 1}",
                style = MaterialTheme.typography.labelLarge,
                fontFamily = FontFamily.Monospace,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.width(56.dp),
            )
            if (item == null) {
                Text(
                    "Loading…",
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.outline,
                    modifier = Modifier.weight(1f).testTag("$tag-placeholder"),
                )
            } else {
                Text(item.label, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f).testTag(tag))
                Text(
                    "v${item.version}",
                    style = MaterialTheme.typography.labelLarge,
                    color = if (item.version > 0u) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
    }
}

/** How many books a fresh library (and Reset) has. */
private const val LIBRARY_LEN = 10_000u

/** The height of the `evens` list. */
private val EVENS_HEIGHT = 144.dp
