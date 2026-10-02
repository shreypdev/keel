package dev.undra.playground.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.RememberObserver
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import dev.undra.playground.core.QueryStatus
import dev.undra.playground.core.TickerQueryHandle
import dev.undra.playground.core.setTickerFailing
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlin.time.Duration.Companion.seconds

/**
 * Observes the `ticker` query for as long as the screen is on it: the handle is made when the screen enters the composition and
 * closed when it leaves, so the core polls only while somebody is looking (the last observer stops it, ADR-043).
 */
private class TickerObserver : RememberObserver {
    val query = TickerQueryHandle.create()

    override fun onRemembered() = Unit

    override fun onForgotten() = query.close()

    override fun onAbandoned() = query.close()
}

/**
 * A query that polls: the core fetches the ticker's counter again a second after the last fetch ended, while this screen is shown and
 * the app is active (backgrounding the app or losing the network pauses it; coming back resumes it). The switch asks for a slower
 * interval for as long as the screen is visible, and Fail on purpose makes the fetches fail without stopping the polling.
 */
@Composable
fun TickerScreen() {
    val observer = remember { TickerObserver() }
    val query = observer.query
    val counter by query.data.collectAsState()
    val status by query.status.collectAsState()
    val fetching by query.fetching.collectAsState()
    val error by query.error.collectAsState()
    val updatedAt by query.updatedAt.collectAsState()
    var slow by remember { mutableStateOf(false) }
    var failing by remember { mutableStateOf(false) }
    val clock = remember { SimpleDateFormat("HH:mm:ss", Locale.getDefault()) }

    // The override is this handle's and lasts while the screen is visible; clearing it returns the query to its own second.
    DisposableEffect(query, slow) {
        query.setPollInterval(if (slow) SLOW_INTERVAL else null)
        onDispose { query.setPollInterval(null) }
    }
    DisposableEffect(failing) {
        setTickerFailing(failing)
        onDispose { setTickerFailing(false) }
    }

    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
        ScreenHeader("Ticker", (counter ?: 0u).toString(), "ticker-count")
        Card(
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer),
            modifier = Modifier.padding(top = 4.dp),
        ) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (fetching) {
                        CircularProgressIndicator(Modifier.size(20.dp).testTag("ticker-fetching"), strokeWidth = 2.dp)
                    } else {
                        Spacer(Modifier.size(20.dp))
                    }
                    Text(statusWord(status), modifier = Modifier.weight(1f).testTag("ticker-status"), style = MaterialTheme.typography.bodyMedium)
                    OutlinedButton(onClick = query::refetch, modifier = Modifier.testTag("ticker-refetch")) { Text("Fetch now") }
                }
                Text(
                    updatedAt?.let { "Updated ${clock.format(Date(it.epochMillis))}" } ?: "Not fetched yet",
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.testTag("ticker-updated"),
                )
                error?.let {
                    Text(
                        "Last fetch failed: ${it.message}",
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.testTag("ticker-error"),
                    )
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Switch(checked = slow, onCheckedChange = { slow = it }, modifier = Modifier.testTag("ticker-slow"))
                    Text("Poll every 5 seconds instead of every second", modifier = Modifier.padding(start = 12.dp))
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Switch(checked = failing, onCheckedChange = { failing = it }, modifier = Modifier.testTag("ticker-failing"))
                    Text("Fail on purpose", modifier = Modifier.padding(start = 12.dp))
                }
            }
        }
        Text(
            "The counter goes up by one on every fetch. The core polls it a second after the last fetch ended while this screen is " +
                "shown and the app is active; with the app in the background, or the network gone, nothing is fetched.",
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(top = 12.dp),
        )
    }
}

private fun statusWord(status: QueryStatus): String =
    when (status) {
        QueryStatus.IDLE -> "Idle"
        QueryStatus.FETCHING -> "Fetching"
        QueryStatus.SUCCESS -> "Success"
        QueryStatus.ERROR -> "Error"
    }

/** The slower interval the switch asks for. */
private val SLOW_INTERVAL = 5.seconds
