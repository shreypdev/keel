package dev.undra.runtime

import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.ManualMainThread
import dev.undra.runtime.support.changeSet
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.flushOnThisThread
import dev.undra.runtime.support.full
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Payloads.PortStatus
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.encodeToByteArray
import java.lang.ref.WeakReference
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import java.util.logging.Level
import kotlin.coroutines.cancellation.CancellationException
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.awaitCancellation
import org.junit.jupiter.api.Test

/*
 * Host callback interfaces (ADR-041): the registry (lending, interning, giving back), delivery through the mirror's
 * drain in order with the change-sets, `coalesce`, `__release` and `__cancel`, the answers of `async` methods and
 * how failures map, a weak wrapper, `background` delivery on a serial executor per instance, and a disconnect.
 */

/** A callback interface, as generated (`#[undra::callback] trait Listener`). */
private interface Listener {
    fun note(line: String)
    fun progress(done: UInt)
    suspend fun ask(question: String): Boolean
}

/** The interface's own error (`#[undra::error]`). */
private class AskError(val code: UByte) : UndraException("ask failed: $code")

private object Ids {
    const val PORT: UInt = 0x1000_0001u
    const val NOTE: UInt = 0x2000_0001u
    const val PROGRESS: UInt = 0x2000_0002u
    const val ASK: UInt = 0x2000_0003u
    const val RELEASE: UInt = 0x2000_00f1u
    const val CANCEL: UInt = 0x2000_00f2u
    const val BG_PORT: UInt = 0x1000_0002u
}

/** The bridge bindgen generates for [Listener] (`main` delivery, or `background` with [background]). */
private class ListenerBridge(portId: UInt, background: Boolean) : UndraCallbackBridge<Listener>(
    name = "Listener",
    portId = portId,
    releaseInstance = Ids.RELEASE,
    cancelCall = Ids.CANCEL,
    background = background,
    coalesced = setOf(Ids.PROGRESS),
) {
    override fun invocation(methodId: UInt, args: UndraReader): UndraCallbackInvocation<Listener>? =
        when (methodId) {
            Ids.NOTE -> {
                val line = args.readStr()
                args.finish()
                UndraCallbackInvocation.Notify("note") { it.note(line) }
            }
            Ids.PROGRESS -> {
                val done = args.readU32()
                args.finish()
                UndraCallbackInvocation.Notify("progress") { it.progress(done) }
            }
            Ids.ASK -> {
                val question = args.readStr()
                args.finish()
                UndraCallbackInvocation.Ask("ask") {
                    try {
                        Codecs.bool.encodeToByteArray(it.ask(question))
                    } catch (e: AskError) {
                        throw UndraPortException(byteArrayOf(e.code.toByte()))
                    }
                }
            }
            else -> null
        }
}

/** What the generated `Listener.weak(target)` is. */
private class WeakListener(target: Listener) : Listener {
    private val target = WeakReference(target)
    override fun note(line: String) {
        target.get()?.note(line)
    }
    override fun progress(done: UInt) {
        target.get()?.progress(done)
    }
    override suspend fun ask(question: String): Boolean {
        val target = target.get() ?: throw UndraCallbackGoneException("Listener")
        return target.ask(question)
    }
}

/** A listener that records its calls; [answer] decides what `ask` does. */
private open class ListenerLog(val seen: MutableList<String> = CopyOnWriteArrayList()) : Listener {
    @Volatile var answer: suspend (String) -> Boolean = { true }
    override fun note(line: String) {
        seen.add("note $line")
    }
    override fun progress(done: UInt) {
        seen.add("progress $done")
    }
    override suspend fun ask(question: String): Boolean = answer(question)
}

/** A store with one `u32` signal (`notes`). */
private class Notes(core: UndraCore, handle: Long) : UndraStore(core, handle) {
    @Volatile var value = 0u
    override fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader) {
        value = Codecs.u32.decode(reader)
        reader.finish()
    }
}

private val NEVER = FramePacer { }

private fun args(instance: ULong, write: UndraWriter.() -> Unit = {}): ByteArray =
    UndraWriter().apply {
        writeU64(instance)
        write()
    }.toByteArray()

private class CallbackRig(background: Boolean = false) : AutoCloseable {
    val t = FakeTransport()
    val main = ManualMainThread()
    val errors = CopyOnWriteArrayList<UndraUnhandledError>()
    val core = ConnectedCore(t, 5.seconds, mirrorOptions = MirrorOptions(framePacer = NEVER), main = main, onError = { errors.add(it) })
    val port = if (background) Ids.BG_PORT else Ids.PORT

