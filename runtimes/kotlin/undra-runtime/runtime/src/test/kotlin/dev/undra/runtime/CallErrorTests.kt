package dev.undra.runtime

import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.attach
import dev.undra.runtime.support.changeSet
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.full
import dev.undra.runtime.support.replyPayload
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.logging.Level
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private const val METHOD = 0x11u
private val TARGET = CallTarget.ObjectMethod(Handle(7L), METHOD)
private val ARGS = byteArrayOf(1, 2, 3)

private fun strings(vararg s: String): ByteArray = UndraWriter().also { w -> s.forEach { w.writeStr(it) } }.toByteArray()

/** A typed error shaped like the generated ones: a sealed `UndraException` with a companion codec. */
private sealed class LabError(message: String) : UndraException(message) {
    data object Empty : LabError("empty")

    data class Rejected(val code: UShort) : LabError("rejected $code")

    companion object : UndraCodec<LabError> {
        override fun encode(w: UndraWriter, v: LabError) {
            when (v) {
                Empty -> w.writeU16(0u)
                is Rejected -> {
                    w.writeU16(1u)
                    w.writeU16(v.code)
                }
            }
        }

        override fun decode(r: UndraReader): LabError =
            when (val tag = r.readU16().toInt()) {
                0 -> Empty
                1 -> Rejected(r.readU16())
                else -> throw WireException.InvalidTag(tag.toUInt(), 0, "LabError")
            }
    }
}

private fun typed(error: LabError): ByteArray = LabError.encodeToByteArray(error)

private fun reply(status: ReplyStatus, body: ByteArray = ByteArray(0)) = UndraReplyException(status, body)

/** A store shaped like the generated ones, to see what `observeAll` does when the core is gone. */
private class ProbeStore(core: UndraCore, handle: Long) : UndraStore(core, handle) {
    init {
        observeAll()
    }

    override fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader) = Unit
}

/** Reports collected from `LoadOptions.onError`. */
private class Reports {
    val all = CopyOnWriteArrayList<UndraUnhandledError>()
    val threads = CopyOnWriteArrayList<String>()
    val handler: (UndraUnhandledError) -> Unit = {
        all.add(it)
        threads.add(Thread.currentThread().name)
    }
}

private fun attachReporting(t: FakeTransport, onError: ((UndraUnhandledError) -> Unit)?): UndraCore =
    UndraCore.attach(t, LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false, onError = onError), makeShared = false)

/**
 * ADR-032, amendment A: the closed set of call failures, the one mapping function, commands that report, the
 * placeholder `shared`. The cases mirror the Swift `CallErrorTests`.
 */
