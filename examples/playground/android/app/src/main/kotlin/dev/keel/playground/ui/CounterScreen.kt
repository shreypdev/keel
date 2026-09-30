package dev.keel.playground.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AssistChip
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.keel.playground.core.Counter
import dev.keel.playground.core.Parity

/** Owns the `Counter` store, so the count survives switching tabs. */
class CounterViewModel : ViewModel() {
    /** The counter in the core. */
    val store = Counter.create()

    override fun onCleared() {
        store.close()
    }
}

/**
 * The counter screen. `count`, `changes` and the computed `parity` are three signals, yet one tap
 * crosses the boundary once: the core writes them in one transaction and sends one change-set.
 */
@Composable
fun CounterScreen(vm: CounterViewModel = viewModel()) {
    val store = vm.store
    val count by store.count.collectAsState()
    val changes by store.changes.collectAsState()
    val parity by store.parity.collectAsState()

    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp), horizontalAlignment = Alignment.Start) {
        ScreenHeader("Counter", "$changes ${if (changes == 1u) "change" else "changes"}", "counter-changes")
        Column(Modifier.weight(1f).fillMaxSize(), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                count.toString(),
                fontSize = 96.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.testTag("counter-value"),
            )
            AssistChip(
                onClick = {},
                label = { Text(if (parity == Parity.EVEN) "even" else "odd") },
                modifier = Modifier.testTag("counter-parity"),
            )
            Spacer(Modifier.height(32.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(24.dp), verticalAlignment = Alignment.CenterVertically) {
                FilledTonalIconButton(
                    onClick = store::decrement,
                    modifier = Modifier.size(72.dp).testTag("counter-dec").semantics { contentDescription = "Decrement" },
                ) { Text("−", fontSize = 32.sp) }
                FilledIconButton(
                    onClick = store::increment,
                    modifier = Modifier.size(72.dp).testTag("counter-inc").semantics { contentDescription = "Increment" },
                ) { Text("+", fontSize = 32.sp) }
            }
            Spacer(Modifier.height(24.dp))
            OutlinedButton(onClick = store::reset, modifier = Modifier.testTag("counter-reset")) { Text("Reset") }
        }
        Text(
            "Each tap is one transaction in the core: count, changes and the computed parity arrive in one change-set.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(bottom = 16.dp),
        )
    }
}
