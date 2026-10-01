package dev.undra.runtime

import dev.undra.runtime.support.FakeNative
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.changeSet
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.full
import dev.undra.runtime.support.handledBy
import dev.undra.runtime.support.portMethods
import dev.undra.runtime.support.replyPayload
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.PortStatus
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.Payloads.StreamFlag
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private val METHOD = 0x31u
private val TARGET = CallTarget.ObjectMethod(Handle(0x100000002L), METHOD)
private val PORT_FOR_CLOSE = 0x77u

/** Everything a core cares about, recorded, with hooks a test can set. */
private class RecordingEvents : TransportEvents {
    val replies = CopyOnWriteArrayList<Triple<UInt, ReplyStatus, List<Byte>>>()
    val items = CopyOnWriteArrayList<Triple<UInt, StreamFlag, List<Byte>>>()
    val changeSets = CopyOnWriteArrayList<ByteArray>()
    val portCalls = CopyOnWriteArrayList<List<Any>>()
    val logs = CopyOnWriteArrayList<String>()
    @Volatile var closed: Boolean = false
    @Volatile var portAnswer: PortOutcome = PortOutcome.Unavailable
    @Volatile var failWith: RuntimeException? = null
    @Volatile var onPort: () -> Unit = {}

    override fun onReply(callId: UInt, status: ReplyStatus, body: ByteArray) {
        failWith?.let { throw it }
        replies.add(Triple(callId, status, body.toList()))
    }

    override fun onStreamItem(callId: UInt, flag: StreamFlag, body: ByteArray) {
        failWith?.let { throw it }
        items.add(Triple(callId, flag, body.toList()))
    }

    val malformed = CopyOnWriteArrayList<Pair<UInt, UndraProtocolException>>()

    override fun onMalformed(callId: UInt, error: UndraProtocolException) {
        failWith?.let { throw it }
        malformed.add(callId to error)
    }

    override fun onChangeSet(changeSet: ByteArray) {
        failWith?.let { throw it }
        changeSets.add(changeSet)
    }

    override fun onPortCall(portId: UInt, methodId: UInt, portCallId: UInt, args: ByteArray): PortOutcome {
        failWith?.let { throw it }
        portCalls.add(listOf(portId, methodId, portCallId, args.toList()))
        onPort()
        return portAnswer
    }

    override fun onLog(level: UByte, target: String, message: String) {
        logs.add("$level $target $message")
    }

    override fun onClosed(cause: Throwable?) {
        closed = true
    }
}

class InprocTransportTests : Suite() {
    init {
        case("connect checks the ABI and the schema hash, then initializes the core with an encoded RuntimeConfig") {
            val native = FakeNative()
            val transport = InprocTransport(native)
            val got = transport.connect(RecordingEvents(), HASH)
            assertEq(HASH, got)
            assertEq(1, native.inits.get())
            // RuntimeConfig: platform String, mode String, core_threads u8, blocking_threads u8, log_level u8.
            val r = UndraReader(native.config!!)
            assertEq("jvm", r.readStr())
            assertEq("inproc", r.readStr())
            assertEq(1.toUByte(), r.readU8())
            assertEq(0.toUByte(), r.readU8())
            assertEq(2.toUByte(), r.readU8())
            r.finish()
            assertEq(Mode.INPROC, transport.mode)
            assertTrue(transport.isSynchronous)
        }

        case("a core built from another schema is reported and never initialized") {
            val native = FakeNative()
            native.hash = 0x1111L
            val got = InprocTransport(native).connect(RecordingEvents(), HASH)
            assertEq(0x1111uL, got)
            assertEq(0, native.inits.get())
            // ...and through UndraCore.attach it becomes the typed exception.
            val e = assertThrows<UndraSchemaMismatchException> {
                UndraCore.attach(InprocTransport(native), LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false), makeShared = false)
            }
            assertEq(0x1111uL, e.got)
            assertEq(0, native.inits.get())
        }

        case("a hash with the top bit set survives the signed JNI long") {
            val native = FakeNative()
            native.hash = -0x1L // 0xFFFFFFFFFFFFFFFF
            val got = InprocTransport(native).connect(RecordingEvents(), ULong.MAX_VALUE)
            assertEq(ULong.MAX_VALUE, got)
            assertEq(1, native.inits.get())
        }

