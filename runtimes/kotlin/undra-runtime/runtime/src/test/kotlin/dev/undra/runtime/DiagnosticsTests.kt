package dev.undra.runtime

import dev.undra.runtime.adapters.DiagnosticsAdapter
import dev.undra.runtime.adapters.StandardFunctions
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.UndraBackgroundReport
import dev.undra.runtime.adapters.UndraPanicFrame
import dev.undra.runtime.adapters.UndraPanicReport
import dev.undra.runtime.support.FakeNative
import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.ManualMainThread
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.attach
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.replyPayload
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertBytes
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.fail
import dev.undra.runtime.testing.hex
import dev.undra.runtime.wire.Fnv
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.PortReply
import dev.undra.runtime.wire.Payloads.PortStatus
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.logging.Level
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private fun frame(address: Long, symbol: String? = null, file: String? = null, line: Int? = null) = UndraPanicFrame(address, symbol, file, line)

private fun sample(operation: String = "Todos.add", message: String = "boom"): UndraPanicReport = UndraPanicReport(
    message = message,
    location = "src/todos.rs:42:9",
    operation = operation,
    thread = "undra-core",
    frames = listOf(frame(0x1234, "todos::add", "src/todos.rs", 42), frame(0x5678)),
    namespace = "playground_core",
    coreVersion = "0.1.0",
    schemaHash = 0x0123456789abcdefL,
    imageId = "ab12cd34",
)

private fun portCall(t: FakeTransport, report: UndraPanicReport, portCallId: UInt = 0u): PortOutcome =
    t.portCall(StandardPorts.Diagnostics.PORT_ID, StandardPorts.Diagnostics.PANICKED, portCallId, UndraPanicReport.encodeToByteArray(report))

private fun backgroundReply(callId: UInt, report: UndraBackgroundReport): ByteArray =
    replyPayload(callId, ReplyStatus.OK, UndraBackgroundReport.encodeToByteArray(report))

/** A core over [t] whose main thread is [main], loaded with [options] and the ports they install (not the shared core). */
private fun coreOn(t: FakeTransport, main: ManualMainThread, onPanic: ((UndraPanicReport) -> Unit)?, onError: ((UndraUnhandledError) -> Unit)? = null, defaultAdapters: Boolean = false): ConnectedCore {
    val core = ConnectedCore(t, 5.seconds, main = main, onError = onError, onPanic = onPanic)
    core.installPorts(LoadOptions(expectedSchemaHash = HASH, defaultAdapters = defaultAdapters, onPanic = onPanic, onError = onError))
    t.connect(core, HASH)
    core.markConnected()
    return core
}

/**
 * The production-operations surface of the Kotlin runtime (ADR-046): the three records of the standard table, the `Diagnostics`
 * port behind `LoadOptions.onPanic`, the new statistics and `UndraCore.runInBackground`.
 */
