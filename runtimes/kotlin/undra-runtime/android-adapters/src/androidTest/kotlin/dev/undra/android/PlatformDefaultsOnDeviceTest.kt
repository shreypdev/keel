package dev.undra.android

import android.content.Context
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.io.File
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * `AndroidPlatformDefaults.install` on a device: all ten ports and the three opt-in ones are there and each of the ten works
 * through its port methods (the opt-in ones have suites of their own: `DbOnDeviceTest`, and the runtime's realtime suite).
 */
class PlatformDefaultsOnDeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private lateinit var server: TestHttpServer
    private val core = RecordingCore()
    private var platform: AndroidPlatform? = null

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

    private fun install(): AndroidPlatform = AndroidPlatformDefaults.install(core, context).also { platform = it }

    @Test
    fun install_registers_the_eleven_method_ports_and_starts_the_two_event_sources() {
        install()
        assertEquals(
            setOf(
                StandardPorts.Kv.PORT_ID, StandardPorts.SecureStore.PORT_ID, StandardPorts.Fs.PORT_ID, StandardPorts.Http.PORT_ID,
                StandardPorts.Clock.PORT_ID, StandardPorts.Rng.PORT_ID, StandardPorts.Log.PORT_ID, StandardPorts.Timer.PORT_ID,
                StandardPorts.WebSocket.PORT_ID, StandardPorts.Sse.PORT_ID, StandardPorts.Db.PORT_ID,
            ),
            core.ports.keys,
        )
        for (id in listOf(StandardPorts.WebSocket.PORT_ID, StandardPorts.Sse.PORT_ID, StandardPorts.Db.PORT_ID)) {
            assertFalse(core.ports.getValue(id).sync)
            assertNotNull("port $id releases what it holds when the core closes", core.ports.getValue(id).detach)
        }
        assertTrue(core.ports.getValue(StandardPorts.Clock.PORT_ID).sync)
        assertTrue(core.ports.getValue(StandardPorts.Rng.PORT_ID).sync)
        assertTrue(core.ports.getValue(StandardPorts.Log.PORT_ID).sync)
        assertTrue(core.ports.getValue(StandardPorts.Timer.PORT_ID).sync)
        for (id in listOf(StandardPorts.Kv.PORT_ID, StandardPorts.SecureStore.PORT_ID, StandardPorts.Fs.PORT_ID, StandardPorts.Http.PORT_ID)) {
            assertFalse("port $id is async", core.ports.getValue(id).sync)
        }
        val events = core.awaitEvents { list ->
            list.any { it.portId == StandardPorts.Connectivity.PORT_ID } && list.any { it.portId == StandardPorts.Lifecycle.PORT_ID }
        }
        assertTrue("Connectivity.changed was sent", events.any { it.portId == StandardPorts.Connectivity.PORT_ID && it.methodId == StandardPorts.Connectivity.CHANGED })
        assertTrue("Lifecycle.changed was sent", events.any { it.portId == StandardPorts.Lifecycle.PORT_ID && it.methodId == StandardPorts.Lifecycle.CHANGED })
    }

    @Test
    fun every_registered_port_answers_through_its_port_methods() {
        install()
        val ports = core.ports

        // Clock
        val now = Codecs.i64.decodeAll(call(ports.getValue(StandardPorts.Clock.PORT_ID), StandardPorts.Clock.NOW_MS, ByteArray(0)))
        assertTrue("Clock.now_ms is the wall clock", kotlin.math.abs(now - System.currentTimeMillis()) < 5_000)
        val m1 = Codecs.u64.decodeAll(call(ports.getValue(StandardPorts.Clock.PORT_ID), StandardPorts.Clock.MONOTONIC_NS, ByteArray(0)))
        val m2 = Codecs.u64.decodeAll(call(ports.getValue(StandardPorts.Clock.PORT_ID), StandardPorts.Clock.MONOTONIC_NS, ByteArray(0)))
        assertTrue(m2 >= m1)

        // Rng
        val ask = UndraWriter(4).also { it.writeU32(32u) }.toByteArray()
        val random = Codecs.bytes.decodeAll(call(ports.getValue(StandardPorts.Rng.PORT_ID), StandardPorts.Rng.FILL, ask))
        assertEquals(32, random.size)
        assertFalse(random.all { it == 0.toByte() })

        // Log: the record goes to logcat; the method answers with nothing
        val log = UndraWriter(32).also {
            it.writeU8(2u)
            it.writeStr("undra_adapters_test")
            it.writeStr("hello from the Log port")
        }.toByteArray()
        assertEquals(0, call(ports.getValue(StandardPorts.Log.PORT_ID), StandardPorts.Log.LOG, log).size)

        // Timer: the core is told when the timer comes due
        val timer = UndraWriter(16).also {
            it.writeU32(41u)
            it.writeU64(80u)
        }.toByteArray()
        call(ports.getValue(StandardPorts.Timer.PORT_ID), StandardPorts.Timer.SET, timer)
        val deadline = System.nanoTime() + 5_000_000_000L
        while (41u !in core.timers && System.nanoTime() < deadline) Thread.sleep(10)
        assertEquals(listOf(41u), core.timers)

        // Kv, SecureStore, Fs
        val kv = ports.getValue(StandardPorts.Kv.PORT_ID)
        call(kv, StandardPorts.Kv.SET, argsOf("undra.query.queue", byteArrayOf(1, 2, 3)))
        assertArrayEquals(byteArrayOf(1, 2, 3), Codecs.option(Codecs.bytes).decodeAll(call(kv, StandardPorts.Kv.GET, argsOf("undra.query.queue"))))
        val secure = ports.getValue(StandardPorts.SecureStore.PORT_ID)
        call(secure, StandardPorts.SecureStore.SET, argsOf("token", byteArrayOf(9, 8, 7)))
        assertArrayEquals(byteArrayOf(9, 8, 7), Codecs.option(Codecs.bytes).decodeAll(call(secure, StandardPorts.SecureStore.GET, argsOf("token"))))
        val fs = ports.getValue(StandardPorts.Fs.PORT_ID)
        call(fs, StandardPorts.Fs.WRITE, argsOf("a/b.txt", "fs".toByteArray()))
        assertEquals("fs", String(Codecs.bytes.decodeAll(call(fs, StandardPorts.Fs.READ, argsOf("a/b.txt")))))

        // Http
        server.fixed("/ping", 200, "pong", listOf("X-From" to "server"))
        val request = HttpRequest(HttpMethod.GET, server.base + "/ping", listOf(Header("Accept", "text/plain")), null, 10_000u)
        val response = HttpResponse.decodeAll(call(ports.getValue(StandardPorts.Http.PORT_ID), StandardPorts.Http.REQUEST, HttpRequest.encodeToByteArray(request)))
        assertEquals(200.toUShort(), response.status)
        assertEquals("pong", String(response.body))
    }

    @Test
    fun the_adapters_are_available_from_the_returned_handle_and_use_the_documented_locations() {
        val platform = install()
        kotlinx.coroutines.runBlocking {
            platform.kv.set("k", byteArrayOf(1))
            platform.fs.write("f", byteArrayOf(2))
            platform.secureStore.set("s", byteArrayOf(3))
        }
        assertTrue(File(context.filesDir, "undra/$TEST_NAMESPACE/kv").isDirectory)
        assertTrue(File(context.filesDir, "undra/$TEST_NAMESPACE/fs/f").isFile)
        assertTrue(File(context.noBackupFilesDir, "undra/$TEST_NAMESPACE/secure").isDirectory)
        assertNotNull(platform.connectivity.current())
        assertNotNull(platform.lifecycle.state)
    }

    @Test
    fun installing_again_is_allowed_and_the_new_sources_report_their_state() {
        val first = install()
        val before = core.awaitEvents { list -> list.any { it.portId == StandardPorts.Connectivity.PORT_ID } }
            .count { it.portId == StandardPorts.Connectivity.PORT_ID }
        val second = install()
        assertTrue(first !== second)
        val after = core.awaitEvents { list -> list.count { it.portId == StandardPorts.Connectivity.PORT_ID } > before }
        assertTrue("the second install reported the state again", after.count { it.portId == StandardPorts.Connectivity.PORT_ID } > before)
        assertEquals("still one registration per port", 11, core.ports.size)
    }

    @Test
    fun close_stops_the_event_sources_but_leaves_the_ports_registered() {
        val platform = install()
        core.awaitEvents { list -> list.any { it.portId == StandardPorts.Lifecycle.PORT_ID } }
        platform.close()
        val lifecycleEvents = core.events.count { it.portId == StandardPorts.Lifecycle.PORT_ID }
        ActivityScenario.launch(TestActivity::class.java).use { Thread.sleep(1_200) }
        assertEquals("no Lifecycle events after close", lifecycleEvents, core.events.count { it.portId == StandardPorts.Lifecycle.PORT_ID })
        assertEquals(11, core.ports.size)
    }
}
