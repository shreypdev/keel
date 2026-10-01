package dev.undra.playground

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.undra.playground.ui.PlaygroundApp
import dev.undra.playground.ui.PlaygroundTheme
import dev.undra.playground.ui.Tab

/**
 * The one activity. It shows the four screens of the playground over the one core `UndraApp` loaded.
 *
 * `adb shell am start -n dev.undra.playground/.MainActivity --es tab remote` opens it on the Remote tab
 * (`todos`, `counter`, `biglist` or `remote`), which is how the screenshots in `.proof/` were taken.
 */
class MainActivity : ComponentActivity() {
    private var tab by mutableStateOf(Tab.TODOS)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        tab = Tab.fromId(savedInstanceState?.getString(STATE_TAB) ?: intent.getStringExtra(EXTRA_TAB))
        setContent {
            PlaygroundTheme { PlaygroundApp(tab = tab, onTab = { tab = it }) }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.getStringExtra(EXTRA_TAB)?.let { tab = Tab.fromId(it) }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putString(STATE_TAB, tab.id)
    }

    private companion object {
        const val EXTRA_TAB = "tab"
        const val STATE_TAB = "tab"
    }
}