class DiagnosticsTests : Suite() {
    init {
        // ---- ids and wire layouts -------------------------------------------------------------------------------

        case("the Diagnostics port and the run_background function ids equal fnv1a32 of their names") {
            assertEq(Fnv.fnv1a32("port.Diagnostics"), StandardPorts.Diagnostics.PORT_ID)
            assertEq(Fnv.fnv1a32("Diagnostics.panicked"), StandardPorts.Diagnostics.PANICKED)
            assertEq(Fnv.fnv1a32("fn.run_background"), StandardFunctions.RUN_BACKGROUND)
            assertEq(0xab68cd7cu, StandardPorts.Diagnostics.PORT_ID)
            assertEq(0xbd147e2eu, StandardPorts.Diagnostics.PANICKED)
            assertEq(0x0e5b14ffu, StandardFunctions.RUN_BACKGROUND)
        }

        case("UndraPanicFrame encodes the golden bytes of crates/undra-ports/tests/encoding.rs") {
            // panic_frame_is_address_symbol_file_line: address u64, symbol Option<String>, file Option<String>, line Option<u32>.
            assertBytes("2211000000000000" + "00" + "00" + "00", UndraPanicFrame.encodeToByteArray(frame(0x1122)))
            assertBytes(
                "0100000000000000" + "01" + "01000000" + "66" + "01" + "04000000" + "612e7273" + "01" + "07000000",
                UndraPanicFrame.encodeToByteArray(frame(1, "f", "a.rs", 7)),
            )
            assertEq(frame(0x1122), UndraPanicFrame.decodeAll(UndraPanicFrame.encodeToByteArray(frame(0x1122))))
            assertEq(frame(1, "f", "a.rs", 7), UndraPanicFrame.decodeAll(UndraPanicFrame.encodeToByteArray(frame(1, "f", "a.rs", 7))))
        }

        case("UndraPanicReport encodes the golden bytes of crates/undra-ports/tests/encoding.rs") {
            // panic_report_is_nine_fields_in_declaration_order.
            val report = UndraPanicReport("m", "l", "o", "t", listOf(frame(2, line = 1)), "n", "1.0", 0x0102L, "ab")
            val golden = "01000000" + "6d" + "01000000" + "6c" + "01000000" + "6f" + "01000000" + "74" +
                "01000000" + "0200000000000000" + "00" + "00" + "01" + "01000000" +
                "01000000" + "6e" + "03000000" + "312e30" + "0201000000000000" + "02000000" + "6162"
            assertBytes(golden, UndraPanicReport.encodeToByteArray(report))
            assertEq(report, UndraPanicReport.decodeAll(UndraPanicReport.encodeToByteArray(report)))
            val empty = UndraPanicReport("", "", "", "", emptyList(), "", "", 0L, "")
            assertBytes(
                "00000000" + "00000000" + "00000000" + "00000000" + "00000000" + "00000000" + "00000000" + "0000000000000000" + "00000000",
                UndraPanicReport.encodeToByteArray(empty),
            )
            assertEq(empty, UndraPanicReport.decodeAll(UndraPanicReport.encodeToByteArray(empty)))
        }

        case("UndraBackgroundReport encodes the golden bytes of crates/undra-ports/tests/encoding.rs") {
            // background_report_is_a_bool_and_three_counts.
            val report = UndraBackgroundReport(finished = true, replayed = 1, refetched = 2, stillPending = 3)
            assertBytes("01" + "01000000" + "02000000" + "03000000", UndraBackgroundReport.encodeToByteArray(report))
            assertEq(report, UndraBackgroundReport.decodeAll(UndraBackgroundReport.encodeToByteArray(report)))
            assertBytes("00" + "00000000" + "00000000" + "00000000", UndraBackgroundReport.encodeToByteArray(UndraBackgroundReport(false, 0, 0, 0)))
            // A bool is 0 or 1: anything else is a malformed report, not `true`.
            assertThrows<WireException> { UndraBackgroundReport.decodeAll(byteArrayOf(2) + ByteArray(12)) }
        }

        case("the records round-trip with unicode, the full u64 range and counts above Int.MAX_VALUE clamped") {
            val report = sample(operation = "computed Todos.visible", message = "boom é 🌊\nsecond line")
                .copy(schemaHash = -0x1L, frames = listOf(frame(Long.MIN_VALUE, "s", "é.rs", Int.MAX_VALUE), frame(-1L)), imageId = "")
            assertEq(report, UndraPanicReport.decodeAll(UndraPanicReport.encodeToByteArray(report)))
            assertEq(report.hashCode(), UndraPanicReport.decodeAll(UndraPanicReport.encodeToByteArray(report)).hashCode())
            assertTrue(report != report.copy(operation = "x"))
            // u64::MAX is -1 as a Long; a u32 line above Int.MAX_VALUE (a core cannot send one, a hostile peer can) is clamped.
            val wide = frame(1, "s", "f", 5)
            val bytes = UndraPanicFrame.encodeToByteArray(wide)
            bytes[bytes.size - 1] = 0x80.toByte() // line = 0x80000005
            assertEq(Int.MAX_VALUE, UndraPanicFrame.decodeAll(bytes).line)
            val counts = ByteArray(13).also { b -> b[0] = 1; for (i in 1..12) b[i] = 0xFF.toByte() }
            assertEq(UndraBackgroundReport(true, Int.MAX_VALUE, Int.MAX_VALUE, Int.MAX_VALUE), UndraBackgroundReport.decodeAll(counts))
            assertThrows<IllegalArgumentException> { UndraPanicFrame(1, line = -1) }
            assertThrows<IllegalArgumentException> { UndraBackgroundReport(true, -1, 0, 0) }
        }

        case("a truncated or over-long panic record is a WireException, at every cut") {
            fun <T> everyCut(what: String, bytes: ByteArray, decode: (ByteArray) -> T) {
                for (cut in 0 until bytes.size) assertThrows<WireException>("$what cut at $cut of ${bytes.size}") { decode(bytes.copyOf(cut)) }
                assertThrows<WireException.TrailingBytes>("$what with a byte too many") { decode(bytes + byteArrayOf(0)) }
            }
            everyCut("frame", UndraPanicFrame.encodeToByteArray(frame(1, "f", "a.rs", 7))) { UndraPanicFrame.decodeAll(it) }
            everyCut("report", UndraPanicReport.encodeToByteArray(sample())) { UndraPanicReport.decodeAll(it) }
            everyCut("background report", UndraBackgroundReport.encodeToByteArray(UndraBackgroundReport(true, 1, 2, 3))) { UndraBackgroundReport.decodeAll(it) }
            // A frame count that promises more frames than the bytes hold is refused before anything is allocated.
            val bare = sample().copy(frames = emptyList())
            val huge = UndraPanicReport.encodeToByteArray(bare)
            val countAt = listOf(bare.message, bare.location, bare.operation, bare.thread).sumOf { 4 + it.toByteArray().size }
            huge[countAt] = 0xFF.toByte()
            huge[countAt + 1] = 0xFF.toByte()
            huge[countAt + 2] = 0xFF.toByte()
            huge[countAt + 3] = 0x7F.toByte()
            assertThrows<WireException>("an absurd frame count") { UndraPanicReport.decodeAll(huge) }
        }

        case("the report's summary is the one line the runtime logs") {
            assertEq("Todos.add: boom (src/todos.rs:42:9)", sample().summary)
            assertEq("the core: boom (src/todos.rs:42:9)", sample(operation = "").summary)
        }

        // ---- the Diagnostics port and onPanic -----------------------------------------------------------------

        case("the Diagnostics adapter decodes the one report and hands it to its handler on the calling thread") {
            val seen = CopyOnWriteArrayList<Pair<UndraPanicReport, Thread>>()
            val impl = DiagnosticsAdapter { seen.add(it to Thread.currentThread()) }.portImpl()
            assertTrue(impl.sync, "Diagnostics is a sync port")
            val reply = runBlocking { impl.methods.getValue(StandardPorts.Diagnostics.PANICKED)(UndraPanicReport.encodeToByteArray(sample())) }
            assertEq(0, reply.size, "fire and forget: an empty reply body")
            assertEq(sample(), seen.single().first)
            assertTrue(Thread.currentThread() === seen.single().second, "the handler ran on the calling thread, not ${seen.single().second}")
            // Missing, truncated and trailing bytes are refused by the decode, not handed over half-read.
            for (args in listOf(NO_BYTES, UndraPanicReport.encodeToByteArray(sample()).copyOf(9), UndraPanicReport.encodeToByteArray(sample()) + byteArrayOf(1))) {
                assertThrows<WireException> { runBlocking { impl.methods.getValue(StandardPorts.Diagnostics.PANICKED)(args) } }
            }
            assertEq(1, seen.size)
        }

        case("a panic report is answered at once and reaches onPanic on the main thread, once, in order") {
            val t = FakeTransport()
            val main = ManualMainThread()
            val got = CopyOnWriteArrayList<Pair<String, String>>()
            val answers = CopyOnWriteArrayList<PortOutcome>()
            coreOn(t, main, { got.add(it.operation to it.message) }).use {
                // Three panics on the core's thread, one port call each, with nothing running the main thread.
                t.onCore {
                    answers.add(portCall(t, sample("explode", "first"), 0u))
                    answers.add(portCall(t, sample("explode_later", "second"), 0u))
                    answers.add(portCall(t, sample("task", "third"), 0u))
                }
                t.awaitCore()
                assertEq(3, answers.size, "every port call returned without the main thread's help")
                for (answer in answers) {
                    assertTrue(answer is PortOutcome.Sync, "a sync port answers inline: $answer")
                    val reply = PortReply.decode((answer as PortOutcome.Sync).reply)
                    assertEq(PortStatus.OK, reply.status)
                    assertEq(0, reply.body.size)
                }
                assertEq(emptyList<Pair<String, String>>(), got.toList(), "the handler has not run: it waits for the main thread")
                assertEq(3, main.pending)
                main.runPending()
                assertEq(listOf("explode" to "first", "explode_later" to "second", "task" to "third"), got.toList())
            }
        }

        case("onPanic runs on the runtime's main dispatcher thread") {
            val t = FakeTransport()
            val threads = CopyOnWriteArrayList<Pair<String, Boolean>>()
            val core = UndraCore.attach(
                t,
                LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false, onPanic = { threads.add(Thread.currentThread().name to UndraDispatchers.isMainThread()) }),
                makeShared = false,
            )
            core.use {
                t.onCore { portCall(t, sample()) }
                t.awaitCore()
                eventually("the report to reach onPanic") { threads.isNotEmpty() }
                assertEq(listOf("undra-main" to true), threads.toList())
            }
        }