class CallErrorTests : Suite() {
    init {
        // ---- the mapping ---------------------------------------------------------------------------------

        case("every reply status maps onto its case") {
            assertTrue(UndraCallError.mapped(reply(ReplyStatus.CANCELLED)) is UndraCallError.CancelledByCore)
            val panic = UndraCallError.mapped(reply(ReplyStatus.PANIC, strings("kaboom", "frame 1"))) as UndraCallError.Panicked
            assertEq("kaboom", panic.panicMessage)
            assertEq("frame 1", panic.backtrace)
            assertTrue(panic.message!!.contains("kaboom"), panic.message!!)
            val refused = UndraCallError.mapped(reply(ReplyStatus.BAD_REQUEST, strings("stale handle"))) as UndraCallError.Refused
            assertEq("stale handle", refused.reason)
            assertTrue(refused.message!!.contains("stale handle"), refused.message!!)
            assertTrue(UndraCallError.mapped(reply(ReplyStatus.ERROR, byteArrayOf(1, 2))) is UndraCallError.Malformed, "a typed error on a method without one")
            assertTrue(UndraCallError.mapped(reply(ReplyStatus.STREAM_OPENED)) is UndraCallError.Malformed)
            assertTrue(UndraCallError.mapped(reply(ReplyStatus.OK)) is UndraCallError.Malformed)
        }

        case("an unreadable panic or refusal body degrades to a placeholder text instead of throwing") {
            val panic = UndraCallError.mapped(reply(ReplyStatus.PANIC, byteArrayOf(9, 9))) as UndraCallError.Panicked
            assertEq("<undecodable panic report>", panic.panicMessage)
            val refused = UndraCallError.mapped(reply(ReplyStatus.BAD_REQUEST, byteArrayOf(9))) as UndraCallError.Refused
            assertEq("<undecodable reason>", refused.reason)
        }

        case("cancellation, a closed set member and anything that is not Undra's pass through unchanged") {
            val cancelled = CancellationException("the coroutine was cancelled")
            assertTrue(UndraCallError.mapped(cancelled) === cancelled, "a platform cancellation is not a core cancellation")
            val already = UndraCallError.Refused("x")
            assertTrue(UndraCallError.mapped(already) === already)
            val foreign = IllegalStateException("a bug of the app")
            assertTrue(UndraCallError.mapped(foreign) === foreign)
            val typedError = LabError.Empty
            assertTrue(UndraCallError.mapped(typedError) === typedError, "a typed error is not the runtime's to classify")
        }

        case("transport failures are Unavailable, protocol and wire failures are Malformed, refusals are Refused") {
            for (reason in UndraTransportException.Reason.entries) {
                val transport = UndraTransportException(reason, "t")
                val mapped = UndraCallError.mapped(transport) as UndraCallError.Unavailable
                assertTrue(mapped.transport === transport && mapped.cause === transport)
                assertTrue(mapped.message!!.contains("unavailable"), mapped.message!!)
            }
            assertTrue(UndraCallError.mapped(UndraProtocolException("reply for another call")) is UndraCallError.Malformed)
            assertTrue(UndraCallError.mapped(WireException.UnexpectedEof(4, 0)) is UndraCallError.Malformed)
            assertTrue(UndraCallError.mapped(UndraPortException(byteArrayOf(1))) is UndraCallError.Malformed)
            assertTrue(UndraCallError.mapped(UndraModeException("snapshots are only available in process")) is UndraCallError.Refused)
            assertTrue(UndraCallError.mapped(UndraRestoreException(3)) is UndraCallError.Refused)
        }

        case("a remote core that came back with another schema is Unavailable, naming both hashes") {
            val mapped = UndraCallError.mapped(UndraSchemaMismatchException(1uL, 2uL)) as UndraCallError.Unavailable
            assertEq(UndraTransportException.Reason.CONNECTION_LOST, mapped.transport.reason)
            assertTrue(mapped.message!!.contains("0x1") && mapped.message!!.contains("0x2"), mapped.message!!)
        }

        case("a plain UndraException, what RemoteTransport throws for a lost connection, is Unavailable") {
            val mapped = UndraCallError.mapped(UndraException("the connection to ws://x is closed")) as UndraCallError.Unavailable
            assertEq(UndraTransportException.Reason.CONNECTION_LOST, mapped.transport.reason)
            assertTrue(mapped.message!!.contains("ws://x"), mapped.message!!)
        }

        case("a typed reply is decoded into the method's own error; a body that does not decode is Malformed") {
            val rejected = UndraCallError.mapped(reply(ReplyStatus.ERROR, typed(LabError.Rejected(7u))), LabError)
            assertEq<Throwable>(LabError.Rejected(7u), rejected)
            assertTrue(UndraCallError.mapped(reply(ReplyStatus.ERROR, typed(LabError.Empty)), LabError) === LabError.Empty)
            assertTrue(UndraCallError.mapped(reply(ReplyStatus.ERROR, byteArrayOf(9, 9, 9)), LabError) is UndraCallError.Malformed)
            assertTrue(UndraCallError.mapped(reply(ReplyStatus.ERROR, typed(LabError.Empty) + byteArrayOf(1)), LabError) is UndraCallError.Malformed, "trailing bytes")
        }

        case("with a domain, every other failure still maps as it does without one") {
            assertTrue(UndraCallError.mapped(reply(ReplyStatus.PANIC, strings("p", "b")), LabError) is UndraCallError.Panicked)
            assertTrue(UndraCallError.mapped(reply(ReplyStatus.CANCELLED), LabError) is UndraCallError.CancelledByCore)
            assertTrue(UndraCallError.mapped(UndraTransportException(UndraTransportException.Reason.CLOSED, "closed"), LabError) is UndraCallError.Unavailable)
            val cancelled = CancellationException("c")
            assertTrue(UndraCallError.mapped(cancelled, LabError) === cancelled)
        }

        case("a stream the core ended with a failure maps by the failure's status, as a failed call does (ADR-036)") {
            assertTrue(UndraCallError.mappedStream(reply(ReplyStatus.CANCELLED)) is UndraCallError.CancelledByCore)
            val panic = UndraCallError.mappedStream(reply(ReplyStatus.PANIC, strings("boom", "at core.rs:1"))) as UndraCallError.Panicked
            assertEq("boom", panic.panicMessage)
            assertEq("at core.rs:1", panic.backtrace)
            val refused = UndraCallError.mappedStream(reply(ReplyStatus.BAD_REQUEST, strings("stale handle"))) as UndraCallError.Refused
            assertEq("stale handle", refused.reason, "status 5 is Refused, the case a failed call maps it to")
            assertTrue(UndraCallError.mappedStream(UndraTransportException(UndraTransportException.Reason.CLOSED, "c")) is UndraCallError.Unavailable)
        }

        case("a typed error item on a stream without an error type is Malformed, not a guess at the core's text") {
            assertTrue(UndraCallError.mappedStream(reply(ReplyStatus.ERROR, strings("cancelled: a restore replaced the receiver"))) is UndraCallError.Malformed)
            assertTrue(UndraCallError.mappedStream(reply(ReplyStatus.ERROR, strings("the stream panicked: boom"))) is UndraCallError.Malformed)
            assertTrue(UndraCallError.mappedStream(reply(ReplyStatus.ERROR, byteArrayOf(1))) is UndraCallError.Malformed, "garbage")
        }

        case("a stream with an error type decodes its typed item as E; failures map as for a stream without one") {
            assertEq<Throwable>(LabError.Rejected(3u), UndraCallError.mappedStream(reply(ReplyStatus.ERROR, typed(LabError.Rejected(3u))), LabError))
            // flag 2 carries only the stream's E now (ADR-036): text the core wrote for a cancellation is not read as one.
            assertTrue(UndraCallError.mappedStream(reply(ReplyStatus.ERROR, strings("cancelled: the runtime shut down")), LabError) is UndraCallError.Malformed)
            assertTrue(UndraCallError.mappedStream(reply(ReplyStatus.ERROR, byteArrayOf(9, 9, 9)), LabError) is UndraCallError.Malformed)
            assertTrue(UndraCallError.mappedStream(reply(ReplyStatus.CANCELLED), LabError) is UndraCallError.CancelledByCore)
            assertTrue(UndraCallError.mappedStream(reply(ReplyStatus.PANIC, strings("x", "")), LabError) is UndraCallError.Panicked)
            assertTrue(UndraCallError.mappedStream(reply(ReplyStatus.BAD_REQUEST, strings("r")), LabError) is UndraCallError.Refused)
            assertTrue(UndraCallError.mappedStream(UndraTransportException(UndraTransportException.Reason.CLOSED, "c"), LabError) is UndraCallError.Unavailable)
        }

        case("an E is never mistaken for the core's String, and the reverse (the tie-break is gone with ADR-036)") {
            // `LabError.Empty` is the two bytes `00 00`: an E.
            assertEq<Throwable>(LabError.Empty, UndraCallError.mappedStream(reply(ReplyStatus.ERROR, byteArrayOf(0, 0)), LabError))
            // Four zero bytes were the empty String that used to win; as an item of the stream's E they leave two bytes over: Malformed.
            val read = UndraCallError.mappedStream(reply(ReplyStatus.ERROR, byteArrayOf(0, 0, 0, 0)), LabError)
            assertTrue(read is UndraCallError.Malformed, "$read")
        }

        // ---- reporting -----------------------------------------------------------------------------------

        case("report logs at error level, maps the failure and calls the handler once on the calling thread") {
            val t = FakeTransport()
            val reports = Reports()
            LogCapture("dev.undra.runtime").use { log ->
                attachReporting(t, reports.handler).use { core ->
                    core.report(reply(ReplyStatus.BAD_REQUEST, strings("stale handle")), "TodoStore.toggle")
                    val report = reports.all.single()
                    assertEq("TodoStore.toggle", report.operation)
                    assertTrue(report.error is UndraCallError.Refused)
                    assertTrue(report.cause === report.error)
                    assertEq("TodoStore.toggle failed: ${report.error.message}", report.message)
                    assertEq(Thread.currentThread().name, reports.threads.single())
                    assertTrue(log.records.any { it.level == Level.SEVERE && it.message.contains("TodoStore.toggle failed") }, "logged at error level")
                }
            }
        }

        case("report without a handler only logs") {
            val t = FakeTransport()
            LogCapture("dev.undra.runtime").use { log ->
                attachReporting(t, null).use { core ->
                    core.report(UndraTransportException(UndraTransportException.Reason.CLOSED, "closed"), "configureRemote")
                    assertTrue(log.records.any { it.level == Level.SEVERE && it.message.contains("configureRemote failed") })
                }
            }
        }

        case("a failure that is not Undra's is reported as Malformed, keeping the original as the cause") {
            val t = FakeTransport()
            val reports = Reports()
            attachReporting(t, reports.handler).use { core ->
                val bug = IllegalStateException("a bug")
                core.report(bug, "Todos.toggle")
                val error = reports.all.single().error
                assertTrue(error is UndraCallError.Malformed && error.cause === bug, "$error")
            }
        }

        case("a handler that calls a failing command is not called again; the nested report is only logged") {
            val t = FakeTransport()
            val seen = CopyOnWriteArrayList<String>()
            lateinit var core: UndraCore
            core = attachReporting(t) { unhandled ->
                seen.add(unhandled.operation)
                core.report(UndraTransportException(UndraTransportException.Reason.CLOSED, "closed"), "nested")
            }
            core.use {
                core.report(reply(ReplyStatus.CANCELLED), "outer")
                assertEq(listOf("outer"), seen.toList())
                // The guard is released afterwards: the next report reaches the handler again.
                core.report(reply(ReplyStatus.CANCELLED), "later")
                assertEq(listOf("outer", "later"), seen.toList())
            }
        }

        case("a handler that throws an Exception is contained and logged; report never throws") {
            val t = FakeTransport()
            LogCapture("dev.undra.runtime").use { log ->
                attachReporting(t) { throw IllegalStateException("the handler is broken") }.use { core ->
                    core.report(reply(ReplyStatus.CANCELLED), "Todos.toggle")
                    assertTrue(log.records.any { it.thrown is IllegalStateException }, "the handler's failure was logged")
                }
            }
        }

        case("reports from 8 threads, 25 each, all reach the handler") {
            val t = FakeTransport()
            val reports = Reports()
            attachReporting(t, reports.handler).use { core ->
                val done = CountDownLatch(8)
                repeat(8) { n ->
                    Thread {
                        repeat(25) { core.report(reply(ReplyStatus.CANCELLED), "thread $n") }
                        done.countDown()
                    }.start()
                }
                assertTrue(done.await(10, TimeUnit.SECONDS))
                assertEq(200, reports.all.size)
            }
        }

        case("a malformed change-set is dropped and reported from the delivery thread, not the core's") {
            val t = FakeTransport()
            val reports = Reports()
            attachReporting(t, reports.handler).use {
                t.onCore { t.events.onChangeSet(byteArrayOf(1, 2, 3)) }
                eventually("the malformed change-set was reported") { reports.all.isNotEmpty() }
                val report = reports.all.single()
                assertTrue(report.operation.startsWith("change-set"), report.operation)
                assertTrue(report.error is UndraCallError.Malformed)
                assertTrue(reports.threads.single() != "fake-core", "reported on ${reports.threads.single()}")
            }
        }

        case("a store change that cannot be applied is reported and skipped") {
            val t = FakeTransport()
            val reports = Reports()
            attachReporting(t, reports.handler).use { core ->
                // A store written by hand (no generated catch): the mirror reports its failure.
                val store = object : UndraStore(core, 5L) {
                    override fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader) {
                        Codecs.u32.decode(reader)
                        reader.finish()
                    }
                }
                t.onCore { t.events.onChangeSet(changeSet(1uL, full(5L, 0u, byteArrayOf(1)))) }
                eventually("the failure was reported") { reports.all.isNotEmpty() }
                val report = reports.all.single()
                assertTrue(report.operation.contains("apply(signal: 0)"), report.operation)
                assertTrue(report.error is UndraCallError.Malformed)
                store.close()
            }
        }

