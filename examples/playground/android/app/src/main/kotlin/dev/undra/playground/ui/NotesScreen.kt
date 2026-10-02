package dev.undra.playground.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
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
import dev.undra.playground.core.Note
import dev.undra.playground.core.Notes
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.adapters.DbError
import kotlinx.coroutines.launch

/**
 * Owns the `Notes` store: a list the core keeps in SQLite through the opt-in `Db` port (ADR-048). On Android that is the
 * platform's own SQLite (`AndroidDbAdapter`, file `undra-playground.sqlite` under the app's database directory), so the notes
 * survive the process. The store's list changes only after the database did.
 */
class NotesViewModel : ViewModel() {
    /** The notes in the core. */
    val store = Notes.create()

    /** The text field. */
    var draft by mutableStateOf("")

    /** What the database said last, when it refused something (or could not be opened). */
    var problem by mutableStateOf<String?>(null)
        private set

    /** Whether the database is open (the migrations ran). */
    var ready by mutableStateOf(false)
        private set

    init {
        // Opens (creating it the first time) and migrates the database, then loads every note into the store.
        perform { store.open(DATABASE) }
    }

    /** Adds the draft as a note. */
    fun add() {
        val title = draft.trim()
        if (title.isEmpty()) {
            problem = "Write something first."
            return
        }
        perform {
            store.add(title)
            draft = ""
        }
    }

    /** Ticks or unticks note [id]. */
    fun toggle(id: Long) = perform { store.toggle(id) }

    /** Removes note [id]. */
    fun remove(id: Long) = perform { store.remove(id) }

    /** Runs a call of the store, turning the typed `DbError` (and any failed call) into words for the screen. */
    private fun perform(call: suspend () -> Unit) {
        viewModelScope.launch {
            try {
                call()
                ready = true
                problem = null
            } catch (e: DbError) {
                problem = e.message
            } catch (e: UndraCallError) {
                problem = e.message
            }
        }
    }

    override fun onCleared() {
        store.close()
    }

    private companion object {
        /** The database's name: the file is `undra-playground.sqlite`. */
        const val DATABASE = "playground"
    }
}

/** The notes screen: the store's `notes` and `version`, read with `collectAsState()`. */
@Composable
fun NotesScreen(vm: NotesViewModel = viewModel()) {
    val notes by vm.store.notes.collectAsState()
    val version by vm.store.version.collectAsState()
    val open = notes.count { !it.done }

    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp).imePadding()) {
        ScreenHeader("Notes · SQLite v$version", "$open open", "notes-open")
        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(
                value = vm.draft,
                onValueChange = { vm.draft = it },
                label = { Text("A note") },
                isError = vm.problem != null,
                supportingText = vm.problem?.let { { Text(it, modifier = Modifier.testTag("notes-error")) } },
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { vm.add() }),
                modifier = Modifier.weight(1f).testTag("notes-input"),
            )
            Button(onClick = vm::add, enabled = vm.ready, modifier = Modifier.padding(top = 8.dp).testTag("notes-add")) { Text("Add") }
        }
        HorizontalDivider(Modifier.padding(top = 4.dp))
        if (notes.isEmpty()) {
            Text(
                if (vm.ready) "No notes yet. They are kept in SQLite, so they are still here after a restart." else "Opening the database…",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 24.dp).testTag("notes-empty"),
            )
        }
        LazyColumn(Modifier.fillMaxWidth().weight(1f)) {
            // The list is keyed by the row id, as the core's signal is.
            items(notes, key = { it.id }) { note ->
                NoteRow(note, onToggle = { vm.toggle(note.id) }, onRemove = { vm.remove(note.id) })
            }
        }
    }
}

@Composable
private fun NoteRow(note: Note, onToggle: () -> Unit, onRemove: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onToggle).testTag("notes-row"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Checkbox(checked = note.done, onCheckedChange = { onToggle() }, modifier = Modifier.testTag("notes-toggle"))
        Text(
            note.title,
            style = MaterialTheme.typography.bodyLarge,
            textDecoration = if (note.done) TextDecoration.LineThrough else TextDecoration.None,
            color = if (note.done) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.weight(1f),
        )
        IconButton(onClick = onRemove, modifier = Modifier.testTag("notes-remove")) {
            Icon(Icons.Filled.Close, contentDescription = "Remove ${note.title}")
        }
    }
}