        case("a report goes through the real in-process path: the sync reply is handed back on the core's thread") {
            val native = FakeNative("diagnostics_inproc")
            val got = CopyOnWriteArrayList<UndraPanicReport>()
            val entry = CoreEntry("diagnostics_inproc", HASH) { native }
            val core = entry.load(LoadOptions(defaultAdapters = false, onPanic = { got.add(it) }))
            core.use {
                var result = -1
                var reply: ByteArray? = null
                native.onCore {
                    val (code, bytes) = native.portCallFull(
                        StandardPorts.Diagnostics.PORT_ID.toInt(),
                        StandardPorts.Diagnostics.PANICKED.toInt(),
                        0,
                        UndraPanicReport.encodeToByteArray(sample("explode", "kaboom")),
                    )
                    result = code
                    reply = bytes
                }
                assertEq(0, result, "port answered synchronously")
                assertEq(PortReply(0u, PortStatus.OK, NO_BYTES), PortReply.decode(reply ?: fail("no sync reply")))
                eventually("onPanic") { got.isNotEmpty() }
                assertEq(sample("explode", "kaboom"), got.single())
            }
        }

        case("without onPanic the runtime logs one error line: the operation, the message and the location") {
            val t = FakeTransport()
            val main = ManualMainThread()
            LogCapture("dev.undra.runtime").use { log ->
                coreOn(t, main, onPanic = null).use {
                    portCall(t, sample("computed Todos.visible", "index out of bounds"))
                    main.runPending()
                    val lines = log.records.filter { it.level == Level.SEVERE }
                    assertEq(1, lines.size, "exactly one line: ${log.messages()}")
                    val line = lines.single().message
                    assertTrue("computed Todos.visible" in line && "index out of bounds" in line && "src/todos.rs:42:9" in line, line)
                    assertTrue('\n' !in line, "a single line: $line")
                }
            }
        }