        case("an unavailable native library is an UndraException that names the core and says how to fix it") {
            val native = FakeNative("acme_pay")
            native.isAvailable = false
            native.unavailableReason = UnsatisfiedLinkError("no acme_pay in java.library.path")
            val e = assertThrows<UndraException> { InprocTransport(native).connect(RecordingEvents(), HASH) }
            assertTrue(e.cause is UnsatisfiedLinkError)
            for (hint in listOf("`acme_pay`", "libacme_pay", "java.library.path", "jniLibs/<abi>/libacme_pay.so", "-Dundra.native.acme_pay.path=", "Mode.REMOTE")) {
                assertTrue(e.message!!.contains(hint), "the message should mention $hint: ${e.message}")
            }
            assertEq(0, native.inits.get())
        }

        case("the runtime speaks ABI version 2 (ADR-044's table); a version-1 core and a newer one are refused before init") {
            for (abi in listOf(1, 3)) {
                val native = FakeNative()
                native.abi = abi
                val e = assertThrows<UndraException> { InprocTransport(native).connect(RecordingEvents(), HASH) }
                assertTrue(e.message!!.contains("ABI version $abi") && e.message!!.contains("speaks 2"), e.message!!)
                assertEq(0, native.inits.get())
            }
            val v2 = FakeNative()
            InprocTransport(v2).use { it.connect(RecordingEvents(), HASH) }
            assertEq(1, v2.inits.get())
        }

        case("two cores with different namespaces run side by side; each close ends only its own core") {
            val a = FakeNative("side_a")
            val b = FakeNative("side_b")
            val ta = InprocTransport(a)
            val tb = InprocTransport(b)
            val ea = RecordingEvents()
            val eb = RecordingEvents()
            ta.connect(ea, HASH)
            tb.connect(eb, HASH)
            assertEq(1, a.inits.get())
            assertEq(1, b.inits.get())
            // Each core's callbacks reach its own transport only.
            a.emitChangeSet(changeSet(1uL, full(0x100000002L, 1u, byteArrayOf(1))))
            b.emitChangeSet(changeSet(1uL, full(0x100000002L, 1u, byteArrayOf(2))))
            b.emitChangeSet(changeSet(2uL, full(0x100000002L, 1u, byteArrayOf(3))))
            assertEq(1, ea.changeSets.size)
            assertEq(2, eb.changeSets.size)
            ta.close()
            assertEq(1, a.shutdowns.get())
            assertEq(0, b.shutdowns.get(), "closing one core leaves the other running")
            // The closed namespace is free again; the other one is still claimed.
            InprocTransport(FakeNative("side_a")).use { it.connect(RecordingEvents(), HASH) }
            val busy = assertThrows<UndraException> { InprocTransport(FakeNative("side_b")).connect(RecordingEvents(), HASH) }
            assertTrue(busy.message!!.contains("`side_b` is already loaded"), busy.message!!)
            tb.close()
            assertEq(1, b.shutdowns.get())
        }

        case("a second core with the same namespace is refused, even over another NativeApi object, and never initialized") {
            val first = FakeNative("same_ns")
            val second = FakeNative("same_ns")
            val t1 = InprocTransport(first)
            t1.connect(RecordingEvents(), HASH)
            val refused = InprocTransport(second)
            val e = assertThrows<UndraException> { refused.connect(RecordingEvents(), HASH) }
            assertTrue(e.message!!.contains("`same_ns` is already loaded"), e.message!!)
            assertEq(0, second.inits.get(), "the refused core never starts")
            // The refused transport owns nothing: closing it neither shuts the loaded core down nor frees its claim.
            refused.close()
            assertEq(0, first.shutdowns.get())
            assertEq(0, second.shutdowns.get())
            assertThrows<UndraException> { InprocTransport(second).connect(RecordingEvents(), HASH) }
            t1.close()
            InprocTransport(second).use { it.connect(RecordingEvents(), HASH) }
            assertEq(1, second.inits.get(), "after the first one closed, the namespace loads again")
        }

