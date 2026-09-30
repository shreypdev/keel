package @@APP_ID@@

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import @@KOTLIN_PACKAGE@@.Filter
import @@KOTLIN_PACKAGE@@.TodoError
import @@KOTLIN_PACKAGE@@.Todos
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    // The store's state lives in the Rust core; closing the store releases its handle.
    private val todos by lazy { Todos() }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) { TodoScreen(todos) }
            }
        }
    }

    override fun onDestroy() {
        if (isFinishing) todos.close()
        super.onDestroy()
    }
}

/** Reading `todos.visible` is reading a StateFlow: the core pushes changes, Compose recomposes. */
@Composable
fun TodoScreen(todos: Todos) {
    val visible by todos.visible.collectAsState()
    val filter by todos.filter.collectAsState()
    val remaining by todos.remaining.collectAsState()
    val scope = rememberCoroutineScope()
    var draft by remember { mutableStateOf("") }
    var problem by remember { mutableStateOf<String?>(null) }

    fun add() {
        scope.launch {
            try {
                todos.add(draft)
                draft = ""
                problem = null
            } catch (e: TodoError) {
                problem = e.message
            }
        }
    }

    Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("$remaining left", style = MaterialTheme.typography.headlineMedium)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                label = { Text("What needs doing?") },
                singleLine = true,
                modifier = Modifier.weight(1f),
            )
            Button(onClick = ::add, enabled = draft.isNotBlank()) { Text("Add") }
        }
        problem?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Filter.entries.forEach { f ->
                FilterChip(
                    selected = filter == f,
                    onClick = { todos.setFilter(f) },
                    label = { Text(f.name.lowercase().replaceFirstChar { it.uppercase() }) },
                )
            }
            TextButton(onClick = { todos.clearDone() }) { Text("Clear done") }
        }
        LazyColumn(modifier = Modifier.fillMaxWidth()) {
            items(visible, key = { it.id }) { todo ->
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth().clickable { todos.toggle(todo.id) },
                ) {
                    Checkbox(checked = todo.done, onCheckedChange = { todos.toggle(todo.id) })
                    Text(todo.title)
                }
            }
        }
    }
}
