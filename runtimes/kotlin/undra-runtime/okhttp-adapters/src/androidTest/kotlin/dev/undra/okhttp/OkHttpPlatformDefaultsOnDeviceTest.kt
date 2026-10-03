package dev.undra.okhttp

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import dev.undra.android.AndroidPlatformDefaults
import dev.undra.android.RecordingCore
import dev.undra.android.TestHttpServer
import dev.undra.android.call
import dev.undra.runtime.adapters.ClientWebSocketAdapter
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.SseEvent
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.runBlocking
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * `AndroidPlatformDefaults.installWithOkHttp` on a device: the ports of `install` are there, and the app's client is in the path of
 * the three network ports (ADR-060).
 */
class OkHttpPlatformDefaultsOnDeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private lateinit var server: TestHttpServer
    private val core = RecordingCore()
    private var platform: OkHttpPlatform? = null
    private val seen = CopyOnWriteArrayList<String>()

    /** A client as an app has one: an interceptor that traces every request and signs it. */
    private val client: OkHttpClient = OkHttpClient.Builder()
        .addInterceptor(
            Interceptor { chain ->
                seen += chain.request().url.encodedPath
                chain.proceed(chain.request().newBuilder().header("X-Traced", "yes").build())
            },
        )
        .build()

    @Before
    fun setUp() {
        server = TestHttpServer()
        File(context.filesDir, "undra").deleteRecursively()
        File(context.noBackupFilesDir, "undra").deleteRecursively()
    }

    @After
    fun tearDown() {
        platform?.close()
        server.close()
        File(context.filesDir, "undra").deleteRecursively()
        File(context.noBackupFilesDir, "undra").deleteRecursively()
    }

    @Test
    fun the_eleven_method_ports_are_registered_and_http_goes_through_the_apps_client() {
        platform = AndroidPlatformDefaults.installWithOkHttp(core, context, client)
        assertEquals(
            setOf(
                StandardPorts.Kv.PORT_ID, StandardPorts.SecureStore.PORT_ID, StandardPorts.Fs.PORT_ID, StandardPorts.Http.PORT_ID,
                StandardPorts.Clock.PORT_ID, StandardPorts.Rng.PORT_ID, StandardPorts.Log.PORT_ID, StandardPorts.Timer.PORT_ID,
                StandardPorts.WebSocket.PORT_ID, StandardPorts.Sse.PORT_ID, StandardPorts.Db.PORT_ID,
            ),
            core.ports.keys,
        )
        for (id in listOf(StandardPorts.Http.PORT_ID, StandardPorts.WebSocket.PORT_ID, StandardPorts.Sse.PORT_ID)) {
            assertFalse("port $id is async", core.ports.getValue(id).sync)
        }
        for (id in listOf(StandardPorts.WebSocket.PORT_ID, StandardPorts.Sse.PORT_ID)) {
            assertNotNull("port $id releases what it holds when the core closes", core.ports.getValue(id).detach)
        }

        server.fixed("/ping", 200, "pong", listOf("X-From" to "server"))
        val request = HttpRequest(HttpMethod.GET, server.base + "/ping", listOf(Header("Accept", "text/plain")), null, 10_000u)
        val response = HttpResponse.decodeAll(call(core.ports.getValue(StandardPorts.Http.PORT_ID), StandardPorts.Http.REQUEST, HttpRequest.encodeToByteArray(request)))
        assertEquals(200.toUShort(), response.status)
        assertEquals("pong", String(response.body))
        assertEquals(listOf("/ping"), seen.toList())
        assertEquals("yes", server.requests.single().header("X-Traced"))
    }

    @Test
    fun an_event_stream_opens_through_the_apps_client_too() {
        val installed = AndroidPlatformDefaults.installWithOkHttp(core, context, client).also { platform = it }
        server.route("/events") { ex ->
            ex.startChunked(200, listOf("Content-Type" to "text/event-stream"))
            ex.chunk("id: 1\ndata: hello\n\n".toByteArray())
            ex.endChunked()
        }
        val events = runBlocking {
            val stream = installed.sse.open(server.base + "/events", emptyList(), null)
            installed.sse.next(stream, 16u)
        }
        assertEquals(listOf(SseEvent("1", "message", "hello", null)), events)
        assertEquals(listOf("/events"), seen.toList())
        assertEquals("yes", server.requests.single().header("X-Traced"))
    }

    @Test
    fun a_platform_adapter_can_be_kept_for_one_port() {
        val installed = AndroidPlatformDefaults.installWithOkHttp(core, context, client, webSocket = ClientWebSocketAdapter()).also { platform = it }
        assertTrue(core.ports.containsKey(StandardPorts.WebSocket.PORT_ID))
        assertEquals(0, installed.webSocket.openConnections)
    }
}
