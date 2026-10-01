package dev.undra.runtime

import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.attach
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.replyPayload
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.fail
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.time.Duration.Companion.milliseconds
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private val METHOD = 0x11u
private val TARGET = CallTarget.ObjectMethod(Handle(7L), METHOD)
private val ARGS = byteArrayOf(1, 2, 3)

private fun string(s: String): ByteArray = Codecs.string.encodeToByteArray(s)

private fun panicBody(message: String, backtrace: String): ByteArray {
    val w = UndraWriter()
    w.writeStr(message)
    w.writeStr(backtrace)
    return w.toByteArray()
}

class CoreCallTests : Suite() {
    init {
        case("callSync sends a Call payload and returns the reply body") {
            val t = FakeTransport()
            t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, string("hi")) }
            attach(t).use { core ->
                assertEq("hi", Codecs.string.decodeAll(core.callSync(TARGET, METHOD, ARGS)))
                val call = t.syncCalls.single()
                assertEq(TARGET, call.target)
                assertEq(ARGS.toList(), call.args.toList())
                assertTrue(call.callId != 0u, "call id 0 is reserved")
            }
        }

        case("callSync throws UndraReplyException carrying the status and body of a non-ok reply") {
            val t = FakeTransport()
            attach(t).use { core ->
                for (status in listOf(ReplyStatus.ERROR, ReplyStatus.CANCELLED, ReplyStatus.STREAM_OPENED)) {
                    t.onCallSync = { call -> replyPayload(call.callId, status, byteArrayOf(9, 9)) }
                    val e = assertThrows<UndraReplyException> { core.callSync(TARGET, METHOD, ARGS) }
                    assertEq(status, e.status)
                    assertEq(listOf<Byte>(9, 9), e.body.toList())
                }
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.PANIC, panicBody("boom", "at x")) }
                val panic = assertThrows<UndraReplyException> { core.callSync(TARGET, METHOD, ARGS) }
                assertEq(ReplyStatus.PANIC, panic.status)
                assertEq("boom", panic.panicInfo?.message)
                assertEq("at x", panic.panicInfo?.backtrace)
                assertTrue(panic.message!!.contains("boom"), "the message names the panic: ${panic.message}")
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.BAD_REQUEST, string("unknown method")) }
                val bad = assertThrows<UndraReplyException> { core.callSync(TARGET, METHOD, ARGS) }
                assertEq("unknown method", bad.badRequestReason)
                assertTrue(bad.message!!.contains("unknown method"), bad.message!!)
            }
        }

        case("callSync reports a reply for another call, or a malformed one, as an UndraException") {
            val t = FakeTransport()
            attach(t).use { core ->
                t.onCallSync = { call -> replyPayload(call.callId + 1u, ReplyStatus.OK) }
                assertThrows<UndraException> { core.callSync(TARGET, METHOD, ARGS) }
                t.onCallSync = { byteArrayOf(1, 2) }
                val e = assertThrows<UndraException> { core.callSync(TARGET, METHOD, ARGS) }
                assertTrue(e !is UndraReplyException, "a malformed reply is not a reply status")
            }
        }

        case("the methodId argument must match the id inside the target") {
            val t = FakeTransport()
            attach(t).use { core ->
                assertThrows<IllegalArgumentException> { core.callSync(TARGET, METHOD + 1u, ARGS) }
                assertThrows<IllegalArgumentException> { core.callSync(CallTarget.FreeFunction(5u), 6u, ARGS) }
                assertThrows<IllegalArgumentException> { core.callSync(CallTarget.Constructor(1u, 2u), 3u, ARGS) }
                // A lazy-list page carries no method id, so anything goes.
                core.callSync(CallTarget.LazyListPage(Handle(1L), 0u, 10u), 0u, NO_BYTES)
                assertEq(1, t.syncCalls.size)
            }
        }

        case("every call kind reaches the transport with its own layout") {
            val t = FakeTransport()
            attach(t).use { core ->
                core.callSync(CallTarget.FreeFunction(5u), 5u, ARGS)
                core.callSync(CallTarget.Constructor(1u, 2u), 2u, ARGS)
                core.callSync(CallTarget.LazyListPage(Handle(9L), 4u, 8u), 0u, NO_BYTES)
                assertEq(listOf<CallTarget>(CallTarget.FreeFunction(5u), CallTarget.Constructor(1u, 2u), CallTarget.LazyListPage(Handle(9L), 4u, 8u)), t.syncCalls.map { it.target })
            }
        }

        case("call suspends and resumes with the reply body, off the core thread") {
            val t = FakeTransport()
            t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK, string("done")) }
            attach(t).use { core ->
                val (body, thread) = runBlocking(Dispatchers.Default) {
                    core.call(TARGET, METHOD, ARGS) to Thread.currentThread().name
                }
                assertEq("done", Codecs.string.decodeAll(body))
                assertTrue(thread != "fake-core", "resumed on the core thread: $thread")
                assertEq(1, t.calls.size)
                assertEq(0, core.stats().hostPendingCalls)
            }
        }

        case("call works when the core replies on the calling thread before the send returns") {
            val t = FakeTransport()
            t.onCall = { call -> t.events.onReply(call.callId, ReplyStatus.OK, string("inline")) }
            attach(t).use { core ->
                val body = runBlocking { core.call(TARGET, METHOD, ARGS) }
                assertEq(string("inline").toList(), body.toList())
                // The same for a coroutine on a dispatcher that would run the resumption inline.
                val again = runBlocking(Dispatchers.Unconfined) { core.call(TARGET, METHOD, ARGS) }
                assertEq(string("inline").toList(), again.toList())
            }
        }

        case("a non-ok reply makes call throw UndraReplyException") {
            val t = FakeTransport()
            t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.ERROR, byteArrayOf(7)) }
            attach(t).use { core ->
                val e = assertThrows<UndraReplyException> { runBlocking { core.call(TARGET, METHOD, ARGS) } }
                assertEq(ReplyStatus.ERROR, e.status)
                assertEq(listOf<Byte>(7), e.body.toList())
            }
        }

        case("cancelling a call sends Cancel once and ignores the late reply") {
            val t = FakeTransport()
            attach(t).use { core ->
                runBlocking {
                    val job = launch(Dispatchers.Default) { core.call(TARGET, METHOD, ARGS) }
                    eventually("the call reaches the transport") { t.calls.isNotEmpty() }
                    job.cancelAndJoin()
                    assertEq(listOf(t.calls.single().callId), t.cancels.toList())
                    assertEq(0, core.stats().hostPendingCalls)
                    // The core answers anyway (or with Cancelled): nothing must blow up or be sent again.
                    t.replyOnCore(t.calls.single().callId, ReplyStatus.CANCELLED)
                    t.awaitCore()
                    assertEq(1, t.cancels.size)
                }
            }
        }

        case("a call that already finished is not cancelled") {
            val t = FakeTransport()
            t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK) }
            attach(t).use { core ->
                runBlocking {
                    val d = async(Dispatchers.Default) { core.call(TARGET, METHOD, ARGS) }
                    d.await()
                    d.cancel()
                }
                assertEq(0, t.cancels.size)
            }
        }

        case("a call on an already cancelled coroutine never reaches the core") {
            val t = FakeTransport()
            attach(t).use { core ->
                runBlocking {
                    val job = launch(start = CoroutineStart.LAZY) { core.call(TARGET, METHOD, ARGS) }
                    job.cancel()
                    job.join()
                }
                assertEq(0, t.calls.size)
                assertEq(0, core.stats().hostPendingCalls)
            }
        }

        case("a continuation that would run inline is resumed off the core thread, so no application code runs there") {
            val t = FakeTransport()
            // The reply comes after the caller has suspended (a reply that beat the suspension would simply not suspend it).
            t.onCall = { call -> t.onCore { Thread.sleep(150); t.events.onReply(call.callId, ReplyStatus.OK, string("x")) } }
            attach(t).use { core ->
                val thread = runBlocking(Dispatchers.Unconfined) {
                    core.call(TARGET, METHOD, ARGS)
                    Thread.currentThread().name
                }
                assertTrue(thread != "fake-core", "resumed on the core thread")
                assertEq("undra-delivery", thread)
            }
        }

        case("call ids are non-zero and unique across concurrent calls") {
            val t = FakeTransport()
            t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK) }
            attach(t).use { core ->
                runBlocking {
                    val jobs = List(200) { async(Dispatchers.Default) { core.call(TARGET, METHOD, ARGS) } }
                    jobs.forEach { it.await() }
                }
                val ids = t.calls.map { it.callId }
                assertEq(200, ids.toSet().size)
                assertTrue(ids.none { it == 0u }, "call id 0 is reserved")
            }
        }

        case("call ids wrap around and skip zero") {
            val up = FakeTransport()
            val nearMax = ConnectedCore(up, 5_000.milliseconds, initialCallId = Int.MAX_VALUE - 1)
            up.connect(nearMax, HASH)
            up.onCall = { call -> up.events.onReply(call.callId, ReplyStatus.OK, NO_BYTES) }
            runBlocking { repeat(4) { nearMax.call(TARGET, METHOD, ARGS) } }
            assertEq(listOf(Int.MAX_VALUE.toUInt(), 0x80000000u, 0x80000001u, 0x80000002u), up.calls.map { it.callId })
            nearMax.close()

            val down = FakeTransport()
            val nearZero = ConnectedCore(down, 5_000.milliseconds, initialCallId = -2)
            down.connect(nearZero, HASH)
            down.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK) }
            repeat(3) { nearZero.callSync(TARGET, METHOD, ARGS) }
            assertEq(listOf(UInt.MAX_VALUE, 1u, 2u), down.syncCalls.map { it.callId })
            nearZero.close()
        }

        case("a transport that rejects the payload fails the call as a bad request") {
            val t = FakeTransport()
            t.callResult = 5
            attach(t).use { core ->
                val e = assertThrows<UndraReplyException> { runBlocking { core.call(TARGET, METHOD, ARGS) } }
                assertEq(ReplyStatus.BAD_REQUEST, e.status)
                assertEq(0, core.stats().hostPendingCalls)
            }
        }

        case("a transport that fails to send fails the call and leaves nothing pending") {
            val t = FakeTransport()
            t.callFailure = UndraException("link down")
            attach(t).use { core ->
                val e = assertThrows<UndraException> { runBlocking { core.call(TARGET, METHOD, ARGS) } }
                assertEq("link down", e.message)
                assertEq(0, core.stats().hostPendingCalls)
            }
        }

        case("construct returns the handle from the reply and rejects a null or malformed one") {
            val t = FakeTransport()
            attach(t).use { core ->
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, Codecs.handle.encodeToByteArray(0x100000005L)) }
                assertEq(0x100000005L, core.construct(3u, 4u, ARGS))
                assertEq(CallTarget.Constructor(3u, 4u), t.syncCalls.single().target)
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, Codecs.handle.encodeToByteArray(0L)) }
                assertThrows<UndraException> { core.construct(3u, 4u, ARGS) }
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, byteArrayOf(1)) }
                assertThrows<UndraException> { core.construct(3u, 4u, ARGS) }
                t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.ERROR, byteArrayOf(2)) }
                assertEq(ReplyStatus.ERROR, assertThrows<UndraReplyException> { core.construct(3u, 4u, ARGS) }.status)
            }
        }

        case("release, event and timerFired reach the transport with their arguments") {
            val t = FakeTransport()
            attach(t).use { core ->
                core.release(0x100000005L)
                core.event(1u, 2u, byteArrayOf(3))
                core.timerFired(77u)
                assertEq(listOf(0x100000005L), t.releases.toList())
                assertEq(Payloads.Event(1u, 2u, byteArrayOf(3)), t.sentEvents.single())
                assertEq(listOf(77u), t.timers.toList())
            }
        }

        case("close fails pending calls, refuses new work, clears shared and is idempotent") {
            val t = FakeTransport()
            val core = attach(t)
            val failure = CopyOnWriteArrayList<Throwable>()
            val worker = Thread {
                try {
                    runBlocking { core.call(TARGET, METHOD, ARGS) }
                } catch (e: Throwable) {
                    failure.add(e)
                }
            }
            worker.start()
            eventually("the call is pending") { t.calls.isNotEmpty() }
            core.close()
            worker.join(10_000)
            assertEq(1, failure.size)
            assertTrue(failure.single() is UndraException, "pending call failed with ${failure.single()}")
            assertTrue(t.closed, "the transport was closed")
            assertThrows<UndraException>("call after close") { runBlocking { core.call(TARGET, METHOD, ARGS) } }
            assertThrows<UndraException>("callSync after close") { core.callSync(TARGET, METHOD, ARGS) }
            assertThrows<UndraException>("observe after close") { core.observe(1L, 0u, true) }
            core.close() // twice is fine
            core.release(1L) // and releasing after close is a quiet no-op
            core.timerFired(3u) // as is a timer that comes due late
            assertEq(0, t.timers.size)
        }

        case("a core that was never loaded refuses shared, and a base UndraCore is an inert test double") {
            // (Other tests attach without making a core shared, so nothing is shared here unless a load succeeded.)
            val double = object : UndraCore() {}
            assertThrows<UnsupportedOperationException> { double.callSync(TARGET, METHOD, ARGS) }
            assertThrows<UnsupportedOperationException> { runBlocking { double.call(TARGET, METHOD, ARGS) } }
            assertThrows<UnsupportedOperationException> { double.stream(TARGET, METHOD, ARGS) }
            assertThrows<UnsupportedOperationException> { double.construct(1u, 2u, ARGS) }
            assertThrows<UnsupportedOperationException> { double.observe(1L, 0u, true) }
            assertThrows<UnsupportedOperationException> { double.release(1L) }
            assertThrows<UnsupportedOperationException> { double.event(1u, 2u, ARGS) }
            assertThrows<UnsupportedOperationException> { double.registerPort(1u, PortImpl(true, emptyMap())) }
            assertThrows<UnsupportedOperationException> { double.snapshot() }
            assertThrows<UnsupportedOperationException> { double.timerFired(1u) }
            assertThrows<UnsupportedOperationException> { double.mode }
            assertEq(UndraStats.UNKNOWN, double.stats().liveHandles)
            double.mirror.register(1L) { _, _, _ -> }
            double.close()
        }

        case("callSync over a transport without a synchronous path waits for the reply") {
            val t = FakeTransport(isSynchronous = false)
            t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK, string("later")) }
            attach(t).use { core ->
                assertEq(Mode.REMOTE, core.mode)
                assertEq(string("later").toList(), core.callSync(TARGET, METHOD, ARGS).toList())
                t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.ERROR, byteArrayOf(1)) }
                assertEq(ReplyStatus.ERROR, assertThrows<UndraReplyException> { core.callSync(TARGET, METHOD, ARGS) }.status)
                t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK, Codecs.handle.encodeToByteArray(42L)) }
                assertEq(42L, core.construct(1u, 2u, ARGS))
                assertEq(0, t.syncCalls.size)
            }
        }

        case("a blocking wait that times out fails, cancels the call and leaves nothing pending") {
            val t = FakeTransport(isSynchronous = false)
            attach(t, timeout = 150.milliseconds).use { core ->
                val e = assertThrows<UndraException> { core.callSync(TARGET, METHOD, ARGS) }
                assertTrue(e.message!!.contains("did not answer"), e.message!!)
                assertEq(listOf(t.calls.single().callId), t.cancels.toList())
                assertEq(0, core.stats().hostPendingCalls)
            }
        }

        case("a blocking wait that is interrupted restores the flag and cancels") {
            val t = FakeTransport(isSynchronous = false)
            attach(t).use { core ->
                val result = CopyOnWriteArrayList<Any>()
                val worker = Thread {
                    try {
                        core.callSync(TARGET, METHOD, ARGS)
                    } catch (e: UndraException) {
                        result.add(e)
                        result.add(Thread.currentThread().isInterrupted)
                    }
                }
                worker.start()
                eventually("the call is sent") { t.calls.isNotEmpty() }
                worker.interrupt()
                worker.join(10_000)
                assertEq(2, result.size)
                assertEq(true, result[1], "the interrupt flag is restored")
                assertEq(1, t.cancels.size)
            }
        }

        case("a blocking wait fails when the core closes under it") {
            val t = FakeTransport(isSynchronous = false)
            val core = attach(t)
            val result = CopyOnWriteArrayList<Throwable>()
            val worker = Thread {
                try {
                    core.callSync(TARGET, METHOD, ARGS)
                } catch (e: Throwable) {
                    result.add(e)
                }
            }
            worker.start()
            eventually("the call is sent") { t.calls.isNotEmpty() }
            core.close()
            worker.join(10_000)
            assertTrue(result.single() is UndraException, "got ${result.single()}")
        }

        case("a rejected payload fails a blocking call as a bad request") {
            val t = FakeTransport(isSynchronous = false)
            t.callResult = 5
            attach(t).use { core ->
                assertEq(ReplyStatus.BAD_REQUEST, assertThrows<UndraReplyException> { core.callSync(TARGET, METHOD, ARGS) }.status)
            }
        }

        case("snapshot and restore pass through; a refused restore is an UndraException; a transport without them is an UndraModeException") {
            val t = FakeTransport()
            t.snapshotBytes = byteArrayOf(5, 6)
            attach(t).use { core ->
                assertEq(listOf<Byte>(5, 6), core.snapshot().toList())
                core.restore(byteArrayOf(1))
                t.restoreResult = 3
                assertTrue(assertThrows<UndraException> { core.restore(byteArrayOf(1)) }.message!!.contains("3"))
            }
            val remote = FakeTransport(isSynchronous = false)
            attach(remote).use { core ->
                assertThrows<UndraModeException> { core.snapshot() }
                assertThrows<UndraModeException> { core.restore(byteArrayOf(1)) }
            }
        }

        case("stats combines the core's document with the host's counters; a transport that cannot ask reports unknown") {
            val t = FakeTransport()
            t.statsJson = "{\"live_handles\":5,\"live_stores\":2,\"tasks\":1,\"active_calls\":3,\"open_streams\":1," +
                "\"pending_port_calls\":4,\"pending_timers\":6,\"transactions\":99,\"panics\":7,\"crossings\":{\"calls\":12}}"
            attach(t).use { core ->
                core.mirror.register(1L) { _, _, _ -> }
                val s = core.stats()
                assertEq(5, s.liveHandles)
                assertEq(2, s.liveStores)
                assertEq(1, s.tasks)
                assertEq(3, s.activeCalls)
                assertEq(1, s.openStreams)
                assertEq(4, s.pendingPortCalls)
                assertEq(6, s.pendingTimers)
                assertEq(99L, s.transactions)
                assertEq(7L, s.panics)
                assertEq(1, s.hostMirrorHandles)
                assertEq(0, s.hostPendingCalls)
                assertEq(t.statsJson, s.raw)
            }
            val remote = FakeTransport(isSynchronous = false)
            attach(remote).use { core ->
                val s = core.stats()
                assertEq(UndraStats.UNKNOWN, s.liveHandles)
                assertEq("{}", s.raw)
            }
        }

        case("attach reports a schema mismatch, closes the transport and never becomes shared") {
            val t = FakeTransport(schemaHash = 0x1234uL)
            val e = assertThrows<UndraSchemaMismatchException> {
                UndraCore.attach(t, LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false), makeShared = true)
            }
            assertEq(HASH, e.expected)
            assertEq(0x1234uL, e.got)
            assertTrue(e.message!!.contains("0x691eee0733e4a44f") && e.message!!.contains("0x1234"), e.message!!)
            assertTrue(t.closed, "the transport was closed")
            assertEq(null, UndraCore.current, "shared stays unset")
        }

        case("attach wraps a transport failure and closes the transport") {
            val t = FakeTransport()
            t.connectFailure = IllegalStateException("kaput")
            val e = assertThrows<UndraException> { UndraCore.attach(t, LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false), makeShared = false) }
            assertTrue(e.message!!.contains("kaput"), e.message!!)
            assertTrue(e.cause is IllegalStateException)
            assertTrue(t.closed)
        }

        case("load rejects contradictory options with an UndraModeException") {
            val remoteNoUrl = assertThrows<UndraModeException> { UndraCore.load(LoadOptions(mode = Mode.REMOTE, expectedSchemaHash = HASH)) }
            assertTrue(remoteNoUrl.message!!.contains("remoteUrl"))
            assertThrows<UndraModeException> { UndraCore.load(LoadOptions(mode = Mode.INPROC, remoteUrl = "ws://localhost:1", expectedSchemaHash = HASH)) }
            assertThrows<UndraModeException> { UndraCore.load(LoadOptions(mode = Mode.REMOTE, remoteUrl = "http://localhost:1", expectedSchemaHash = HASH)) }
            assertThrows<UndraModeException> { UndraCore.load(LoadOptions(mode = Mode.REMOTE, remoteUrl = "not a url", expectedSchemaHash = HASH)) }
        }

        case("LoadOptions has the documented defaults") {
            val o = LoadOptions(expectedSchemaHash = 5uL)
            assertEq(Mode.INPROC, o.mode)
            assertEq(null, o.remoteUrl)
            assertEq(emptyMap<UInt, PortImpl>(), o.adapters)
            assertEq(true, o.defaultAdapters)
            assertEq(30_000L, o.remoteTimeout.inWholeMilliseconds)
            assertTrue(o.toString().contains("0x5"))
        }

        case("several concurrent callers get their own replies") {
            val t = FakeTransport()
            t.onCall = { call ->
                // Reply with the call's own first argument byte, from the core thread, in reverse-ish order.
                t.replyOnCore(call.callId, ReplyStatus.OK, call.args)
            }
            attach(t).use { core ->
                runBlocking {
                    val results = List(100) { i -> async(Dispatchers.Default) { core.call(TARGET, METHOD, byteArrayOf(i.toByte())) } }
                    results.forEachIndexed { i, d -> assertEq(listOf(i.toByte()), d.await().toList()) }
                }
            }
        }

        case("callSync is safe to call from many threads at once") {
            val t = FakeTransport()
            t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, call.args) }
            attach(t).use { core ->
                runBlocking {
                    val results = List(64) { i -> async(Dispatchers.IO) { core.callSync(TARGET, METHOD, byteArrayOf(i.toByte())) } }
                    results.forEachIndexed { i, d -> assertEq(listOf(i.toByte()), d.await().toList()) }
                }
                assertEq(64, t.syncCalls.map { it.callId }.toSet().size)
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
