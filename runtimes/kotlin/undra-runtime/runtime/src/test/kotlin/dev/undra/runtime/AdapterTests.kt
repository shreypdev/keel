package dev.undra.runtime

import dev.undra.runtime.adapters.AppState
import dev.undra.runtime.adapters.ClockAdapter
import dev.undra.runtime.adapters.ConnectivityEvents
import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.JulLog
import dev.undra.runtime.adapters.JvmAdapters
import dev.undra.runtime.adapters.LifecycleEvents
import dev.undra.runtime.adapters.LogAdapter
import dev.undra.runtime.adapters.NetKind
import dev.undra.runtime.adapters.RngAdapter
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.TimerAdapter
import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.TempDir
import dev.undra.runtime.support.attach
import dev.undra.runtime.support.eventually
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertBytes
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.hex
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Fnv
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.logging.Level
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private fun call(impl: PortImpl, method: UInt, args: ByteArray = NO_BYTES): ByteArray = runBlocking { impl.methods.getValue(method)(args) }

private fun args(build: UndraWriter.() -> Unit): ByteArray = UndraWriter().also(build).toByteArray()

class AdapterTests : Suite() {
    init {
        case("every standard port and method id equals fnv1a32 of its documented name") {
            fun id(name: String) = Fnv.fnv1a32(name)
            assertEq(id("port.Clock"), StandardPorts.Clock.PORT_ID)
            assertEq(id("Clock.now_ms"), StandardPorts.Clock.NOW_MS)
            assertEq(id("Clock.monotonic_ns"), StandardPorts.Clock.MONOTONIC_NS)
            assertEq(id("port.Rng"), StandardPorts.Rng.PORT_ID)
            assertEq(id("Rng.fill"), StandardPorts.Rng.FILL)
            assertEq(id("port.Log"), StandardPorts.Log.PORT_ID)
            assertEq(id("Log.log"), StandardPorts.Log.LOG)
            assertEq(id("port.Http"), StandardPorts.Http.PORT_ID)
            assertEq(id("Http.request"), StandardPorts.Http.REQUEST)
            assertEq(id("port.Kv"), StandardPorts.Kv.PORT_ID)
            assertEq(id("Kv.get"), StandardPorts.Kv.GET)
            assertEq(id("Kv.set"), StandardPorts.Kv.SET)
            assertEq(id("Kv.delete"), StandardPorts.Kv.DELETE)
            assertEq(id("Kv.list"), StandardPorts.Kv.LIST)
            assertEq(id("port.SecureStore"), StandardPorts.SecureStore.PORT_ID)
            assertEq(id("SecureStore.get"), StandardPorts.SecureStore.GET)
            assertEq(id("SecureStore.set"), StandardPorts.SecureStore.SET)
            assertEq(id("SecureStore.delete"), StandardPorts.SecureStore.DELETE)
            assertEq(id("SecureStore.list"), StandardPorts.SecureStore.LIST)
            assertEq(id("port.Fs"), StandardPorts.Fs.PORT_ID)
            assertEq(id("Fs.read"), StandardPorts.Fs.READ)
            assertEq(id("Fs.write"), StandardPorts.Fs.WRITE)
            assertEq(id("Fs.delete"), StandardPorts.Fs.DELETE)
            assertEq(id("Fs.list"), StandardPorts.Fs.LIST)
            assertEq(id("port.Timer"), StandardPorts.Timer.PORT_ID)
            assertEq(id("Timer.set"), StandardPorts.Timer.SET)
            assertEq(id("port.Connectivity"), StandardPorts.Connectivity.PORT_ID)
            assertEq(id("Connectivity.changed"), StandardPorts.Connectivity.CHANGED)
            assertEq(id("port.Lifecycle"), StandardPorts.Lifecycle.PORT_ID)
            assertEq(id("Lifecycle.changed"), StandardPorts.Lifecycle.CHANGED)
        }

        case("record codecs produce the documented bytes") {
            val request = HttpRequest(HttpMethod.POST, "/", listOf(Header("a", "b")), byteArrayOf(9), 5u)
            assertBytes("0100" + "01000000" + "2f" + "01000000" + "0100000061" + "0100000062" + "01" + "01000000" + "09" + "01" + "05000000", HttpRequest.encodeToByteArray(request))
            assertBytes("0000" + "01000000" + "2f" + "00000000" + "00" + "00", HttpRequest.encodeToByteArray(HttpRequest(HttpMethod.GET, "/")))
            assertBytes("c800" + "00000000" + "01000000" + "09", HttpResponse.encodeToByteArray(HttpResponse(200u, emptyList(), byteArrayOf(9))))
            assertBytes("0000" + "01000000" + "78", HttpError.encodeToByteArray(HttpError.Network("x")))
            assertBytes("0100", HttpError.encodeToByteArray(HttpError.Timeout))
            assertBytes("0200", HttpError.encodeToByteArray(HttpError.Cancelled))
            assertBytes("0300" + "01000000" + "79", HttpError.encodeToByteArray(HttpError.InvalidUrl("y")))
            assertBytes("0000", FsError.encodeToByteArray(FsError.NotFound))
            assertBytes("0100", FsError.encodeToByteArray(FsError.Denied))
            assertBytes("0200" + "01000000" + "65", FsError.encodeToByteArray(FsError.Io("e")))
            assertBytes("0400", NetKind.encodeToByteArray(NetKind.NONE))
            assertBytes("0200", AppState.encodeToByteArray(AppState.BACKGROUND))
            assertBytes("0600", HttpMethod.encodeToByteArray(HttpMethod.OPTIONS))
        }

        case("record codecs round-trip every variant") {
            for (m in HttpMethod.entries) assertEq(m, HttpMethod.decodeAll(HttpMethod.encodeToByteArray(m)))
            for (k in NetKind.entries) assertEq(k, NetKind.decodeAll(NetKind.encodeToByteArray(k)))
            for (a in AppState.entries) assertEq(a, AppState.decodeAll(AppState.encodeToByteArray(a)))
            val errors = listOf(HttpError.Network("héllo 🌊"), HttpError.Timeout, HttpError.Cancelled, HttpError.InvalidUrl(""))
            for (e in errors) assertEq(e, HttpError.decodeAll(HttpError.encodeToByteArray(e)))
            val fs = listOf(FsError.NotFound, FsError.Denied, FsError.Io("disk"))
            for (e in fs) assertEq(e, FsError.decodeAll(FsError.encodeToByteArray(e)))
            val request = HttpRequest(HttpMethod.PUT, "https://example.com/é?q=1", listOf(Header("A", "1"), Header("A", "2")), ByteArray(70_000) { it.toByte() }, UInt.MAX_VALUE)
            assertEq(request, HttpRequest.decodeAll(HttpRequest.encodeToByteArray(request)))
            assertEq(request.hashCode(), HttpRequest.decodeAll(HttpRequest.encodeToByteArray(request)).hashCode())
            val response = HttpResponse(UShort.MAX_VALUE, listOf(Header("x", "")), NO_BYTES)
            assertEq(response, HttpResponse.decodeAll(HttpResponse.encodeToByteArray(response)))
            assertTrue(request != request.copy(body = null))
        }

        case("unknown variants and truncated records are WireExceptions, never anything else") {
            assertThrows<WireException.InvalidTag> { HttpMethod.decodeAll(byteArrayOf(7, 0)) }
            assertThrows<WireException.InvalidTag> { NetKind.decodeAll(byteArrayOf(5, 0)) }
            assertThrows<WireException.InvalidTag> { AppState.decodeAll(byteArrayOf(3, 0)) }
            assertThrows<WireException.InvalidTag> { HttpError.decodeAll(byteArrayOf(4, 0)) }
            assertThrows<WireException.InvalidTag> { FsError.decodeAll(byteArrayOf(3, 0)) }
            val bytes = HttpRequest.encodeToByteArray(HttpRequest(HttpMethod.GET, "/x", listOf(Header("a", "b")), byteArrayOf(1), 1u))
            for (cut in 0 until bytes.size) assertThrows<WireException>("cut at $cut") { HttpRequest.decodeAll(bytes.copyOf(cut)) }
        }

        case("port errors are exceptions with readable messages") {
            assertTrue(HttpError.Network("refused").message!!.contains("refused"))
            assertTrue(HttpError.InvalidUrl("no host").message!!.contains("no host"))
            assertTrue(FsError.Io("disk full").message!!.contains("disk full"))
        }

        case("Clock: wall time and a monotonic counter from zero, as a sync port") {
            var wall = 1_700_000_000_000L
            var nanos = -5_000L // System.nanoTime() may be negative
            val clock = ClockAdapter({ wall }, { nanos })
            assertEq(1_700_000_000_000L, clock.nowMs())
            assertEq(0uL, clock.monotonicNs())
            nanos += 1_234
            assertEq(1_234uL, clock.monotonicNs())
            val impl = clock.portImpl()
            assertTrue(impl.sync)
            wall += 5
            assertEq(1_700_000_000_005L, Codecs.i64.decodeAll(call(impl, StandardPorts.Clock.NOW_MS)))
            assertEq(1_234uL, Codecs.u64.decodeAll(call(impl, StandardPorts.Clock.MONOTONIC_NS)))
            val real = ClockAdapter()
            val a = real.monotonicNs()
            Thread.sleep(5)
            assertTrue(real.monotonicNs() > a, "monotonic time advances")
            assertTrue(kotlin.math.abs(real.nowMs() - System.currentTimeMillis()) < 1000)
        }

        case("Rng: exact length, fresh bytes each time, capped, and reachable as a sync port") {
            val rng = RngAdapter()
            assertEq(0, rng.fill(0u).size)
            assertEq(1, rng.fill(1u).size)
            assertEq(4096, rng.fill(4096u).size)
            assertTrue(!rng.fill(32u).contentEquals(rng.fill(32u)), "two 256-bit draws must differ")
            assertTrue(rng.fill(64u).any { it != 0.toByte() })
            assertEq(RngAdapter.MAX_FILL.toInt(), rng.fill(RngAdapter.MAX_FILL).size)
            assertThrows<IllegalArgumentException> { rng.fill(RngAdapter.MAX_FILL + 1u) }
            val impl = rng.portImpl()
            assertTrue(impl.sync)
            val reply = call(impl, StandardPorts.Rng.FILL, args { writeU32(16u) })
            assertEq(16, Codecs.bytes.decodeAll(reply).size)
            assertThrows<WireException> { call(impl, StandardPorts.Rng.FILL, byteArrayOf(1)) }
            assertThrows<WireException> { call(impl, StandardPorts.Rng.FILL, args { writeU32(1u); writeU8(0u) }) }
        }

        case("Log: levels map to java.util.logging and the target names the logger") {
            LogCapture("undra.adapter.test").use { log ->
                val impl = LogAdapter().portImpl()
                assertTrue(impl.sync)
                for (level in 0..6) {
                    call(impl, StandardPorts.Log.LOG, args { writeU8(level.toUByte()); writeStr("undra.adapter.test"); writeStr("m$level") })
                }
                val expected = listOf(Level.FINEST, Level.FINE, Level.INFO, Level.WARNING, Level.SEVERE, Level.SEVERE, Level.INFO)
                assertEq(expected, log.records.map { it.level })
                assertEq((0..6).map { "m$it" }, log.messages())
                assertEq("undra.adapter.test", log.records.first().loggerName)
            }
            LogCapture("undra").use { log ->
                JulLog.log(2u, "", "no target")
                assertEq(listOf("no target"), log.messages())
            }
        }

        case("Timer: fires each timer once after its delay, in due order; close cancels the rest") {
            val fired = CopyOnWriteArrayList<UInt>()
            val timers = TimerAdapter { fired.add(it) }
            val impl = timers.portImpl()
            assertTrue(impl.sync)
            call(impl, StandardPorts.Timer.SET, args { writeU32(2u); writeU64(150uL) })
            call(impl, StandardPorts.Timer.SET, args { writeU32(1u); writeU64(30uL) })
            call(impl, StandardPorts.Timer.SET, args { writeU32(3u); writeU64(0uL) })
            call(impl, StandardPorts.Timer.SET, args { writeU32(9u); writeU64(ULong.MAX_VALUE) }) // never in this test; must not overflow
            eventually("timers 3, 1 and 2 fire") { fired.size == 3 }
            assertEq(listOf(3u, 1u, 2u), fired.toList())
            timers.close()
            Thread.sleep(50)
            assertEq(3, fired.size, "the far-future timer was cancelled by close")
        }

        case("Timer: the thread exits when idle, but never while a timer is still pending") {
            val fired = CopyOnWriteArrayList<UInt>()
            val timers = TimerAdapter({ fired.add(it) }, 60L)
            timers.set(1u, 400uL) // pending for much longer than the idle timeout
            Thread.sleep(200)
            assertTrue(timers.hasLiveThread, "the worker must stay while a timer is pending")
            eventually("the pending timer still fires") { fired.contains(1u) }
            eventually("the idle worker exits") { !timers.hasLiveThread }
            timers.set(2u, 10uL) // and a new one starts on demand
            eventually("a timer set after the thread exited fires") { fired.contains(2u) }
            timers.close()
        }

        case("Timer: a failing callback is logged and does not stop later timers") {
            val fired = CopyOnWriteArrayList<UInt>()
            val timers = TimerAdapter { id -> if (id == 1u) throw IllegalStateException("core closed") else fired.add(id) }
            LogCapture("dev.undra.runtime").use { log ->
                timers.set(1u, 0uL)
                timers.set(2u, 20uL)
                eventually("the second timer fires") { fired.isNotEmpty() }
                assertTrue(log.records.any { it.thrown is IllegalStateException })
            }
            timers.close()
        }

        case("Timer: sets that arrive after close are refused by the executor, not silently queued forever") {
            val timers = TimerAdapter { }
            timers.set(1u, 0uL)
            timers.close()
            // A closed adapter is done; setting again must not hang or leak a thread. (It throws the executor's rejection.)
            assertThrows<java.util.concurrent.RejectedExecutionException> { timers.set(2u, 0uL) }
        }

        case("Connectivity and Lifecycle stubs send the right event bytes") {
            val t = FakeTransport()
            attach(t).use { core ->
                ConnectivityEvents(core).changed(true, NetKind.WIFI)
                ConnectivityEvents(core).changed(false, NetKind.NONE)
                LifecycleEvents(core).changed(AppState.BACKGROUND)
                val e = t.sentEvents
                assertEq(StandardPorts.Connectivity.PORT_ID, e[0].portId)
                assertEq(StandardPorts.Connectivity.CHANGED, e[0].methodId)
                assertEq("010000", hex(e[0].payload))
                assertEq("000400", hex(e[1].payload))
                assertEq(StandardPorts.Lifecycle.PORT_ID, e[2].portId)
                assertEq(StandardPorts.Lifecycle.CHANGED, e[2].methodId)
                assertEq("0200", hex(e[2].payload))
            }
        }

        case("JvmAdapters.standard covers every port that has an implementation; portable is the four that need nothing") {
            TempDir().use { dir ->
                val all = JvmAdapters.standard(dir.path) { }
                assertEq(
                    setOf(
                        StandardPorts.Clock.PORT_ID, StandardPorts.Rng.PORT_ID, StandardPorts.Log.PORT_ID, StandardPorts.Timer.PORT_ID,
                        StandardPorts.Http.PORT_ID, StandardPorts.Kv.PORT_ID, StandardPorts.SecureStore.PORT_ID, StandardPorts.Fs.PORT_ID,
                    ),
                    all.keys,
                )
                val sync = setOf(StandardPorts.Clock.PORT_ID, StandardPorts.Rng.PORT_ID, StandardPorts.Log.PORT_ID, StandardPorts.Timer.PORT_ID)
                for ((id, impl) in all) assertEq(id in sync, impl.sync, "sync flag of port $id")
                assertEq(sync, JvmAdapters.portable { }.keys)
                assertTrue(Fnv.fnv1a32("Clock.now_ms") in all.getValue(StandardPorts.Clock.PORT_ID).methods)
                assertEq(4, all.getValue(StandardPorts.Kv.PORT_ID).methods.size)
                assertEq(4, all.getValue(StandardPorts.SecureStore.PORT_ID).methods.size)
                assertEq(4, all.getValue(StandardPorts.Fs.PORT_ID).methods.size)
                assertEq(1, all.getValue(StandardPorts.Http.PORT_ID).methods.size)
            }
        }

        case("the default data directory follows the undra.data.dir property") {
            val previous = System.getProperty(JvmAdapters.DATA_DIR_PROPERTY)
            try {
                System.setProperty(JvmAdapters.DATA_DIR_PROPERTY, "/some/where")
                assertEq("/some/where", JvmAdapters.defaultDataDir().toString())
                System.clearProperty(JvmAdapters.DATA_DIR_PROPERTY)
                assertTrue(JvmAdapters.defaultDataDir().toString().endsWith(".undra/data"), JvmAdapters.defaultDataDir().toString())
            } finally {
                if (previous != null) System.setProperty(JvmAdapters.DATA_DIR_PROPERTY, previous) else System.clearProperty(JvmAdapters.DATA_DIR_PROPERTY)
            }
        }

        case("Platform detection says JVM here") {
            assertEq(false, Platform.isAndroid)
            assertEq("jvm", Platform.name)
        }

        case("the runtime version reported in RuntimeConfig and Hello is set") {
            assertTrue(UNDRA_RUNTIME_VERSION.isNotEmpty())
        }
    }

    @Test
    fun allCases() = assertPassed()
}
