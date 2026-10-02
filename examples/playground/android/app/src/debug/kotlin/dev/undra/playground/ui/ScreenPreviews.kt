package dev.undra.playground.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.tooling.preview.Preview
import dev.undra.playground.core.TodoSelection
import dev.undra.playground.core.Todos
import dev.undra.playground.core.UndraIds
import dev.undra.testkit.RecordedCore
import dev.undra.testkit.ReplayOptions

// Compose previews over the testing kit (docs/TESTING.md). Android Studio's preview pane runs on a desktop JVM that cannot load the app's native
// core, so a preview plays a *recording* of a session under the generated store (a RecordedCore: no Rust runs, so it renders the same on every
// machine). The recording is `testkit/fixtures/session-todos.json`, bundled as a debug asset. To drive the real core with scripted ports, use
// PreviewCore in an instrumented or JVM test (docs/TESTING.md).

@Composable
private fun recordedTodos(startAtMs: Long?, playToEnd: Boolean): TodosViewModel {
    val context = LocalContext.current
    return remember(startAtMs, playToEnd) {
        val json = context.assets.open("session-todos.json").bufferedReader().use { it.readText() }
        val recorded = RecordedCore.load(json, UndraIds.SCHEMA_HASH, ReplayOptions(startAtMs = startAtMs), makeShared = false)
        if (playToEnd) recorded.playAll()
        TodosViewModel(Todos(recorded.core), TodoSelection(recorded.core))
    }
}

/** The recording played to its end: three items, one of them done. */
@Preview(name = "Todos: a recorded session, played to the end", showBackground = true, widthDp = 360, heightDp = 640)
@Composable
fun TodosRecordedToTheEndPreview() {
    PlaygroundTheme { TodosScreen(recordedTodos(startAtMs = null, playToEnd = true)) }
}

/** The same recording from 400 ms in: three items, none done yet. */
@Preview(name = "Todos: the same recording at 400 ms", showBackground = true, widthDp = 360, heightDp = 640)
@Composable
fun TodosRecordedMidwayPreview() {
    PlaygroundTheme { TodosScreen(recordedTodos(startAtMs = 400, playToEnd = false)) }
}