        case("with onPanic set the runtime does not also log the report") {
            val t = FakeTransport()
            val main = ManualMainThread()
            LogCapture("dev.undra.runtime").use { log ->
                coreOn(t, main, onPanic = { }).use {
                    portCall(t, sample())
                    main.runPending()
                    assertEq(emptyList<String>(), log.records.filter { it.level.intValue() >= Level.WARNING.intValue() }.map { it.message })
                }
            }
        }

        case("a throwing onPanic is caught, logged and reported to onError, and the next report is delivered all the same") {
            val t = FakeTransport()
            val main = ManualMainThread()
            val delivered = CopyOnWriteArrayList<String>()
            val reported = CopyOnWriteArrayList<UndraUnhandledError>()
            LogCapture("dev.undra.runtime").use { log ->
                coreOn(t, main, onPanic = {
                    delivered.add(it.message)
                    if (it.message == "first") throw IllegalStateException("the crash reporter is down")
                }, onError = { reported.add(it) }).use {
                    portCall(t, sample(message = "first"))
                    portCall(t, sample(message = "second"))
                    main.runPending()
                    assertEq(listOf("first", "second"), delivered.toList())
                    val failure = reported.single()
                    assertEq("onPanic", failure.operation)
                    assertTrue("the crash reporter is down" in failure.message!!, failure.message!!)
                    assertTrue(failure.error is UndraCallError.Malformed, "mapped onto the closed set: ${failure.error}")
                    assertTrue(log.records.any { it.level == Level.SEVERE && it.message.contains("onPanic") }, "logged: ${log.messages()}")
                }
            }
        }

