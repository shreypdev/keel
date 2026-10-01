package dev.undra.playground

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.lifecycleScope
import dev.undra.playground.ui.DevServerProblem
import dev.undra.playground.ui.DevStatusBar
import dev.undra.playground.ui.PlaygroundApp
import dev.undra.playground.ui.PlaygroundTheme
import dev.undra.playground.ui.Tab
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.launch

/**
 * The one activity. It shows the four screens of the playground over the one core `UndraApp` loaded.
 *
 * `adb shell am start -n dev.undra.playground/.MainActivity --es tab remote` opens it on the Remote tab
 * (`todos`, `counter`, `biglist` or `remote`), which is how the screenshots in `.proof/` were taken.
 * `--es undra_dev_url ws://10.0.2.2:7443` runs it against `undra dev` (debug builds; see [DevServer]).
 */
class MainActivity : ComponentActivity() {
    private var tab by mutableStateOf(Tab.TODOS)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val app = application as UndraApp
        app.start(DevServer.requested(intent))
        tab = Tab.fromId(savedInstanceState?.getString(STATE_TAB) ?: intent.getStringExtra(EXTRA_TAB))
        // A new core replaced one the dev server lost: this activity's stores belong to the old one, so start
        // over on the new one (finish() clears the view models; the intent brings the tab back).
        lifecycleScope.launch {
            app.epoch.drop(1).collect {
                finish()
                startActivity(intent.putExtra(EXTRA_TAB, tab.id))
            }
        }
        setContent {
            PlaygroundTheme {
                Surface {
                    Column(Modifier.fillMaxSize()) {
                        DevStatusBar(app)
                        val failure by app.failure.collectAsState()
                        failure?.let { DevServerProblem(it, onRetry = app::retry) }
                            ?: PlaygroundApp(tab = tab, onTab = { tab = it })
                    }
                }
            }
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
