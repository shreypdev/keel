package dev.undra.fieldbook

import android.graphics.BitmapFactory
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.lifecycleScope
import dev.undra.fieldbook.core.Auth
import dev.undra.fieldbook.core.AuthError
import dev.undra.fieldbook.core.Note
import dev.undra.fieldbook.core.Notebook
import dev.undra.fieldbook.core.ServerConfig
import dev.undra.fieldbook.core.Session
import dev.undra.fieldbook.core.configureServer
import dev.undra.fieldbook.core.readPhoto
import java.text.DateFormat
import java.util.Date
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    // The stores' state lives in the Rust core. They are created only once there is a core.
    private val auth by lazy { Auth() }
    private val notebook by lazy {
        // `server/server.mjs`: the Android emulator reaches the machine it runs on at 10.0.2.2.
        configureServer(ServerConfig(baseUrl = "http://10.0.2.2:8787"))
        Notebook()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val app = application as UndraApp
        app.start(DevServer.requested(intent))
        // A new core replaced one the dev server lost: this activity's stores belong to the old one, so start over.
        lifecycleScope.launch {
            app.epoch.drop(1).collect {
                finish()
                startActivity(intent)
            }
        }
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    Column {
                        DevStatusBar(app)
                        val failure by app.failure.collectAsState()
                        failure?.let { DevServerProblem(it, onRetry = app::retry) } ?: FieldbookScreen(auth, notebook)
                    }
                }
            }
        }
    }

    override fun onDestroy() {
        if (isFinishing) {
            notebook.close()
            auth.close()
        }
        super.onDestroy()
    }
}

/** Reading `auth.session` is reading a StateFlow: the core pushes changes, Compose recomposes. */
@Composable
fun FieldbookScreen(auth: Auth, notebook: Notebook) {
    // What the device holds shows at once; the session is picked up in the background.
    LaunchedEffect(Unit) {
        runCatching { notebook.load() }
        runCatching { auth.resume() }
    }
    val session by auth.session.collectAsState()
    when (val s = session) {
        Session.SignedOut -> SignIn(auth)
        is Session.SignedIn -> Notes(auth, notebook, s.user)
    }
}

@Composable
fun SignIn(auth: Auth) {
    val scope = rememberCoroutineScope()
    val busy by auth.busy.collectAsState()
    var name by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var problem by remember { mutableStateOf<String?>(null) }
    Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Fieldbook", style = MaterialTheme.typography.headlineMedium)
        OutlinedTextField(name, { name = it }, label = { Text("Your name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        OutlinedTextField(
            code, { code = it }, label = { Text("Team code") }, singleLine = true,
            visualTransformation = PasswordVisualTransformation(), modifier = Modifier.fillMaxWidth(),
        )
        Button(
            enabled = name.isNotBlank() && code.isNotEmpty() && !busy,
            onClick = {
                scope.launch {
                    try {
                        auth.signIn(name, code)
                        problem = null
                    } catch (e: AuthError.BadCredentials) {
                        problem = "Wrong name or team code (the demo's is “fieldbook”)."
                    } catch (e: dev.undra.runtime.UndraException) {
                        problem = e.message
                    }
                }
            },
        ) { Text("Sign in") }
        problem?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    }
}

@Composable
fun Notes(auth: Auth, notebook: Notebook, user: String) {
    val scope = rememberCoroutineScope()
    val visible by notebook.visible.collectAsState()
    val tags by notebook.tags.collectAsState()
    val filter by notebook.filter.collectAsState()
    val pending by notebook.pending.collectAsState()
    var title by remember { mutableStateOf("") }
    var body by remember { mutableStateOf("") }
    var tag by remember { mutableStateOf("") }
    var problem by remember { mutableStateOf<String?>(null) }

    fun run(work: suspend () -> Unit) {
        scope.launch {
            try {
                work()
                problem = null
            } catch (e: dev.undra.runtime.UndraException) {
                problem = e.message
            }
        }
    }

    Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Fieldbook", style = MaterialTheme.typography.headlineMedium, modifier = Modifier.weight(1f))
            TextButton(onClick = { run { auth.signOut() } }) { Text(user) }
        }
        Text(
            if (pending == 0u) "Everything is sent" else "$pending change(s) waiting to send",
            style = MaterialTheme.typography.bodySmall,
        )
        problem?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        OutlinedTextField(title, { title = it }, label = { Text("Title") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(tag, { tag = it }, label = { Text("Tag") }, singleLine = true, modifier = Modifier.weight(1f))
            Button(
                enabled = title.isNotBlank(),
                onClick = { run { notebook.add(title, body, tag); title = ""; body = "" } },
            ) { Text("Add note") }
        }
        OutlinedTextField(
            filter.query, { notebook.setQuery(it) }, label = { Text("Search notes") }, singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = filter.tag.isEmpty(), onClick = { notebook.setTag("") }, label = { Text("all") })
            tags.forEach { t -> FilterChip(selected = filter.tag == t, onClick = { notebook.setTag(t) }, label = { Text(t) }) }
        }
        LazyColumn(modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(visible, key = { it.id.toLong() }) { note -> NoteRow(note, notebook, ::run) }
        }
    }
}

@Composable
fun NoteRow(note: Note, notebook: Notebook, run: (suspend () -> Unit) -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri != null) {
            val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
            if (bytes != null) run { notebook.attachPhoto(note.id, bytes) }
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(note.title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            if (note.tag.isNotEmpty()) Text(note.tag, style = MaterialTheme.typography.labelSmall)
            TextButton(onClick = { run { notebook.togglePin(note.id) } }) { Text(if (note.pinned) "Unpin" else "Pin") }
            TextButton(onClick = { run { notebook.remove(note.id) } }) { Text("Delete") }
        }
        if (note.body.isNotEmpty()) Text(note.body)
        Text(DateFormat.getDateTimeInstance().format(Date(note.created.epochMillis)), style = MaterialTheme.typography.bodySmall)
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
            note.photos.forEach { Thumb(it) }
            TextButton(onClick = { picker.launch("image/*") }) { Text("+ photo") }
        }
    }
}

/** A photo of a note: the core reads the bytes from its `Fs` port, the screen draws them. */
@Composable
fun Thumb(path: String) {
    var bitmap by remember(path) { mutableStateOf<androidx.compose.ui.graphics.ImageBitmap?>(null) }
    LaunchedEffect(path) {
        val bytes = runCatching { readPhoto(path) }.getOrNull() ?: return@LaunchedEffect
        bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
    }
    bitmap?.let { Image(it, contentDescription = "A photo attached to the note", modifier = Modifier.size(56.dp)) }
}
