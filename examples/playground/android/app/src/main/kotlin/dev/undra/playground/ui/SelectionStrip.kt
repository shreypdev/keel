package dev.undra.playground.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp

/**
 * The strip under the to-do and the note list: how many rows are ticked, the newest row and the commands of the
 * selection store. `TodoSelection` and `NoteSelection` are one generic `Selection<T>` of the core (ADR-058),
 * instantiated twice, so the two screens show the same strip over two different stores.
 *
 * @param kind `"todo"` or `"note"`: the prefix of the test tags.
 * @param removeSelected an extra command of the screen (the to-do list removes what is ticked), or null.
 */
@Composable
fun SelectionStrip(
    kind: String,
    count: UInt,
    latest: String?,
    picked: List<String>,
    onSelectAll: () -> Unit,
    onClear: () -> Unit,
    onNewDraft: () -> Unit,
    removeSelected: (() -> Unit)? = null,
) {
    Column(Modifier.padding(vertical = 4.dp).testTag("$kind-selection")) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("$count selected", style = MaterialTheme.typography.labelLarge, modifier = Modifier.testTag("$kind-selected-count"))
            Text(
                latest?.let { "Latest: $it" } ?: "Nothing yet",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.testTag("$kind-latest"),
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            TextButton(onClick = onSelectAll, modifier = Modifier.testTag("$kind-select-all")) { Text("Select all") }
            TextButton(onClick = onClear, enabled = count > 0u, modifier = Modifier.testTag("$kind-select-clear")) { Text("Clear") }
            if (removeSelected != null) {
                TextButton(onClick = removeSelected, enabled = count > 0u, modifier = Modifier.testTag("$kind-remove-selected")) {
                    Text("Remove selected")
                }
            }
            TextButton(onClick = onNewDraft, modifier = Modifier.testTag("$kind-new-draft")) { Text("New draft") }
        }
        if (picked.isNotEmpty()) {
            Text(
                picked.joinToString(", "),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.testTag("$kind-picked"),
            )
        }
    }
}