        case("a malformed report is the port's failure: answered unavailable, onPanic not called, onError told") {
            val t = FakeTransport()
            val main = ManualMainThread()
            val got = CopyOnWriteArrayList<UndraPanicReport>()
            val reported = CopyOnWriteArrayList<UndraUnhandledError>()
            LogCapture("dev.undra.runtime").use {
                coreOn(t, main, { got.add(it) }, onError = { reported.add(it) }).use {
                    val outcome = t.portCall(StandardPorts.Diagnostics.PORT_ID, StandardPorts.Diagnostics.PANICKED, 0u, byteArrayOf(1, 2, 3))
                    assertEq(PortOutcome.Unavailable, outcome)
                    main.runPending()
                    assertEq(0, got.size)
                    eventually("the failure to be reported") { reported.isNotEmpty() }
                    assertTrue(reported.single().operation.contains("0x${StandardPorts.Diagnostics.PANICKED.toString(16)}"), reported.single().operation)
                }
            }
        }

        case("the Diagnostics adapter is registered also with the default adapters off, and an adapter of the app replaces it") {
            val t = FakeTransport()
            val main = ManualMainThread()
            val got = CopyOnWriteArrayList<String>()
            coreOn(t, main, { got.add("onPanic ${it.message}") }, defaultAdapters = false).use {
                portCall(t, sample(message = "a"))
                main.runPending()
                assertEq(listOf("onPanic a"), got.toList())
            }
            val t2 = FakeTransport()
            val mine = CopyOnWriteArrayList<String>()
            attach(t2, adapters = mapOf(StandardPorts.Diagnostics.PORT_ID to DiagnosticsAdapter { mine.add(it.message) }.portImpl())).use {
                portCall(t2, sample(message = "b"))
                assertEq(listOf("b"), mine.toList())
            }
        }

