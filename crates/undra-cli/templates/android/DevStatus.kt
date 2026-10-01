package @@APP_ID@@

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.undra.runtime.ClosedReason
import dev.undra.runtime.ConnectionState

/** What a user of `undra dev` reads about the connection, in one line. */
fun describe(url: String?, state: ConnectionState): String = when (state) {
    ConnectionState.Connecting -> "Connecting to $url"
    ConnectionState.Connected -> "Dev server: $url"
    is ConnectionState.Reconnecting -> "Reconnecting to $url (attempt ${state.attempt})"
    is ConnectionState.Closed -> when (state.reason) {
        ClosedReason.SESSION_LOST -> "The core was rebuilt: loading the new one"
        ClosedReason.SCHEMA_MISMATCH -> "The schema changed, state reset: run undra bindgen and rebuild the app"
        ClosedReason.REQUESTED -> "Disconnected"
        ClosedReason.FAILED -> "Connection failed: ${state.cause?.message}"
    }
}

/**
 * The connection to `undra dev`, as a thin bar above the screens: green while connected, amber while the runtime
 * reconnects, red when the connection is over, and for a few seconds what the dev server says about a reload. Nothing
 * at all for the in-process core.
 */
@Composable
fun DevStatusBar(app: UndraApp) {
    val url = app.devUrl ?: return
    val state by app.connection.collectAsState()
    val notice by app.devNotice.collectAsState()
    val color = when (state) {
        ConnectionState.Connected -> Color(0xFF2E7D32)
        is ConnectionState.Closed -> Color(0xFFC62828)
        else -> Color(0xFFEF6C00)
    }
    Row(
        modifier = Modifier.fillMaxWidth().background(color).statusBarsPadding().padding(horizontal = 12.dp, vertical = 4.dp).testTag("dev-status"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // What the dev server says about a reload ("Reloaded, state kept") replaces the line for a few seconds.
        Text(notice ?: describe(url, state), color = Color.White, fontSize = 12.sp)
    }
}

/** Shown instead of the screens while there is no core: the dev server cannot be reached, or it is another build. */
@Composable
fun DevServerProblem(message: String, onRetry: () -> Unit) {
    Column(
        modifier = Modifier.fillMaxSize().padding(24.dp).testTag("dev-problem"),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
    ) {
        Text("No core to talk to", style = MaterialTheme.typography.headlineSmall)
        Text(message)
        Text(
            "Is `undra dev` running? An emulator reaches it at ws://10.0.2.2:<port>; a USB device needs `adb reverse tcp:<port> tcp:<port>` " +
                "(`undra dev --android` does it) and ws://127.0.0.1:<port>.",
            style = MaterialTheme.typography.bodySmall,
        )
        Button(onClick = onRetry, modifier = Modifier.testTag("dev-retry")) { Text("Retry") }
    }
}
