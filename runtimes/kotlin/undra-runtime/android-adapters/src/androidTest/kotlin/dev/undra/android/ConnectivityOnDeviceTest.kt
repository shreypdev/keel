package dev.undra.android

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Looper
import androidx.test.core.app.ApplicationProvider
import androidx.test.platform.app.InstrumentationRegistry
import dev.undra.runtime.adapters.NetKind
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.UndraReader
import java.io.BufferedReader
import java.io.FileInputStream
import java.io.InputStreamReader
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

/** The Connectivity adapter against the device's real ConnectivityManager. */
class ConnectivityOnDeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val manager = context.getSystemService(ConnectivityManager::class.java)

    /** What the system says, computed without the adapter. */
    private fun systemSaysOnline(): Boolean {
        val network = manager.activeNetwork ?: return false
        return manager.getNetworkCapabilities(network)?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
    }

    private fun decode(event: RecordingCore.Event): Pair<Boolean, NetKind> {
        assertEquals(StandardPorts.Connectivity.PORT_ID, event.portId)
        assertEquals(StandardPorts.Connectivity.CHANGED, event.methodId)
        val reader = UndraReader(event.payload)
        val online = reader.readBool()
        val kind = NetKind.decode(reader)
        reader.finish()
        return online to kind
    }

    @Test
    fun current_reads_the_state_of_the_default_network() {
        val status = AndroidConnectivityAdapter(context).current()
        assertEquals(systemSaysOnline(), status.online)
        if (status.online) assertNotEquals(NetKind.NONE, status.kind) else assertEquals(NetKind.NONE, status.kind)
        println("Connectivity.current() = $status")
    }

    @Test
    fun attach_tells_the_core_the_current_state_at_once() {
        val core = RecordingCore()
        val adapter = AndroidConnectivityAdapter(context)
        try {
            adapter.attach(core)
            val events = core.awaitEvents { it.isNotEmpty() }
            assertTrue("an initial Connectivity.changed event", events.isNotEmpty())
            val (online, kind) = decode(events.first())
            assertEquals(systemSaysOnline(), online)
            assertEquals(adapter.current().kind, kind)
            println("first Connectivity.changed: online=$online kind=$kind")
        } finally {
            adapter.close()
        }
    }

    @Test
    fun the_listener_is_called_on_a_thread_of_its_own_and_only_for_changes() {
        val threads = CopyOnWriteArrayList<Thread>()
        val statuses = CopyOnWriteArrayList<NetworkStatus>()
        val first = CountDownLatch(1)
        val adapter = AndroidConnectivityAdapter(context)
        try {
            adapter.start { status ->
                threads.add(Thread.currentThread())
                statuses.add(status)
                first.countDown()
            }
            assertTrue(first.await(5, TimeUnit.SECONDS))
            Thread.sleep(500) // the system's follow-up callbacks (link properties, blocked status) repeat the state
            assertNotEquals(Looper.getMainLooper().thread, threads.first())
            assertEquals("undra-connectivity", threads.first().name)
            assertEquals("identical consecutive states are reported once: $statuses", statuses.toList().zipWithNext().count { (a, b) -> a == b }, 0)
        } finally {
            adapter.close()
        }
    }

    @Test
    fun close_stops_the_reports_and_start_can_be_called_again() {
        val core = RecordingCore()
        val adapter = AndroidConnectivityAdapter(context)
        adapter.attach(core)
        core.awaitEvents { it.isNotEmpty() }
        adapter.close()
        adapter.close() // idempotent
        val before = core.events.size
        adapter.attach(core) // a fresh start reports the state again
        val after = core.awaitEvents { it.size > before }
        assertTrue(after.size > before)
        adapter.close()
    }

    @Test
    fun a_validated_network_is_online_even_when_validation_is_required() {
        val strict = AndroidConnectivityAdapter(context, requireValidated = true).current()
        val caps = manager.activeNetwork?.let(manager::getNetworkCapabilities)
        val validated = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true
        assertEquals(systemSaysOnline() && validated, strict.online)
        assertFalse(!systemSaysOnline() && strict.online)
    }

    /**
     * Takes the emulator's network away and gives it back, and checks that the core hears both. This changes the state of
     * the whole device, so it runs only when asked: `-Pandroid.testInstrumentationRunnerArguments.undra.networkToggle=true`.
     */
    @Test
    fun turning_the_devices_network_off_and_on_is_reported_to_the_core() {
        val arguments = InstrumentationRegistry.getArguments()
        assumeTrue("set the instrumentation argument undra.networkToggle=true to run this", arguments.getString("undra.networkToggle") == "true")
        assumeTrue("the device has no network to take away", systemSaysOnline())
        val core = RecordingCore()
        val adapter = AndroidConnectivityAdapter(context)
        try {
            adapter.attach(core)
            val first = decode(core.awaitEvents { it.isNotEmpty() }.first())
            assertTrue("starts online", first.first)

            shell("svc wifi disable")
            shell("svc data disable")
            val offline = core.awaitEvents(30_000) { events -> events.any { !decode(it).first } }
            assertTrue("an offline event after the network went away: ${offline.map { decode(it) }}", offline.any { !decode(it).first })
            assertEquals(NetKind.NONE, decode(offline.first { !decode(it).first }).second)

            shell("svc wifi enable")
            shell("svc data enable")
            val sawOffline = offline.size
            val online = core.awaitEvents(60_000) { events -> events.drop(sawOffline).any { decode(it).first } }
            val last = decode(online.last())
            println("events: ${online.map { decode(it) }}")
            assertTrue("an online event after the network came back", online.drop(sawOffline).any { decode(it).first })
            assertTrue(last.first)
        } finally {
            shell("svc wifi enable")
            shell("svc data enable")
            adapter.close()
        }
    }

    private fun shell(command: String) {
        val descriptor = InstrumentationRegistry.getInstrumentation().uiAutomation.executeShellCommand(command)
        BufferedReader(InputStreamReader(FileInputStream(descriptor.fileDescriptor))).use { it.readText() } // wait for it to finish
        descriptor.close()
    }
}
