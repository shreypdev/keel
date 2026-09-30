package dev.keel.runtime

import dev.keel.runtime.support.FakeTransport
import dev.keel.runtime.support.NO_BYTES
import dev.keel.runtime.support.attach
import dev.keel.runtime.support.eventually
import dev.keel.runtime.testing.Suite
import dev.keel.runtime.testing.assertEq
import dev.keel.runtime.testing.assertThrows
import dev.keel.runtime.testing.assertTrue
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.Handle
import dev.keel.runtime.wire.Payloads
import dev.keel.runtime.wire.Payloads.CallTarget
import dev.keel.runtime.wire.Payloads.ReplyStatus
import dev.keel.runtime.wire.decodeAll
import dev.keel.runtime.wire.encodeToByteArray
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private val METHOD = 0x21u
private val TARGET = CallTarget.ObjectMethod(Handle(3L), METHOD)

private fun item(i: Int): ByteArray = Codecs.u32.encodeToByteArray(i.toUInt())

private fun decode(bytes: ByteArray): Int = Codecs.u32.decodeAll(bytes).toInt()

class StreamTests : Suite() {
    init {
        case("items arrive in order and the flow completes when the core ends the stream") {
            val t = FakeTransport()
            t.onCall = { call -> t.serveStream(call, List(5) { item(it) }) }
            attach(t).use { core ->
                val items = runBlocking { core.stream(TARGET, METHOD, byteArrayOf(9)).map { decode(it) }.toList() }
                assertEq(listOf(0, 1, 2, 3, 4), items)
                assertEq(listOf<Byte>(9), t.calls.single().args.toList())
                assertEq(0, core.stats().hostPendingCalls)
                assertEq(0, t.cancels.size, "a stream that ended is not cancelled")
            }
        }

        case("an empty stream ends at once") {
            val t = FakeTransport()
            t.onCall = { call -> t.serveStream(call, emptyList()) }
            attach(t).use { core ->
                assertEq(emptyList<Int>(), runBlocking { core.stream(TARGET, METHOD, NO_BYTES).map { decode(it) }.toList() })
                assertEq(0, t.cancels.size)
            }
        }

        case("credit: 16 granted after the stream opens, then 8 more each time fewer than 8 remain") {
            val t = FakeTransport()
            var producer: FakeTransport.StreamProducer? = null
            t.onCall = { call -> producer = t.serveStream(call, List(40) { item(it) }) }
            attach(t).use { core ->
                val creditsWhenItemArrived = ArrayList<Int>()
                val items = runBlocking {
                    val out = ArrayList<Int>()
                    core.stream(TARGET, METHOD, NO_BYTES).collect {
                        creditsWhenItemArrived.add(t.credits.size)
                        out.add(decode(it))
                    }
                    out
                }
                assertEq((0 until 40).toList(), items)
                // 16 up front; then after the 9th, 17th, 25th and 33rd item the outstanding credit dropped below 8.
                assertEq(listOf(16u, 8u, 8u, 8u, 8u), t.credits.map { it.second })
                // ...and each top-up went out at exactly that moment: items 1-9 saw one grant, 10-17 two, and so on.
                val expected = (1..40).map { n -> if (n <= 9) 1 else if (n <= 17) 2 else if (n <= 25) 3 else if (n <= 33) 4 else 5 }
                assertEq(expected, creditsWhenItemArrived)
                assertTrue(t.credits.all { it.first == t.calls.single().callId })
                assertEq(40, producer!!.sent)
            }
        }

        case("credit is only granted once the core has said the stream is open") {
            val t = FakeTransport()
            attach(t).use { core ->
                runBlocking {
                    val job = launch(Dispatchers.Default) { core.stream(TARGET, METHOD, NO_BYTES).toList() }
                    eventually("the call is sent") { t.calls.isNotEmpty() }
                    Thread.sleep(100)
                    assertEq(0, t.credits.size, "no credit before STREAM_OPENED")
                    t.replyOnCore(t.calls.single().callId, ReplyStatus.STREAM_OPENED)
                    eventually("credit is granted after the stream opened") { t.credits.size == 1 }
                    assertEq(16u, t.credits.single().second)
                    job.cancelAndJoin()
                }
            }
        }

        case("back-pressure: a collector that stops consuming stops the core after the credit it was given") {
            val t = FakeTransport()
            var producer: FakeTransport.StreamProducer? = null
            t.onCall = { call -> producer = t.serveStream(call, List(100) { item(it) }) }
            attach(t).use { core ->
                runBlocking {
                    val gate = CompletableDeferred<Unit>()
                    val seen = java.util.concurrent.atomic.AtomicInteger()
                    val job = launch(Dispatchers.Default) {
                        core.stream(TARGET, METHOD, NO_BYTES).collect {
                            seen.incrementAndGet()
                            gate.await() // a collector stuck on its first item
                        }
                    }
                    eventually("the first item is consumed") { seen.get() == 1 }
                    Thread.sleep(150)
                    assertEq(16, producer!!.sent, "the core sent exactly the credit it was granted")
                    assertEq(listOf(16u), t.credits.map { it.second })
                    gate.complete(Unit)
                    eventually("more credit follows consumption") { t.credits.size > 1 }
                    job.cancelAndJoin()
                }
            }
        }

        case("a failing stream delivers its items, then throws KeelReplyException with the error body") {
            val t = FakeTransport()
            t.onCall = { call -> t.serveStream(call, List(3) { item(it) }, failure = byteArrayOf(4, 5)) }
            attach(t).use { core ->
                val seen = ArrayList<Int>()
                val e = assertThrows<KeelReplyException> {
                    runBlocking { core.stream(TARGET, METHOD, NO_BYTES).collect { seen.add(decode(it)) } }
                }
                assertEq(listOf(0, 1, 2), seen)
                assertEq(ReplyStatus.ERROR, e.status)
                assertEq(listOf<Byte>(4, 5), e.body.toList())
                assertEq(0, core.stats().hostPendingCalls)
                assertEq(0, t.cancels.size)
            }
        }

        case("a reply other than 'opened' fails the stream before any item") {
            val t = FakeTransport()
            t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.BAD_REQUEST, byteArrayOf(1)) }
            attach(t).use { core ->
                val e = assertThrows<KeelReplyException> { runBlocking { core.stream(TARGET, METHOD, NO_BYTES).toList() } }
                assertEq(ReplyStatus.BAD_REQUEST, e.status)
                assertEq(0, t.credits.size)
                assertEq(0, t.cancels.size)
                assertEq(0, core.stats().hostPendingCalls)
            }
        }

        case("a method that answers with a value where a stream was expected is a KeelException") {
            val t = FakeTransport()
            t.onCall = { call -> t.replyOnCore(call.callId, ReplyStatus.OK, byteArrayOf(1)) }
            attach(t).use { core ->
                assertThrows<KeelException> { runBlocking { core.stream(TARGET, METHOD, NO_BYTES).toList() } }
            }
        }

        case("cancelling the collector cancels the stream in the core once, and late items are ignored") {
            val t = FakeTransport()
            var producer: FakeTransport.StreamProducer? = null
            t.onCall = { call -> producer = t.serveStream(call, List(1000) { item(it) }) }
            attach(t).use { core ->
                runBlocking {
                    // Park the collector after the first item so the stream cannot run to completion
                    // before the cancellation lands (on a fast machine 1000 items drain in under a ms).
                    val firstItem = CompletableDeferred<Unit>()
                    val parked = CompletableDeferred<Unit>() // never completed; collect suspends here
                    val job = launch(Dispatchers.Default) {
                        core.stream(TARGET, METHOD, NO_BYTES).collect {
                            firstItem.complete(Unit)
                            parked.await()
                        }
                    }
                    firstItem.await()
                    job.cancelAndJoin()
                    assertEq(listOf(t.calls.single().callId), t.cancels.toList())
                    assertEq(0, core.stats().hostPendingCalls)
                    t.itemOnCore(t.calls.single().callId, item(1)) // late
                    t.endOnCore(t.calls.single().callId)
                    t.awaitCore()
                    assertEq(1, t.cancels.size)
                    assertTrue(producer!!.sent <= 1000)
                }
            }
        }

        case("taking a few items and stopping cancels the stream") {
            val t = FakeTransport()
            t.onCall = { call -> t.serveStream(call, List(1000) { item(it) }) }
            attach(t).use { core ->
                val items = runBlocking { core.stream(TARGET, METHOD, NO_BYTES).map { decode(it) }.take(3).toList() }
                assertEq(listOf(0, 1, 2), items)
                assertEq(1, t.cancels.size)
            }
        }

        case("every collection starts its own call") {
            val t = FakeTransport()
            t.onCall = { call -> t.serveStream(call, List(2) { item(it) }) }
            attach(t).use { core ->
                val flow = core.stream(TARGET, METHOD, NO_BYTES)
                assertEq(0, t.calls.size, "the flow is cold")
                runBlocking {
                    assertEq(2, flow.toList().size)
                    assertEq(2, flow.toList().size)
                }
                assertEq(2, t.calls.size)
                assertTrue(t.calls[0].callId != t.calls[1].callId)
            }
        }

        case("a collector on Dispatchers.Unconfined is never run on the core thread") {
            val t = FakeTransport()
            t.onCall = { call -> t.serveStream(call, List(4) { item(it) }) }
            attach(t).use { core ->
                val threads = runBlocking(Dispatchers.Unconfined) {
                    val names = ArrayList<String>()
                    core.stream(TARGET, METHOD, NO_BYTES).collect { names.add(Thread.currentThread().name) }
                    names
                }
                assertEq(4, threads.size)
                assertTrue(threads.none { it == "fake-core" }, "items were handled on the core thread: $threads")
            }
        }

        case("closing the core ends open streams with a KeelException") {
            val t = FakeTransport()
            val core = attach(t)
            runBlocking {
                val failure = CompletableDeferred<Throwable>()
                val job = launch(Dispatchers.Default) {
                    try {
                        core.stream(TARGET, METHOD, NO_BYTES).toList()
                    } catch (e: Throwable) {
                        failure.complete(e)
                    }
                }
                eventually("the stream call is sent") { t.calls.isNotEmpty() }
                t.replyOnCore(t.calls.single().callId, ReplyStatus.STREAM_OPENED)
                eventually("credit is granted") { t.credits.isNotEmpty() }
                core.close()
                val e = failure.await()
                assertTrue(e is KeelException, "got $e")
                job.join()
            }
        }

        case("a stream on a closed core fails when collected") {
            val t = FakeTransport()
            val core = attach(t)
            core.close()
            assertThrows<KeelException> { runBlocking { core.stream(TARGET, METHOD, NO_BYTES).toList() } }
        }

        case("a rejected or unsent stream call does not leak a pending entry") {
            val t = FakeTransport()
            t.callResult = 5
            attach(t).use { core ->
                assertEq(ReplyStatus.BAD_REQUEST, assertThrows<KeelReplyException> { runBlocking { core.stream(TARGET, METHOD, NO_BYTES).toList() } }.status)
                assertEq(0, core.stats().hostPendingCalls)
                t.callResult = 0
                t.callFailure = KeelException("down")
                assertThrows<KeelException> { runBlocking { core.stream(TARGET, METHOD, NO_BYTES).toList() } }
                assertEq(0, core.stats().hostPendingCalls)
            }
        }

        case("a stream item body is passed through untouched, including empty and large ones") {
            val t = FakeTransport()
            val big = ByteArray(300_000) { (it % 251).toByte() }
            t.onCall = { call -> t.serveStream(call, listOf(NO_BYTES, big, byteArrayOf(0))) }
            attach(t).use { core ->
                val items = runBlocking { core.stream(TARGET, METHOD, NO_BYTES).toList() }
                assertEq(3, items.size)
                assertEq(0, items[0].size)
                assertTrue(items[1].contentEquals(big))
                assertEq(listOf<Byte>(0), items[2].toList())
            }
        }

        case("the reply codes of the wire payloads used here are the documented ones") {
            assertEq(4.toUByte(), ReplyStatus.STREAM_OPENED.code)
            assertEq(0.toUByte(), Payloads.StreamFlag.ITEM.code)
            assertEq(1.toUByte(), Payloads.StreamFlag.END.code)
            assertEq(2.toUByte(), Payloads.StreamFlag.ERROR.code)
        }
    }

    @Test
    fun allCases() = assertPassed()
}