    init {
        t.connect(core, HASH)
        core.installCallbacks(listOf(ListenerBridge(port, background)))
    }

    fun call(method: UInt, instance: ULong, portCallId: UInt = 0u, write: UndraWriter.() -> Unit = {}): PortOutcome =
        t.portCall(port, method, portCallId, args(instance, write))

    fun note(instance: ULong, line: String) = call(Ids.NOTE, instance) { writeStr(line) }
    fun ask(instance: ULong, id: UInt, question: String = "go on?") = call(Ids.ASK, instance, id) { writeStr(question) }
    fun release(instance: ULong) = call(Ids.RELEASE, instance)
    fun cancel(instance: ULong, id: UInt) = call(Ids.CANCEL, instance) { writeU32(id) }

    fun drain() = core.mirror.flushOnThisThread(main)

    fun replies(): List<Pair<UInt, PortStatus>> = t.portReplies.map { it.portCallId to it.status }

    override fun close() = core.close()
}

class CallbackTests : Suite() {
    init {
        registry()
        delivery()
        answers()
        lifecycle()
    }

    private fun registry() {
        case("lend interns by identity and counts one reference per crossing; the entry goes at zero; handles are never reused") {
            val callbacks = UndraCallbacks()
            val a = ListenerLog()
            val b = ListenerLog()
            val ha = callbacks.lend(a)
            assertEq(1uL, ha, "the per-core counter starts at 1")
            assertEq(ha, callbacks.lend(a), "the same object is the same handle")
            val hb = callbacks.lend(b)
            assertTrue(hb != ha && hb != 0uL)
            assertEq(2, callbacks.count(a))
            assertEq(1, callbacks.count(b))
            assertEq(2, callbacks.liveCount)
            callbacks.release(ha)
            assertEq(1, callbacks.count(a))
            callbacks.giveBack(ha)
            assertEq(0, callbacks.count(a))
            assertEq(null, callbacks.instanceOf(a))
            assertEq(1, callbacks.liveCount)
            val again = callbacks.lend(a)
            assertTrue(again != ha && again != hb, "a handle is never reused")
        }

        case("an over-release is logged as an error and frees nothing early") {
            LogCapture("dev.undra.runtime", Level.ALL).use { log ->
                val callbacks = UndraCallbacks()
                val a = ListenerLog()
                val h = callbacks.lend(a)
                callbacks.release(h)
                callbacks.release(h)
                callbacks.release(999uL)
                assertEq(0, callbacks.liveCount)
                assertTrue(log.records.count { it.level == Level.SEVERE && it.message.contains("more often than it was lent") } == 2, log.messages().toString())
                val b = ListenerLog()
                val hb = callbacks.lend(b)
                callbacks.release(h) // still an over-release: b is not touched
                assertEq(1, callbacks.count(b))
                assertTrue(hb != h)
            }
        }

        case("giveBackIfRefused gives back only what the core did not take") {
            val callbacks = UndraCallbacks()
            val a = ListenerLog()
            fun taken(error: Throwable): Boolean {
                val h = callbacks.lend(a)
                val before = callbacks.count(a)
                callbacks.giveBackIfRefused(error, h)
                val kept = callbacks.count(a) == before
                while (callbacks.count(a) > 0) callbacks.release(h)
                return kept
            }
            assertTrue(!taken(UndraReplyException(ReplyStatus.BAD_REQUEST, ByteArray(0))), "a refusal (status 5)")
            assertTrue(!taken(UndraTransportException(UndraTransportException.Reason.CLOSED, "closed")), "a closed core")
            assertTrue(!taken(UndraTransportException(UndraTransportException.Reason.CONNECTION_LOST, "lost")), "a lost connection")
            assertTrue(!taken(UndraCallError.Refused("foreign object")), "refused before sending")
            assertTrue(!taken(IllegalArgumentException("encoding")), "a failure before sending")
            assertTrue(taken(UndraReplyException(ReplyStatus.ERROR, ByteArray(1))), "the method's own error")
            assertTrue(taken(UndraReplyException(ReplyStatus.PANIC, ByteArray(0))), "a panic")
            assertTrue(taken(UndraReplyException(ReplyStatus.CANCELLED, ByteArray(0))), "cancelled by the core")
            assertTrue(taken(CancellationException("caller cancelled")), "the caller's cancellation")
            assertTrue(taken(UndraTransportException(UndraTransportException.Reason.TIMEOUT, "slow")), "a timeout")
            assertTrue(taken(UndraProtocolException("malformed reply")), "a malformed reply")
            callbacks.giveBackIfRefused(IllegalStateException(), null)
        }
    }