        case("a failing init is reported and a retry is allowed; a second core after a success is refused") {
            val native = FakeNative()
            native.initResult = 7
            val e = assertThrows<UndraException> { InprocTransport(native).connect(RecordingEvents(), HASH) }
            assertTrue(e.message!!.contains("7"), e.message!!)
            native.initResult = 0
            InprocTransport(native).connect(RecordingEvents(), HASH) // the failed attempt did not claim the native runtime
            val again = assertThrows<UndraException> { InprocTransport(native).connect(RecordingEvents(), HASH) }
            assertTrue(again.message!!.contains("already loaded"), again.message!!)
            assertEq(2, native.inits.get())
        }

        case("host-to-core calls map ids to the JNI's signed ints") {
            val native = FakeNative()
            val transport = InprocTransport(native)
            transport.connect(RecordingEvents(), HASH)
            transport.cancel(0xFFFFFFFEu)
            transport.streamCredit(5u, 16u)
            transport.observe(0x100000002L, UInt.MAX_VALUE, true)
            transport.release(0x100000002L)
            transport.event(0x80000000u, 3u, byteArrayOf(1))
            transport.timerFired(9u)
            transport.portReply(byteArrayOf(4, 4))
            assertEq(listOf(-2), native.cancels.toList())
            assertEq(listOf(5 to 16), native.credits.toList())
            assertEq(listOf(Triple(0x100000002L, -1, true)), native.observes.toList())
            assertEq(listOf(0x100000002L), native.releases.toList())
            assertEq(listOf(Int.MIN_VALUE), native.events.map { it.first })
            assertEq(listOf(9), native.timers.toList())
            assertEq(1, native.portReplies.size)
            assertEq("{\"live_handles\":0}", transport.statsJson())
            assertEq(8, transport.snapshot().size)
            assertEq(0, transport.restore(byteArrayOf(1)))
        }

        case("replies are decoded from the direct buffer and stay intact after the buffer is recycled") {
            val native = FakeNative()
            val events = RecordingEvents()
            InprocTransport(native).connect(events, HASH)
            native.emitReply(replyPayload(7u, ReplyStatus.OK, byteArrayOf(1, 2, 3, 4, 5)))
            native.emitReply(replyPayload(8u, ReplyStatus.ERROR, NO_BYTES))
            native.emitReply(replyPayload(9u, ReplyStatus.PANIC, byteArrayOf(9)))
            assertEq(
                listOf(
                    Triple(7u, ReplyStatus.OK, listOf<Byte>(1, 2, 3, 4, 5)),
                    Triple(8u, ReplyStatus.ERROR, emptyList<Byte>()),
                    Triple(9u, ReplyStatus.PANIC, listOf<Byte>(9)),
                ),
                events.replies.toList(),
            )
        }

        case("a malformed reply still tells the caller, by the call id JNI passes alongside it, as malformed") {
            val native = FakeNative()
            val events = RecordingEvents()
            InprocTransport(native).connect(events, HASH)
            native.emitReplyFor(11, byteArrayOf(1, 2)) // shorter than call_id + status
            native.emitReplyFor(12, byteArrayOf(12, 0, 0, 0, 99)) // an unknown status byte
            // Not a status the core never sent: a protocol failure, which generated code reports as Malformed.
            assertEq(0, events.replies.size)
            assertEq(listOf(11u, 12u), events.malformed.map { it.first })
            assertTrue(events.malformed.all { it.second.message!!.contains("malformed reply") }, "${events.malformed}")
            assertTrue(UndraCallError.mapped(events.malformed[0].second) is UndraCallError.Malformed, "maps to Malformed")
        }

        case("change-sets are copied out of the buffer before it is recycled") {
            val native = FakeNative()
            val events = RecordingEvents()
            InprocTransport(native).connect(events, HASH)
            val payload = changeSet(3uL, full(0x100000002L, 1u, byteArrayOf(5, 6, 7)))
            native.emitChangeSet(payload)
            assertEq(1, events.changeSets.size)
            assertTrue(events.changeSets.single().contentEquals(payload), "the copy differs from what the core sent")
        }

