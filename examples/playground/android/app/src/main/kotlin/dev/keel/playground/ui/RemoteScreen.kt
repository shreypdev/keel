package dev.keel.playground.ui

import android.app.Application
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.keel.playground.KeelApp
import dev.keel.playground.core.QueryStatus
import dev.keel.playground.core.RemoteError
import dev.keel.playground.core.RemoteTodo
import dev.keel.playground.core.RemoteTodosQueryHandle
import dev.keel.playground.core.createRemoteTodo
import dev.keel.playground.core.setRemoteDone
import dev.keel.runtime.adapters.NetKind
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlinx.coroutines.launch

/** The server list this screen shows; every list has its own cache entry in the core. */
private const val LIST = "inbox"

/**
 * Optimistic placeholders are numbered down from `u32::MAX` until the server has answered for them
 * (`RemoteTodo.id` in the core); anything this high is not the server's yet.
 */
private const val PLACEHOLDER_FLOOR = 0xFFFF_0000u

/**
 * Owns the query handle of the `inbox` list and the calls made on it. The data, the status, the error and the
 * fetch timing are the core's (`RemoteTodosQueryHandle`); this holds only what the UI adds to them.
 */
class RemoteViewModel(app: Application) : AndroidViewModel(app) {
    private val keel = app as KeelApp

    /** The cached `inbox` list: constructing it starts a fetch if the data is missing or stale. */
    val query = RemoteTodosQueryHandle.create(LIST)

    /** The text field. */
    var draft by mutableStateOf("")

    /** The Offline switch. */
    var offline by mutableStateOf(false)
        private set

    /** Why the last mutation failed, if it did. */
    var problem by mutableStateOf<String?>(null)
        private set

    /**
     * Adds the draft. The core shows it at once as a placeholder and asks the server; offline, the request is
     * queued and this call keeps waiting until the network returns and the queue is replayed.
     */
    fun add() {
        val title = draft.trim()
        if (title.isEmpty()) return
        draft = ""
        viewModelScope.launch { attempt { createRemoteTodo(LIST, title) } }
    }

    /** Flips an item's `done` flag, optimistically: the row changes now and changes back if the server refuses. */
    fun toggle(todo: RemoteTodo) {
        viewModelScope.launch { attempt { setRemoteDone(LIST, todo.id, !todo.done) } }
    }

    /** Fetches now, even if the data is fresh. */
    fun refresh() {
        query.refetch()
    }

    /**
     * Takes the network away or gives it back. The demo server fails its requests, and the core is told
     * through the `Connectivity` port, which is what makes it queue idempotent mutations and, when the network
     * is back, replay them and refetch.
     */
    fun setOfflineMode(on: Boolean) {
        offline = on
        keel.server.offline = on
        keel.connectivity.changed(online = !on, kind = if (on) NetKind.NONE else NetKind.WIFI)
    }

    private suspend fun attempt(mutation: suspend () -> Unit) {
        problem = try {
            mutation()
            null
        } catch (e: RemoteError) {
            describe(e)
        }
    }

    private fun describe(e: RemoteError): String =
        when (e) {
            RemoteError.NotConfigured -> "The server address is not configured."
            is RemoteError.Http -> "Network problem: ${e.cause.message}."
            is RemoteError.Status -> "The server answered ${e.code}."
            is RemoteError.BadBody -> "The server sent something unexpected."
        }

    override fun onCleared() {
        query.close()
    }
}

/** The remote screen: a cached server list with optimistic updates, and a switch that takes the network away. */
@Composable
fun RemoteScreen(vm: RemoteViewModel = viewModel()) {
    val query = vm.query
    val data by query.data.collectAsState()
    val status by query.status.collectAsState()
    val fetching by query.fetching.collectAsState()
    val error by query.error.collectAsState()
    val updatedAt by query.updatedAt.collectAsState()
    val clock = remember { SimpleDateFormat("HH:mm:ss", Locale.getDefault()) }

    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp).imePadding()) {
        ScreenHeader("Remote", statusWord(status), "remote-status")
        Card(
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (fetching) {
                        CircularProgressIndicator(Modifier.size(20.dp).testTag("remote-fetching"), strokeWidth = 2.dp)
                    } else {
                        Spacer(Modifier.size(20.dp))
                    }
                    Text(
                        updatedAt?.let { "Updated ${clock.format(Date(it.epochMillis))}" } ?: "Not fetched yet",
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.weight(1f).testTag("remote-updated"),
                    )
                    OutlinedButton(onClick = vm::refresh, modifier = Modifier.testTag("remote-refresh")) { Text("Refresh") }
                }
                error?.let {
                    Text(
                        "Last fetch failed: ${it.message}",
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.testTag("remote-error"),
                    )
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Switch(checked = vm.offline, onCheckedChange = vm::setOfflineMode, modifier = Modifier.testTag("remote-offline"))
                    Text("Offline", modifier = Modifier.padding(start = 12.dp))
                }
                Text(
                    "Offline, every request fails and the core is told the network is gone. What you add is queued " +
                        "and replayed when you switch back.",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        Row(Modifier.padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(
                value = vm.draft,
                onValueChange = { vm.draft = it },
                label = { Text("Add to the inbox") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { vm.add() }),
                modifier = Modifier.weight(1f).testTag("remote-input"),
            )
            Button(onClick = vm::add, enabled = vm.draft.isNotBlank(), modifier = Modifier.testTag("remote-add")) { Text("Add") }
        }
        vm.problem?.let {
            Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(top = 4.dp).testTag("remote-problem"))
        }
        val list = data
        if (list == null) {
            Text("Loading…", modifier = Modifier.padding(top = 16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        } else if (list.isEmpty()) {
            Text("The list is empty.", modifier = Modifier.padding(top = 16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        LazyColumn(Modifier.fillMaxSize().padding(top = 8.dp)) {
            items(list.orEmpty(), key = { it.id.toLong() }) { todo ->
                RemoteRow(todo, offline = vm.offline, onToggle = { vm.toggle(todo) })
            }
        }
    }
}

@Composable
private fun RemoteRow(todo: RemoteTodo, offline: Boolean, onToggle: () -> Unit) {
    val placeholder = todo.id >= PLACEHOLDER_FLOOR
    Column {
        Row(Modifier.fillMaxWidth().testTag("remote-row"), verticalAlignment = Alignment.CenterVertically) {
            Checkbox(checked = todo.done, onCheckedChange = { onToggle() }, enabled = !placeholder, modifier = Modifier.testTag("remote-toggle"))
            Text(todo.title, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
            if (placeholder) {
                Text(
                    if (offline) "queued" else "sending…",
                    style = MaterialTheme.typography.labelLarge,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.testTag("remote-pending"),
                )
            }
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
    }
}

/** The status of a query as a word for the headline. */
private fun statusWord(status: QueryStatus): String =
    when (status) {
        QueryStatus.IDLE -> "Idle"
        QueryStatus.FETCHING -> "Fetching"
        QueryStatus.SUCCESS -> "Success"
        QueryStatus.ERROR -> "Error"
    }
