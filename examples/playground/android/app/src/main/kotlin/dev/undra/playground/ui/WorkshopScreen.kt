package dev.undra.playground.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.undra.playground.core.ReportError
import dev.undra.playground.core.Reporter
import dev.undra.playground.core.Shelf
import dev.undra.playground.core.Watch
import dev.undra.playground.core.Workshop
import dev.undra.runtime.UndraCallError
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * Owns a `Workshop` and the two shelves it hands out, and is the app's `Reporter`: the core calls [reporter] on the
 * main thread, in order with the stores' changes, so the progress and the notes it shows are always in step with
 * the shelves above them.
 */
class WorkshopViewModel : ViewModel() {
    /** The workshop in the core (a store). */
    val workshop = Workshop()

    /** A child store the workshop returns: `shelf("left")` is the same wrapper every time (ADR-040). */
    val left: Shelf = workshop.shelf("left")

    /** The other shelf. */
    val right: Shelf = workshop.shelf("right")

    private val _progress = MutableStateFlow(0f)

    /** How far the last job got, from the reporter's `progress` calls. */
    val progress: StateFlow<Float> = _progress.asStateFlow()

    private val _notes = MutableStateFlow<List<String>>(emptyList())

    /** The newest notes the core sent the reporter, oldest first. */
    val notes: StateFlow<List<String>> = _notes.asStateFlow()

    private val _approve = MutableStateFlow(true)

    /** What the reporter answers when a job asks whether to go on. */
    val approve: StateFlow<Boolean> = _approve.asStateFlow()

    private val _outcome = MutableStateFlow("")

    /** How the last job ended. */
    val outcome: StateFlow<String> = _outcome.asStateFlow()

    /** The app's implementation of the core's `Reporter` callback interface (ADR-041). */
    private val reporter = object : Reporter {
        override fun progress(done: UInt, total: UInt) {
            _progress.value = if (total == 0u) 0f else done.toFloat() / total.toFloat()
        }

        override fun note(line: String) {
            _notes.value = (_notes.value + line).takeLast(MAX_NOTES)
        }

        override suspend fun confirm(question: String): Boolean = _approve.value
    }

    /**
     * The subscription that keeps [reporter] in the core: `announce` tells it. The reporter refers back to this
     * ViewModel, so the ViewModel closes the subscription when it goes (and the core lets go of the reporter).
     */
    private val watch: Watch = workshop.watch(reporter)

    /** Puts one item on [shelf] (a method of the child store). */
    fun stock(shelf: Shelf) = shelf.stock(1u)

    /** Moves every item of the left shelf onto the right one: a workshop method that takes two of its stores. */
    fun mergeLeftIntoRight() = workshop.merge(left, right)

    /** Tells every subscribed reporter something; ours shows it. */
    fun announce() {
        workshop.announce("${left.items.value} left, ${right.items.value} right")
    }

    /** Runs a five-step job that reports to [reporter] and asks it whether to go on. */
    fun runJob() {
        viewModelScope.launch {
            _outcome.value = "running…"
            _outcome.value = try {
                "done: ${workshop.run(5u, reporter)} steps"
            } catch (e: ReportError) {
                when (e) {
                    ReportError.Declined -> "declined"
                    is ReportError.Unavailable -> "the reporter failed: ${e.value}"
                }
            } catch (e: UndraCallError) {
                e.message.orEmpty()
            }
        }
    }

    /** Sets what the reporter answers. */
    fun setApprove(approve: Boolean) {
        _approve.value = approve
    }

    override fun onCleared() {
        watch.close()
        left.close()
        right.close()
        workshop.close()
    }

    private companion object {
        const val MAX_NOTES = 5
    }
}

/**
 * The workshop: objects as parameters and returns (two shelves the workshop hands out and takes back) and a host
 * callback (the reporter the core calls with progress, notes and a question).
 */
@Composable
fun WorkshopScreen(vm: WorkshopViewModel = viewModel()) {
    val leftItems by vm.left.items.collectAsState()
    val rightItems by vm.right.items.collectAsState()
    val jobs by vm.workshop.jobs.collectAsState()
    val progress by vm.progress.collectAsState()
    val notes by vm.notes.collectAsState()
    val approve by vm.approve.collectAsState()
    val outcome by vm.outcome.collectAsState()

    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        ScreenHeader("Workshop", "${leftItems + rightItems} items", "workshop-items")
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            ShelfCard("Left", leftItems, "shelf-left", Modifier.weight(1f)) { vm.stock(vm.left) }
            ShelfCard("Right", rightItems, "shelf-right", Modifier.weight(1f)) { vm.stock(vm.right) }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(onClick = vm::mergeLeftIntoRight, modifier = Modifier.testTag("workshop-merge")) { Text("Merge left into right") }
            OutlinedButton(onClick = vm::announce, modifier = Modifier.testTag("workshop-announce")) { Text("Announce") }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(onClick = vm::runJob, modifier = Modifier.testTag("workshop-run")) { Text("Run a job") }
            Switch(checked = approve, onCheckedChange = vm::setApprove, modifier = Modifier.testTag("workshop-approve"))
            Text(if (approve) "the reporter says go on" else "the reporter declines")
        }
        LinearProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth().testTag("workshop-progress"))
        Text("$jobs jobs run. $outcome", modifier = Modifier.testTag("workshop-outcome"))
        Text("Reporter notes", style = MaterialTheme.typography.titleSmall)
        for (note in notes) Text(note, style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun ShelfCard(label: String, items: UInt, tag: String, modifier: Modifier, onStock: () -> Unit) {
    Card(modifier) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(label, style = MaterialTheme.typography.labelLarge)
            Text("$items", style = MaterialTheme.typography.headlineMedium, modifier = Modifier.testTag(tag))
            OutlinedButton(onClick = onStock, modifier = Modifier.testTag("$tag-stock")) { Text("Stock 1") }
        }
    }
}