        case("stream items are decoded and copied; one that cannot be read is reported as malformed, not as the stream's E") {
            val native = FakeNative()
            val events = RecordingEvents()
            InprocTransport(native).connect(events, HASH)
            native.emitStream(4, Payloads.StreamItem(4u, StreamFlag.ITEM, byteArrayOf(8, 9)).toByteArray())
            native.emitStream(4, Payloads.StreamItem(4u, StreamFlag.END, NO_BYTES).toByteArray())
            native.emitStream(5, byteArrayOf(1)) // shorter than call_id + flag
            native.emitStream(6, byteArrayOf(6, 0, 0, 0, 4)) // flag 4 does not exist
            val cancelled = Payloads.StreamFailure(ReplyStatus.CANCELLED, "the runtime shut down", "").toByteArray()
            native.emitStream(7, Payloads.StreamItem(7u, StreamFlag.FAILED, cancelled).toByteArray())
            assertEq(Triple(4u, StreamFlag.ITEM, listOf<Byte>(8, 9)), events.items[0])
            assertEq(Triple(4u, StreamFlag.END, emptyList<Byte>()), events.items[1])
            assertEq(3, events.items.size, "the two unreadable items are not items")
            // A well-formed flag-3 item passes through as it came; UndraCore decodes the failure.
            assertEq(Triple(7u, StreamFlag.FAILED, cancelled.toList()), events.items[2])
            assertEq(listOf(5u, 6u), events.malformed.map { it.first }, "the call id JNI passes alongside the item")
            for ((_, error) in events.malformed) {
                assertTrue(error.message!!.contains("malformed stream item"), "$error")
                // Not flag 2 (the stream's own typed error, ADR-036) and not the core's own String (which reads as a panic): Malformed.
                assertTrue(UndraCallError.mappedStream(error) is UndraCallError.Malformed, "maps to Malformed")
            }
            assertEq(emptyList<Int>(), native.cancels.toList(), "the transport only reports; UndraCore decides what to cancel")
        }

        case("a sync port answer is handed back through portSyncReply on the same thread") {
            val native = FakeNative()
            val events = RecordingEvents()
            InprocTransport(native).connect(events, HASH)
            events.portAnswer = PortOutcome.Sync(Payloads.PortReply(3u, PortStatus.OK, byteArrayOf(1)).toByteArray())
            assertEq(0, native.portCall(10, 20, 3, byteArrayOf(4, 5, 6)))
            assertEq(Payloads.PortReply(3u, PortStatus.OK, byteArrayOf(1)), Payloads.PortReply.decode(native.syncPortReply!!))
            assertEq(listOf<Any>(10u, 20u, 3u, listOf<Byte>(4, 5, 6)), events.portCalls.single())
        }

        case("async and unavailable port answers map to 1 and 2") {
            val native = FakeNative()
            val events = RecordingEvents()
            InprocTransport(native).connect(events, HASH)
            events.portAnswer = PortOutcome.Async
            assertEq(1, native.portCall(1, 2, 3, NO_BYTES))
            events.portAnswer = PortOutcome.Unavailable
            assertEq(2, native.portCall(1, 2, 4, NO_BYTES))
        }

        case("portSyncReply is per thread, so concurrent sync port calls do not mix their answers") {
            val native = FakeNative()
            val events = object : TransportEvents by RecordingEvents() {
                override fun onPortCall(portId: UInt, methodId: UInt, portCallId: UInt, args: ByteArray): PortOutcome {
                    Thread.sleep(1) // widen the window in which another thread's answer could be picked up
                    return PortOutcome.Sync(Payloads.PortReply(portCallId, PortStatus.OK, args).toByteArray())
                }
            }
            InprocTransport(native).connect(events, HASH)
            val wrong = AtomicInteger()
            val start = CountDownLatch(1)
            val workers = List(8) { n ->
                Thread {
                    start.await()
                    repeat(50) { i ->
                        val id = n * 1000 + i
                        val (code, reply) = native.portCallFull(1, 1, id, byteArrayOf(n.toByte(), i.toByte()))
                        val decoded = Payloads.PortReply.decode(reply ?: NO_BYTES)
                        if (code != 0 || decoded.portCallId != id.toUInt() || decoded.body.toList() != listOf(n.toByte(), i.toByte())) wrong.incrementAndGet()
                    }
                }.also { it.start() }
            }
            start.countDown()
            workers.forEach { it.join(20_000) }
            assertEq(0, wrong.get())
            assertEq(0, native.violations.size)
        }

