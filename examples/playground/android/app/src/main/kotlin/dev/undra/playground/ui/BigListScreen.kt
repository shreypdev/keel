package dev.undra.playground.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
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
import dev.undra.playground.core.BigList
import dev.undra.playground.core.Item
import dev.undra.playground.core.ListError
import dev.undra.runtime.UndraException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Owns the `BigList` store (ten thousand rows) and the switch that streams updates into it. */
class BigListViewModel : ViewModel() {
    /** The list in the core. */
    val store = BigList.create()

    /** Whether rows are being updated ten times a second. */
    var streaming by mutableStateOf(false)

    /** Why the last operation was refused, if it was. */
    var problem by mutableStateOf<String?>(null)

    /**
     * Runs a list operation. The core answers a position outside the list with a typed [ListError]; anything else
     * that goes wrong with the call is an `UndraCallError`. Both are an `UndraException` whose message reads well,
     * so one `catch` shows either to the user.
     */
    fun operate(operation: () -> Unit) {
        problem = try {
            operation()
            null
        } catch (e: UndraException) {
            e.message
        }
    }

    override fun onCleared() {
        store.close()
    }
}

/**
 * Ten thousand rows in a `LazyColumn`, and one-row operations on them. The list is a keyed signal in the core,
 * so inserting, updating, moving or removing a row crosses the boundary as a patch of one operation,
 * however long the list is; switch "Stream updates" on to see ten of them a second land on visible rows.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun BigListScreen(vm: BigListViewModel = viewModel()) {
    val store = vm.store
    val items by store.items.collectAsState()
    val count by store.count.collectAsState()
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()

    // Ten updates a second to a random row that is on screen. The effect restarts when the switch changes.
    LaunchedEffect(vm.streaming) {
        var tick = 0
        while (vm.streaming) {
            delay(STREAM_INTERVAL_MS)
            val row = listState.layoutInfo.visibleItemsInfo.randomOrNull()?.index ?: continue
            val item = store.items.value.getOrNull(row) ?: continue
            tick++
            vm.operate { store.updateAt(row.toUInt(), "Item ${item.id} · live update $tick") }
        }
    }

    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
        ScreenHeader("10k list", "%,d rows".format(count.toLong()), "biglist-count")
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            // "The first row" is the first one on screen, read when a button is pressed.
            Action("Insert top", "biglist-insert-top") { vm.operate { store.insertAt(0u, "Inserted at the top") } }
            Action("Insert middle", "biglist-insert-middle") {
                val middle = store.items.value.size / 2
                vm.operate { store.insertAt(middle.toUInt(), "Inserted in the middle") }
                scope.launch { listState.scrollToItem(middle) }
            }
            Action("Update", "biglist-update") {
                val first = listState.firstVisibleItemIndex
                val item = store.items.value.getOrNull(first) ?: return@Action
                vm.operate { store.updateAt(first.toUInt(), "Item ${item.id} · edited") }
            }
            Action("Move", "biglist-move") {
                val first = listState.firstVisibleItemIndex
                vm.operate { store.moveItem(first.toUInt(), (first + MOVE_BY).toUInt()) }
            }
            Action("Remove", "biglist-remove") { vm.operate { store.removeAt(listState.firstVisibleItemIndex.toUInt()) } }
            Action("Reset", "biglist-reset") {
                store.reset()
                scope.launch { listState.scrollToItem(0) }
            }
        }
        Row(Modifier.padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Switch(checked = vm.streaming, onCheckedChange = { vm.streaming = it }, modifier = Modifier.testTag("biglist-stream"))
            Text("Stream updates (10 per second)", modifier = Modifier.padding(start = 12.dp))
        }
        vm.problem?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.testTag("biglist-error")) }
        HorizontalDivider()
        LazyColumn(state = listState, modifier = Modifier.fillMaxSize().testTag("biglist-list")) {
            // Keyed by the row's identity, so a patch moves and updates rows instead of redrawing the window. The key is
            // a Long because a LazyColumn key has to fit in a Bundle, which a UInt does not.
            items(items, key = { it.id.toLong() }) { item -> ItemRow(item) }
        }
    }
}

@Composable
private fun Action(label: String, tag: String, onClick: () -> Unit) {
    FilledTonalButton(onClick = onClick, modifier = Modifier.testTag(tag)) { Text(label) }
}

@Composable
private fun ItemRow(item: Item) {
    Column {
        Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                "#${item.id}",
                style = MaterialTheme.typography.labelLarge,
                fontFamily = FontFamily.Monospace,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.width(72.dp),
            )
            Text(item.label, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
            Text(
                "v${item.version}",
                style = MaterialTheme.typography.labelLarge,
                color = if (item.version > 0u) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
    }
}

/** 10 updates a second. */
private const val STREAM_INTERVAL_MS = 100L

/** How many rows "Move" takes the first visible row down. */
private const val MOVE_BY = 5