        case("a closed main thread does not lose the report: it is logged") {
            val t = FakeTransport()
            val closedMain = object : MainThread {
                override val dispatcher: CoroutineDispatcher get() = Dispatchers.Unconfined

                override fun post(task: Runnable) {
                    throw java.util.concurrent.RejectedExecutionException("the looper is quitting")
                }

                override fun isCurrent(): Boolean = false
            }
            LogCapture("dev.undra.runtime").use { log ->
                val core = ConnectedCore(t, 5.seconds, main = closedMain, onPanic = { })
                core.installPorts(LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false))
                t.connect(core, HASH)
                core.use {
                    val outcome = portCall(t, sample("explode", "kaboom"))
                    assertTrue(outcome is PortOutcome.Sync, "the core is answered all the same: $outcome")
                    assertTrue(log.records.any { it.level == Level.SEVERE && it.message.contains("kaboom") }, log.messages().toString())
                }
            }
        }

        case("LoadOptions carries onPanic through the schema-hash default, and says so in toString") {
            val handler: (UndraPanicReport) -> Unit = { }
            val notice: (String) -> Unit = { }
            val options = LoadOptions(onPanic = handler, onDevNotice = notice)
            assertTrue("onPanic=set" in options.toString(), options.toString())
            assertTrue("onPanic=none" in LoadOptions().toString())
            val withHash = options.withSchemaHashDefault(HASH)
            assertTrue(withHash.onPanic === handler, "onPanic survives the default")
            assertTrue(withHash.onDevNotice === notice, "onDevNotice survives it too")
            assertEq(HASH, withHash.expectedSchemaHash)
        }

        // ---- statistics ----------------------------------------------------------------------------------------

        case("the statistics read panic_reports and the background object, and tolerate a core without them") {
            val json = "{\"live_handles\":3,\"panics\":8,\"panic_reports\":8,\"background\":{\"tasks\":3,\"pending\":2,\"runs\":4,\"finished\":1,\"replayed\":5,\"refetched\":6}}"
            val s = UndraStats.fromCoreJson(json, 0, 0)
            assertEq(8L, s.panicReports)
            assertEq(BackgroundStats(tasks = 3, pending = 2, runs = 4L, finished = 1L, replayed = 5L, refetched = 6L), s.background)
            assertTrue(s.background.hasPendingWork)
            assertTrue(s.toString().contains("panicReports=8") && s.toString().contains("pending=2"), s.toString())
            assertEq(BackgroundStats(3, 2, 4L, 1L, 5L, 6L).hashCode(), s.background.hashCode())

            val old = UndraStats.fromCoreJson("{\"live_handles\":3,\"panics\":1}", 0, 0)
            assertEq(UndraStats.UNKNOWN.toLong(), old.panicReports)
            assertEq(UndraStats.UNKNOWN, old.background.tasks)
            assertEq(UndraStats.UNKNOWN, old.background.pending)
            assertEq(UndraStats.UNKNOWN.toLong(), old.background.runs)
            assertTrue(!old.background.hasPendingWork)
            // A background object with a field missing, one that is not an object, and counts that cannot be counts.
            val partial = UndraStats.fromCoreJson("{\"background\":{\"pending\":1,\"runs\":-2}}", 0, 0)
            assertEq(1, partial.background.pending)
            assertEq(-2L, partial.background.runs)
            assertEq(UndraStats.UNKNOWN, partial.background.tasks)
            assertEq(UndraStats.UNKNOWN.toLong(), UndraStats.fromCoreJson("{\"background\":[1],\"panic_reports\":\"x\"}", 0, 0).panicReports)
            assertEq(UndraStats.UNKNOWN, UndraStats.fromCoreJson("{\"background\":7}", 0, 0).background.pending)
            assertEq(Int.MAX_VALUE, UndraStats.fromCoreJson("{\"background\":{\"pending\":99999999999}}", 0, 0).background.pending)
            // The constructor still takes liveHandles alone.
            assertEq(UndraStats.UNKNOWN, UndraStats(0).background.pending)
        }

        case("a core's stats() carries the background numbers the transport reports") {
            val t = FakeTransport()
            t.statsJson = "{\"live_handles\":0,\"panic_reports\":2,\"background\":{\"tasks\":3,\"pending\":1,\"runs\":0,\"finished\":0,\"replayed\":0,\"refetched\":0}}"
            attach(t).use { core ->
                val stats = core.stats()
                assertEq(2L, stats.panicReports)
                assertEq(3, stats.background.tasks)
                assertEq(1, stats.background.pending)
            }
        }

        // ---- runInBackground -----------------------------------------------------------------------------------

        case("runInBackground calls the standard function with the deadline as one u64 and returns the report") {
            val t = FakeTransport()
            t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK, UndraBackgroundReport.encodeToByteArray(UndraBackgroundReport(true, 2, 1, 0))) }
            attach(t).use { core ->
                val report = runBlocking { core.runInBackground(540_000L) }
                assertEq(UndraBackgroundReport(finished = true, replayed = 2, refetched = 1, stillPending = 0), report)
                val call = t.calls.single()
                assertEq(CallTarget.FreeFunction(0x0e5b14ffu), call.target)
                assertBytes("603d080000000000", call.args) // 540_000 as a little-endian u64
                assertEq(0, core.stats().hostPendingCalls)
            }
        }

        case("a deadline the core cannot take as a u64 is sent as zero, and the largest one as itself") {
            val t = FakeTransport()
            t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK, UndraBackgroundReport.encodeToByteArray(UndraBackgroundReport(false, 0, 0, 1))) }
            attach(t).use { core ->
                runBlocking {
                    core.runInBackground(-5L)
                    core.runInBackground(Long.MAX_VALUE)
                }
                assertEq(listOf("0000000000000000", "ffffffffffffff7f"), t.calls.map { hex(it.args) })
            }
        }

        case("cancelling runInBackground cancels the call in the core and the caller sees the cancellation") {
            val t = FakeTransport()
            attach(t).use { core ->
                runBlocking {
                    var failure: Throwable? = null
                    val job = launch(Dispatchers.Default) {
                        try {
                            core.runInBackground(30_000L)
                        } catch (e: Throwable) {
                            failure = e
                            throw e
                        }
                    }
                    eventually("the call reaches the core") { t.calls.isNotEmpty() }
                    job.cancelAndJoin()
                    assertEq(listOf(t.calls.single().callId), t.cancels.toList())
                    assertTrue(failure is CancellationException, "cancelled, not mapped: $failure")
                    assertEq(0, core.stats().hostPendingCalls)
                }
            }
        }

        case("runInBackground on a closed core fails as UndraCallError.Unavailable (closed), without touching the core") {
            val t = FakeTransport()
            val core = attach(t)
            core.close()
            val e = assertThrows<UndraCallError.Unavailable> { runBlocking { core.runInBackground(1_000L) } }
            assertEq(UndraTransportException.Reason.CLOSED, e.transport.reason)
            assertEq(0, t.calls.size)
        }

        case("runInBackground on a core that was never loaded fails as Unavailable (closed), like every call on the placeholder") {
            val entry = CoreEntry("diagnostics_unloaded", HASH) { FakeNative("diagnostics_unloaded") }
            LogCapture("dev.undra.runtime").use {
                val e = assertThrows<UndraCallError.Unavailable> { runBlocking { entry.core.runInBackground(1_000L) } }
                assertEq(UndraTransportException.Reason.CLOSED, e.transport.reason)
            }
        }

        case("runInBackground maps a cancelled, refused or unreadable reply onto UndraCallError") {
            val t = FakeTransport()
            attach(t).use { core ->
                t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.CANCELLED) }
                assertThrows<UndraCallError.CancelledByCore>("status 3 is the core cancelling the run") { runBlocking { core.runInBackground(1_000L) } }
                val reason = UndraWriter().also { it.writeStr("no such function") }.toByteArray()
                t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.BAD_REQUEST, reason) }
                assertEq("no such function", assertThrows<UndraCallError.Refused> { runBlocking { core.runInBackground(1_000L) } }.reason)
                t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK, byteArrayOf(1, 0)) }
                assertThrows<UndraCallError.Malformed>("a body that is not a BackgroundReport") { runBlocking { core.runInBackground(1_000L) } }
                t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK, UndraBackgroundReport.encodeToByteArray(UndraBackgroundReport(true, 0, 0, 0)) + byteArrayOf(9)) }
                assertThrows<UndraCallError.Malformed>("trailing bytes") { runBlocking { core.runInBackground(1_000L) } }
            }
        }

        case("a status 3 reply with a panic is reported as the core's own panic, not as a report") {
            val t = FakeTransport()
            t.onCall = { call ->
                val w = UndraWriter()
                w.writeStr("kaboom")
                w.writeStr("at x")
                t.replyOnCore(call.callId, ReplyStatus.PANIC, w.toByteArray())
            }
            attach(t).use { core ->
                assertEq("kaboom", assertThrows<UndraCallError.Panicked> { runBlocking { core.runInBackground(1_000L) } }.panicMessage)
            }
        }

        case("a test double that overrides call serves runInBackground too") {
            val double = object : UndraCore() {
                var seen: Pair<CallTarget, ByteArray>? = null

                override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
                    seen = target to args
                    return UndraBackgroundReport.encodeToByteArray(UndraBackgroundReport(true, 0, 0, 0))
                }
            }
            assertEq(UndraBackgroundReport(true, 0, 0, 0), runBlocking { double.runInBackground(5L) })
            assertEq(CallTarget.FreeFunction(StandardFunctions.RUN_BACKGROUND), double.seen!!.first)
            // The base class refuses what it does not implement, like every other call.
            assertThrows<UnsupportedOperationException> { runBlocking { object : UndraCore() {}.runInBackground(5L) } }
            val args = double.seen!!.second
            assertEq(5uL, UndraReader(args).readU64())
        }
    }

    @Test
    fun allCases() = assertPassed()
}