        case("the per-thread sync reply hand-off never leaks one call's answer to another") {
            val native = FakeNative()
            val events = RecordingEvents()
            InprocTransport(native).connect(events, HASH)
            events.portAnswer = PortOutcome.Sync(byteArrayOf(1, 1))
            native.portCall(1, 1, 1, NO_BYTES)
            assertEq(listOf<Byte>(1, 1), native.syncPortReply!!.toList())
            // Asking again without a new sync answer yields nothing rather than the stale one.
            assertEq(0, native.callbacks.portSyncReply().size)
        }

        case("exceptions thrown while handling a callback never escape into native code") {
            val native = FakeNative()
            val events = RecordingEvents()
            InprocTransport(native).connect(events, HASH)
            events.failWith = IllegalStateException("bug in the core layer")
            dev.undra.runtime.support.LogCapture("dev.undra.runtime").use { log ->
                native.emitReply(replyPayload(1u, ReplyStatus.OK))
                native.emitChangeSet(changeSet(1uL))
                native.emitStream(1, Payloads.StreamItem(1u, StreamFlag.END, NO_BYTES).toByteArray())
                assertEq(2, native.portCall(1, 1, 1, NO_BYTES), "a failing port call is answered as unavailable")
                assertEq(4, log.records.size)
            }
        }

        case("close shuts the native core down once and gives the claim back, so a new load starts fresh (ADR-034)") {
            val native = FakeNative()
            val first = InprocTransport(native)
            first.connect(RecordingEvents(), HASH)
            first.close()
            first.close()
            assertEq(1, native.shutdowns.get(), "close ends the native core's work, once")
            // The claim went with it: the same process loads a new core.
            val second = InprocTransport(native)
            second.connect(RecordingEvents(), HASH)
            assertEq(2, native.inits.get())
            second.close()
            assertEq(2, native.shutdowns.get())
            assertTrue(native.violations.isEmpty(), native.violations.toString())
        }

        case("close from inside a core callback is refused and leaves the core open (ADR-034)") {
            val native = FakeNative()
            val transport = InprocTransport(native)
            val events = RecordingEvents()
            transport.connect(events, HASH)
            var refused: Throwable? = null
            events.onPort = { refused = runCatching { transport.close() }.exceptionOrNull() }
            native.portCall(1, 1, 1, NO_BYTES)
            assertTrue(refused is UndraException, "close from a callback must throw: $refused")
            assertTrue(refused!!.message!!.contains("inside a core callback"), refused!!.message!!)
            assertEq(0, native.shutdowns.get(), "a refused close does not shut the core down")
            transport.close()
            assertEq(1, native.shutdowns.get())
            assertTrue(native.violations.isEmpty(), native.violations.toString())
        }