    private fun delivery() {
        case("an invocation runs in the drain, after the change-sets that arrived before it, never inside the port call") {
            CallbackRig().use { rig ->
                val notes = Notes(rig.core, 1L)
                val seen = CopyOnWriteArrayList<String>()
                val listener = object : ListenerLog(seen) {
                    override fun note(line: String) {
                        seen.add("$line@${notes.value}")
                    }
                }
                val h = rig.core.callbacks.lend(listener)
                rig.t.events.onChangeSet(changeSet(1uL, full(1L, 0u, Codecs.u32.encodeToByteArray(1u))))
                assertEq(PortOutcome.Async, rig.note(h, "one"))
                rig.t.events.onChangeSet(changeSet(2uL, full(1L, 0u, Codecs.u32.encodeToByteArray(2u))))
                rig.note(h, "two")
                assertEq(emptyList<String>(), seen.toList(), "only queued by the port call")
                rig.drain()
                assertEq(listOf("one@1", "two@2"), seen.toList())
                assertEq(2L, rig.core.mirror.stats().callbacksDelivered)
                notes.close()
            }
        }

        case("a coalesce method delivers only the newest pending call per instance in a drain; the others all arrive in order") {
            CallbackRig().use { rig ->
                val a = ListenerLog()
                val b = ListenerLog()
                val ha = rig.core.callbacks.lend(a)
                val hb = rig.core.callbacks.lend(b)
                for (i in 1..50) {
                    rig.call(Ids.PROGRESS, ha) { writeU32(i.toUInt()) }
                    rig.note(ha, "burst $i")
                }
                rig.call(Ids.PROGRESS, hb) { writeU32(7u) }
                rig.drain()
                assertEq(listOf("progress 50"), a.seen.filter { it.startsWith("progress") })
                assertEq((1..50).map { "note burst $it" }, a.seen.filter { it.startsWith("note") })
                assertEq(listOf("progress 7"), b.seen.toList(), "per instance")
                assertEq(52L, rig.core.mirror.stats().callbacksDelivered)
            }
        }

        case("__release takes effect when the queue reaches it, after the invocations queued before it") {
            CallbackRig().use { rig ->
                val a = ListenerLog()
                val h = rig.core.callbacks.lend(a)
                rig.note(h, "last words")
                rig.release(h)
                assertEq(1, rig.core.callbacks.liveCount)
                rig.drain()
                assertEq(listOf("note last words"), a.seen.toList())
                assertEq(0, rig.core.callbacks.liveCount)
                rig.note(h, "too late")
                rig.drain()
                assertEq(1, a.seen.size, "an instance the host no longer holds is not called")
            }
        }

        case("a background interface runs on a serial executor per instance, in call order, without a drain") {
            CallbackRig(background = true).use { rig ->
                val running = AtomicInteger()
                val overlap = AtomicInteger()
                val seen = CopyOnWriteArrayList<String>()
                val threads = CopyOnWriteArrayList<String>()
                val listener = object : ListenerLog(seen) {
                    override fun note(line: String) {
                        if (running.incrementAndGet() > 1) overlap.incrementAndGet()
                        threads.add(Thread.currentThread().name)
                        Thread.sleep(1)
                        seen.add(line)
                        running.decrementAndGet()
                    }
                }
                val h = rig.core.callbacks.lend(listener)
                for (i in 1..40) rig.note(h, "$i")
                rig.release(h)
                eventually("every call ran and the release came last") { seen.size == 40 && rig.core.callbacks.liveCount == 0 }
                assertEq((1..40).map { "$it" }, seen.toList())
                assertEq(0, overlap.get(), "one at a time")
                assertEq(0L, rig.core.mirror.stats().callbacksDelivered, "not through the drain")
                assertTrue(threads.none { it == Thread.currentThread().name })
            }
        }
    }

