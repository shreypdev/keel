package dev.undra.playground.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.undra.playground.core.Filter
import dev.undra.playground.core.Todo
import dev.undra.playground.core.TodoError
import dev.undra.playground.core.Todos
import dev.undra.runtime.UndraCallError
import kotlinx.coroutines.launch

/**
 * Owns the `Todos` store and what the screen types into it. The store is the core's object, not a copy of
 * its state: it lives as long as this ViewModel (across tab switches and rotation) and closing it releases
 * its handle in the core.
 */
class TodosViewModel(
    /** The to-do list in the core. A preview passes a store over a recorded core (`ScreenPreviews.kt`). */
    val store: Todos = Todos.create(),
) : ViewModel() {
    /** The text field. */
    var draft by mutableStateOf("")

    /** Why the last `add` was refused, for the field to show. */
    var problem by mutableStateOf<String?>(null)
        private set

    /**
     * Adds the draft. The core refuses a blank title with a typed error, which this turns into words; anything else
     * that goes wrong with the call (the core panicked, was closed, ...) is an `UndraCallError` that reads well as is.
     */
    fun add() {
        viewModelScope.launch {
            try {
                store.add(draft)
                draft = ""
                problem = null
            } catch (e: TodoError) {
                problem = when (e) {
                    TodoError.EmptyTitle -> "Give it a title first."
                }
            } catch (e: UndraCallError) {
                problem = e.message
            }
        }
    }

    override fun onCleared() {
        store.close()
    }
}

/** The to-do screen. Every value on it is a `StateFlow` of the store, read with `collectAsState()`. */
@Composable
fun TodosScreen(vm: TodosViewModel = viewModel()) {
    val store = vm.store
    val visible by store.visible.collectAsState()
    val all by store.todos.collectAsState()
    val filter by store.filter.collectAsState()
    val remaining by store.remaining.collectAsState()

    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp).imePadding()) {
        ScreenHeader("Todos", "$remaining left", "remaining")
        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(
                value = vm.draft,
                onValueChange = { vm.draft = it },
                label = { Text("What needs doing?") },
                isError = vm.problem != null,
                supportingText = vm.problem?.let { { Text(it, modifier = Modifier.testTag("todo-error")) } },
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { vm.add() }),
                modifier = Modifier.weight(1f).testTag("todo-input"),
            )
            Button(onClick = vm::add, modifier = Modifier.padding(top = 8.dp).testTag("todo-add")) { Text("Add") }
        }
        Row(Modifier.padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for (f in Filter.entries) {
                FilterChip(
                    selected = filter == f,
                    onClick = { store.setFilter(f) },
                    label = { Text(f.name.lowercase().replaceFirstChar { it.uppercase() }) },
                    modifier = Modifier.testTag("filter-${f.name.lowercase()}"),
                )
            }
            Box(Modifier.weight(1f))
            TextButton(onClick = store::clearDone, enabled = all.any { it.done }, modifier = Modifier.testTag("todo-clear-done")) {
                Text("Clear done")
            }
        }
        HorizontalDivider()
        if (visible.isEmpty()) {
            Text(
                if (all.isEmpty()) "Nothing to do yet." else "Nothing in this filter.",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 24.dp),
            )
        }
        LazyColumn(Modifier.fillMaxWidth().weight(1f)) {
            // The list is keyed by id in the core, so Compose moves and animates rows instead of redrawing them.
            items(visible, key = { it.id }) { todo ->
                TodoRow(todo, onToggle = { store.toggle(todo.id) }, onRemove = { store.remove(todo.id) })
            }
        }
    }
}

@Composable
private fun TodoRow(todo: Todo, onToggle: () -> Unit, onRemove: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onToggle).testTag("todo-row"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Checkbox(checked = todo.done, onCheckedChange = { onToggle() }, modifier = Modifier.testTag("todo-toggle"))
        Text(
            todo.title,
            style = MaterialTheme.typography.bodyLarge,
            textDecoration = if (todo.done) TextDecoration.LineThrough else TextDecoration.None,
            color = if (todo.done) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.weight(1f),
        )
        IconButton(onClick = onRemove, modifier = Modifier.testTag("todo-remove")) {
            Icon(Icons.Filled.Close, contentDescription = "Remove ${todo.title}")
        }
    }
}