        case("an UndraCore closed from a core callback stays open; closed normally it shuts the native core down") {
            val native = FakeNative()
            val core = UndraCore.attach(InprocTransport(native), LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false), makeShared = false)
            var refused: Throwable? = null
            core.registerPort(
                PORT_FOR_CLOSE,
                PortImpl(true, portMethods(1u handledBy { _: ByteArray -> refused = runCatching { core.close() }.exceptionOrNull(); NO_BYTES })),
            )
            native.portCall(PORT_FOR_CLOSE.toInt(), 1, 1, NO_BYTES)
            assertTrue(refused is UndraException, "close from a sync port must throw: $refused")
            assertEq(0, native.shutdowns.get())
            native.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, Codecs.u32.encodeToByteArray(5u)) }
            assertEq(5u, Codecs.u32.decodeAll(core.callSync(TARGET, METHOD, NO_BYTES)), "the core is still open")
            core.close()
            assertEq(1, native.shutdowns.get())
        }

        case("callbacks after close are ignored") {
            val native = FakeNative()
            val events = RecordingEvents()
            val transport = InprocTransport(native)
            transport.connect(events, HASH)
            transport.close()
            transport.close()
            native.emitReply(replyPayload(1u, ReplyStatus.OK))
            native.emitChangeSet(changeSet(1uL))
            assertEq(2, native.portCall(1, 1, 1, NO_BYTES))
            assertEq(0, events.replies.size + events.changeSets.size)
        }

        case("through an UndraCore: call, callSync, stream and observe against the fake native") {
            val native = FakeNative()
            val core = UndraCore.attach(InprocTransport(native), LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false), makeShared = false)
            core.use {
                // callSync: straight through, reply decoded.
                native.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, Codecs.u32.encodeToByteArray(5u)) }
                assertEq(5u, Codecs.u32.decodeAll(core.callSync(TARGET, METHOD, NO_BYTES)))
                // call: the reply arrives on the core thread, into a direct buffer that is recycled at once.
                native.onCall = { call -> Thread { native.emitReply(replyPayload(call.callId, ReplyStatus.OK, byteArrayOf(1, 2, 3))) }.start() }
                assertEq(listOf<Byte>(1, 2, 3), runBlocking { core.call(TARGET, METHOD, NO_BYTES) }.toList())
                // call whose reply is delivered synchronously inside native call(), like the real sync path.
                native.onCall = { call -> native.emitReply(replyPayload(call.callId, ReplyStatus.OK, byteArrayOf(4))) }
                assertEq(listOf<Byte>(4), runBlocking { core.call(TARGET, METHOD, NO_BYTES) }.toList())
                // observe: the initial change-set is delivered inside native observe(), and applied before it returns.
                val seen = CopyOnWriteArrayList<UInt>()
                core.mirror.register(0x100000002L) { _, _, r -> seen.add(Codecs.u32.decode(r)) }
                native.onObserve = { handle, _, on -> if (on) native.emitChangeSet(changeSet(1uL, full(handle, 0u, Codecs.u32.encodeToByteArray(77u)))) }
                core.observe(0x100000002L, 0u, true)
                assertEq(listOf(77u), seen.toList(), "applied before observe returned")
                // stream: credit goes out only after the core says the stream is open, and never from inside a callback.
                native.onCall = { call ->
                    Thread {
                        native.emitReply(replyPayload(call.callId, ReplyStatus.STREAM_OPENED))
                        for (i in 0 until 3) native.emitStream(call.callId.toInt(), Payloads.StreamItem(call.callId, StreamFlag.ITEM, Codecs.u32.encodeToByteArray(i.toUInt())).toByteArray())
                        native.emitStream(call.callId.toInt(), Payloads.StreamItem(call.callId, StreamFlag.END, NO_BYTES).toByteArray())
                    }.start()
                }
                val items = runBlocking { core.stream(TARGET, METHOD, NO_BYTES).toList() }.map { Codecs.u32.decodeAll(it) }
                assertEq(listOf(0u, 1u, 2u), items)
                assertEq(16, native.credits.single().second)
                assertEq(emptyList<String>(), native.violations.toList())
            }
        }

        case("through an UndraCore: a stream the native core fails (flag 3) or garbles ends inside the UndraException hierarchy, and an unreadable item cancels the core's stream") {
            val native = FakeNative()
            val core = UndraCore.attach(InprocTransport(native), LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false), makeShared = false)
            core.use {
                val garbledCallId = java.util.concurrent.atomic.AtomicReference<UInt>()
                fun endWith(item: (callId: UInt) -> ByteArray): UndraException {
                    native.onCall = { call ->
                        Thread {
                            native.emitReply(replyPayload(call.callId, ReplyStatus.STREAM_OPENED))
                            native.emitStream(call.callId.toInt(), Payloads.StreamItem(call.callId, StreamFlag.ITEM, Codecs.u32.encodeToByteArray(1u)).toByteArray())
                            native.emitStream(call.callId.toInt(), item(call.callId))
                        }.start()
                    }
                    val seen = CopyOnWriteArrayList<UInt>()
                    val e = assertThrows<UndraException> {
                        runBlocking { core.stream(TARGET, METHOD, NO_BYTES).collect { seen.add(Codecs.u32.decodeAll(it)) } }
                    }
                    assertEq(listOf(1u), seen.toList(), "the item before the end was delivered")
                    return e
                }
                // Shutdown or a restore that replaced the receiver: cancelled by the core.
                val cancelled = endWith { id ->
                    Payloads.StreamItem(id, StreamFlag.FAILED, Payloads.StreamFailure(ReplyStatus.CANCELLED, "the runtime shut down", "").toByteArray()).toByteArray()
                }
                assertEq(ReplyStatus.CANCELLED, (cancelled as UndraReplyException).status)
                // A stream that panicked.
                val panicked = endWith { id ->
                    Payloads.StreamItem(id, StreamFlag.FAILED, Payloads.StreamFailure(ReplyStatus.PANIC, "boom", "at core.rs:1").toByteArray()).toByteArray()
                }
                assertEq(Payloads.PanicInfo("boom", "at core.rs:1"), (panicked as UndraReplyException).panicInfo)
                // A stream the core refused.
                val refused = endWith { id ->
                    Payloads.StreamItem(id, StreamFlag.FAILED, Payloads.StreamFailure(ReplyStatus.BAD_REQUEST, "stale handle", "").toByteArray()).toByteArray()
                }
                assertEq(ReplyStatus.BAD_REQUEST, (refused as UndraReplyException).status)
                assertEq("stale handle", refused.badRequestReason)
                assertEq(UndraCallError.Refused("stale handle").message, UndraCallError.mappedStream(refused).message, "status 5 maps to Refused, as a failed call does")
                // An item the transport cannot read at all.
                assertEq(emptyList<Int>(), native.cancels.toList(), "the core's own failures need no cancel")
                val garbled = endWith { id -> garbledCallId.set(id); Codecs.u32.encodeToByteArray(id) + byteArrayOf(9) }
                assertTrue(garbled is UndraProtocolException, "$garbled")
                assertTrue(garbled.message!!.startsWith("the core sent a malformed stream item: "), garbled.message!!)
                assertTrue(UndraCallError.mappedStream(garbled) is UndraCallError.Malformed, "maps to Malformed")
                // A flag-3 item whose failure body cannot be read.
                val badFailure = endWith { id -> Payloads.StreamItem(id, StreamFlag.FAILED, byteArrayOf(1)).toByteArray() }
                assertTrue(badFailure is UndraProtocolException, "$badFailure")
                assertTrue(badFailure.message!!.startsWith("the core sent a malformed stream failure: "), badFailure.message!!)
                assertTrue(UndraCallError.mappedStream(badFailure) is UndraCallError.Malformed, "maps to Malformed")
                eventually("no stream is left pending") { core.stats().hostPendingCalls == 0 }
                // Only the unreadable item leaves the core's stream open (L6): one cancel, for that call. A flag-3
                // item the core sent itself (cancelled, panic, an unreadable failure body) ended the core's side.
                eventually("the unreadable item's stream is cancelled once") { native.cancels.size == 1 }
                assertEq(garbledCallId.get(), native.cancels.single().toUInt())
                assertEq(emptyList<String>(), native.violations.toList())
            }
        }

        case("many threads calling through an UndraCore while the core replies from its own thread") {
            val native = FakeNative()
            val core = UndraCore.attach(InprocTransport(native), LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false), makeShared = false)
            core.use {
                native.onCall = { call -> Thread { native.emitReply(replyPayload(call.callId, ReplyStatus.OK, call.args)) }.start() }
                runBlocking {
                    val results = List(300) { i -> async(Dispatchers.Default) { core.call(TARGET, METHOD, byteArrayOf((i % 100).toByte())) } }
                    results.forEachIndexed { i, d -> assertEq(listOf((i % 100).toByte()), d.await().toList()) }
                }
                eventually("no call is left pending") { core.stats().hostPendingCalls == 0 }
                assertEq(emptyList<String>(), native.violations.toList())
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