        case("a port implementation that fails is reported and answers unavailable") {
            val t = FakeTransport()
            val reports = Reports()
            val port = 0xaaaa0001u
            val method = 0xbbbb0003u
            val impl = PortImpl(true, mapOf(method to { _: ByteArray -> throw IllegalStateException("boom") }))
            val core = UndraCore.attach(
                t,
                LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false, adapters = mapOf(port to impl), onError = reports.handler),
                makeShared = false,
            )
            core.use {
                assertEq(PortOutcome.Unavailable, t.portCall(port, method, 1u, ByteArray(0)))
                eventually("the port failure was reported") { reports.all.isNotEmpty() }
                assertEq("port 0xaaaa0001 method 0xbbbb0003", reports.all.single().operation)
            }
        }

        // ---- constructors and stores ---------------------------------------------------------------------

        case("constructObject maps what construct throws") {
            val t = FakeTransport()
            attach(t).use { core ->
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, Codecs.handle.encodeToByteArray(0x100000005L)) }
                assertEq(0x100000005L, core.constructObject(3u, 4u, ARGS))
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.BAD_REQUEST, strings("undecodable arguments")) }
                assertEq("undecodable arguments", assertThrows<UndraCallError.Refused> { core.constructObject(3u, 4u, ARGS) }.reason)
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, Codecs.handle.encodeToByteArray(0L)) }
                assertTrue(assertThrows<UndraCallError.Malformed> { core.constructObject(3u, 4u, ARGS) }.detail.contains("null handle"))
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.PANIC, strings("boom", "")) }
                assertEq("boom", assertThrows<UndraCallError.Panicked> { core.constructObject(3u, 4u, ARGS) }.panicMessage)
            }
            val closed = attach(FakeTransport())
            closed.close()
            assertEq(UndraTransportException.Reason.CLOSED, assertThrows<UndraCallError.Unavailable> { closed.constructObject(3u, 4u, ARGS) }.transport.reason)
        }

        case("observeAll closes the store and throws Unavailable when the core is gone") {
            val t = FakeTransport()
            val core = attach(t)
            core.close()
            val failure = assertThrows<UndraCallError.Unavailable> { ProbeStore(core, 9L) }
            assertEq(UndraTransportException.Reason.CLOSED, failure.transport.reason)
        }

        case("a core closed by the host fails pending and later calls with a CLOSED transport failure; a lost connection with CONNECTION_LOST") {
            val t = FakeTransport()
            val core = attach(t)
            core.close()
            val closed = assertThrows<UndraTransportException> { core.callSync(TARGET, METHOD, ARGS) }
            assertEq(UndraTransportException.Reason.CLOSED, closed.reason)
            val lost = FakeTransport()
            val remote = attach(lost)
            lost.events.onClosed(IllegalStateException("socket reset"))
            val failure = assertThrows<UndraTransportException> { remote.callSync(TARGET, METHOD, ARGS) }
            assertEq(UndraTransportException.Reason.CONNECTION_LOST, failure.reason)
            assertTrue(failure.message!!.contains("socket reset"), failure.message!!)
        }

        case("a reply or stream item the transport cannot decode fails the call as a protocol failure, which maps to Malformed") {
            val t = FakeTransport()
            attach(t).use { core ->
                t.onCall = { call -> t.onCore { t.events.onMalformed(call.callId, UndraProtocolException("the core sent a malformed reply: test")) } }
                val failure = runBlocking {
                    try {
                        core.call(TARGET, METHOD, ARGS)
                        null
                    } catch (e: Exception) {
                        e
                    }
                }
                assertTrue(failure is UndraProtocolException, "got $failure")
                assertTrue(UndraCallError.mapped(failure!!) is UndraCallError.Malformed, "maps to Malformed")
                val streamFailure = runBlocking {
                    try {
                        core.stream(TARGET, METHOD, ARGS).collect {}
                        null
                    } catch (e: Exception) {
                        e
                    }
                }
                assertTrue(streamFailure is UndraProtocolException, "got $streamFailure")
                assertTrue(UndraCallError.mappedStream(streamFailure!!) is UndraCallError.Malformed, "maps to Malformed")
            }
        }

        case("a refused snapshot is an UndraRestoreException with the core's code") {
            val t = FakeTransport()
            t.snapshotBytes = byteArrayOf(1)
            t.restoreResult = 6
            attach(t).use { core ->
                assertEq(6, assertThrows<UndraRestoreException> { core.restore(byteArrayOf(1)) }.code)
            }
        }

        // ---- shared --------------------------------------------------------------------------------------

        case("shared with no core loaded is a closed placeholder: calls fail Unavailable, nothing throws on access") {
            assertEq(null, UndraCore.current)
            val shared = UndraCore.shared
            assertTrue(UndraCore.shared === shared, "the placeholder is one object")
            assertEq(null, UndraCore.current, "the placeholder never becomes the shared core")
            val sync = assertThrows<UndraTransportException> { shared.callSync(TARGET, METHOD, ARGS) }
            assertEq(UndraTransportException.Reason.CLOSED, sync.reason)
            assertTrue(sync.message!!.contains("UndraCore.load"), "the message says how to fix it: ${sync.message}")
            assertThrows<UndraTransportException> { runBlocking { shared.call(TARGET, METHOD, ARGS) } }
            assertThrows<UndraTransportException> { runBlocking { shared.stream(TARGET, METHOD, ARGS).collect {} } }
            val constructor = assertThrows<UndraCallError.Unavailable> { shared.constructObject(1u, 2u, ARGS) }
            assertEq(UndraTransportException.Reason.CLOSED, constructor.transport.reason)
            shared.release(1L)
            shared.timerFired(1u)
            shared.close()
        }

        case("a command on the placeholder only logs; registerPort is ignored with a warning") {
            LogCapture("dev.undra.runtime").use { log ->
                val shared = UndraCore.shared
                shared.report(UndraTransportException(UndraTransportException.Reason.CLOSED, "no core"), "Todos.toggle")
                assertTrue(log.records.any { it.level == Level.SEVERE && it.message.contains("Todos.toggle failed") })
                shared.registerPort(1u, PortImpl(true, emptyMap()))
                assertTrue(log.records.any { it.level == Level.WARNING && it.message.contains("registerPort") })
            }
        }

        case("after the shared core is closed, shared is the placeholder again") {
            val t = FakeTransport()
            val core = UndraCore.attach(t, LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false), makeShared = true)
            try {
                assertTrue(UndraCore.shared === core && UndraCore.current === core)
            } finally {
                core.close()
            }
            assertEq(null, UndraCore.current)
            assertThrows<UndraTransportException> { UndraCore.shared.callSync(TARGET, METHOD, ARGS) }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