    private fun answers() {
        case("an async method answers with its value (status 0), its own error (status 1), or unavailable (status 2) and a report") {
            CallbackRig().use { rig ->
                val a = ListenerLog()
                val h = rig.core.callbacks.lend(a)
                a.answer = { true }
                rig.ask(h, 7u)
                rig.drain()
                assertEq(listOf(7u to PortStatus.OK), rig.replies())
                assertEq(true, Codecs.bool.decode(UndraReader(rig.t.portReplies.last().body)))
                a.answer = { throw AskError(3u) }
                rig.ask(h, 8u)
                rig.drain()
                assertEq(8u to PortStatus.ERROR, rig.replies().last())
                assertEq(listOf<Byte>(3), rig.t.portReplies.last().body.toList())
                assertEq(0, rig.errors.size, "the method's own error is not reported")
                a.answer = { throw IllegalStateException("a bug") }
                rig.ask(h, 9u)
                rig.drain()
                assertEq(9u to PortStatus.UNAVAILABLE, rig.replies().last())
                eventually("reported") { rig.errors.size == 1 }
                assertEq("Listener.ask", rig.errors.single().operation)
            }
        }

        case("a throw from a fire-and-forget method is reported and answers nothing") {
            CallbackRig().use { rig ->
                val bad = object : ListenerLog() {
                    override fun note(line: String) = throw IllegalStateException("bad note")
                }
                val h = rig.core.callbacks.lend(bad)
                rig.note(h, "x")
                rig.drain()
                eventually("reported") { rig.errors.size == 1 }
                assertEq("Listener.note", rig.errors.single().operation)
                assertEq(0, rig.t.portReplies.size)
            }
        }

        case("__cancel cancels the running job at once, and a call cancelled before it starts never runs") {
            CallbackRig().use { rig ->
                val cancelled = CopyOnWriteArrayList<String>()
                val started = CopyOnWriteArrayList<String>()
                val a = ListenerLog()
                a.answer = { q ->
                    started.add(q)
                    try {
                        awaitCancellation()
                    } catch (e: CancellationException) {
                        cancelled.add(q)
                        throw e
                    }
                }
                val h = rig.core.callbacks.lend(a)
                rig.ask(h, 11u, "running")
                rig.drain()
                assertEq(listOf("running"), started.toList())
                rig.cancel(h, 11u)
                eventually("the job saw the cancellation") { cancelled.contains("running") }
                rig.ask(h, 12u, "queued")
                rig.cancel(h, 12u)
                eventually("the cancel was processed") { true }
                Thread.sleep(50)
                rig.drain()
                assertEq(listOf("running"), started.toList(), "a call cancelled before it started never runs")
                Thread.sleep(20)
                assertEq(0, rig.t.portReplies.size, "nobody waits for an answer")
                assertEq(0, rig.errors.size)
                assertEq(1, rig.core.callbacks.liveCount, "the instance stays until the core lets go")
            }
        }

        case("a weak wrapper forwards while its target lives, then does nothing or answers unavailable, unreported") {
            CallbackRig().use { rig ->
                val seen = CopyOnWriteArrayList<String>()
                var target: ListenerLog? = ListenerLog(seen)
                val weak = WeakListener(target!!)
                val h = rig.core.callbacks.lend(weak)
                rig.note(h, "alive")
                rig.drain()
                assertEq(listOf("note alive"), seen.toList())
                val ref = WeakReference(target)
                target = null
                eventually("the target is collected", timeoutMs = 20_000) {
                    System.gc()
                    ref.get() == null
                }
                rig.note(h, "gone")
                rig.ask(h, 21u)
                rig.drain()
                assertEq(listOf("note alive"), seen.toList())
                assertEq(listOf(21u to PortStatus.UNAVAILABLE), rig.replies())
                assertEq(0, rig.errors.size)
            }
        }

        case("an unknown method or arguments that do not decode answer unavailable") {
            CallbackRig().use { rig ->
                val h = rig.core.callbacks.lend(ListenerLog())
                assertEq(PortOutcome.Unavailable, rig.call(0x7777u, h, 5u))
                assertEq(PortOutcome.Unavailable, rig.call(Ids.NOTE, h) { writeU8(1u) })
                eventually("reported") { rig.errors.size == 1 }
                assertEq(PortOutcome.Unavailable, rig.t.portCall(0x4242u, Ids.NOTE, 0u, args(h)), "not a callback port: a plain unregistered port")
            }
        }
    }

    private fun lifecycle() {
        case("a lost connection keeps the registry for the session's return and a closed core drops it; a late release is not an error") {
            LogCapture("dev.undra.runtime", Level.ALL).use { log ->
                val rig = CallbackRig()
                val a = ListenerLog()
                val h = rig.core.callbacks.lend(a)
                rig.t.drop()
                // The server keeps the session's objects, and the proxies of the instances lent, for its return (ADR-051).
                assertEq(1, rig.core.callbacks.liveCount)
                assertEq(h, rig.core.callbacks.instanceOf(a))
                rig.core.callbacks.release(h)
                assertEq(0, rig.core.callbacks.liveCount)
                assertTrue(log.records.none { it.level == Level.SEVERE }, log.messages().toString())
                val h2 = rig.core.callbacks.lend(a)
                rig.close()
                assertEq(0, rig.core.callbacks.liveCount)
                // A release that arrives after the close is expected, not an over-release.
                rig.core.callbacks.release(h2)
                assertTrue(log.records.none { it.level == Level.SEVERE }, log.messages().toString())
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
