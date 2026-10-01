package dev.undra.runtime

import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.changeSet
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.full
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.encodeToByteArray
import java.util.logging.Level
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private val METHOD = 0x21u
private val TARGET = CallTarget.ObjectMethod(Handle(0x100000002L), METHOD)
private const val STORE = 0x100000002L
private const val OTHER = 0x100000003L

/** A remote-mode core over a fake transport that can be dropped and brought back, and the states it went through. */
private class Rig(
    val t: FakeTransport = FakeTransport(isSynchronous = false),
    onError: ((UndraUnhandledError) -> Unit)? = null,
) : AutoCloseable {
    val states = CopyOnWriteArrayList<ConnectionState>()
    val core: UndraCore = UndraCore.attach(
        t,
        LoadOptions(
            mode = Mode.REMOTE,
            remoteUrl = "ws://fake",
            expectedSchemaHash = HASH,
            defaultAdapters = false,
            onConnectionChange = { states.add(it) },
            onError = onError,
        ),
        makeShared = false,
    )

    /** What the core reports, in short: `connecting`, `connected`, `reconnecting 2`, `closed:SESSION_LOST`. */
    fun log(): List<String> = states.map(::name)

    override fun close() = core.close()
}

private fun name(state: ConnectionState): String = when (state) {
    ConnectionState.Connecting -> "connecting"
    ConnectionState.Connected -> "connected"
    is ConnectionState.Reconnecting -> "reconnecting ${state.attempt}"
    is ConnectionState.Closed -> "closed:${state.reason}"
}

private fun u32(n: UInt): ByteArray = Codecs.u32.encodeToByteArray(n)

/** What a transport that reconnects makes of the core: state, what fails, what is observed again. */
class ReconnectCoreTests : Suite() {
    init {
        case("a dev notice from `undra dev` reaches onDevNotice of a remote core, once per record, on a thread of the runtime's") {
            val notices = CopyOnWriteArrayList<Pair<String, String>>()
            val t = FakeTransport(isSynchronous = false)
            val core = UndraCore.attach(
                t,
                LoadOptions(
                    mode = Mode.REMOTE,
                    remoteUrl = "ws://fake",
                    expectedSchemaHash = HASH,
                    defaultAdapters = false,
                    onDevNotice = { notices.add(Thread.currentThread().name to it) },
                ),
                makeShared = false,
            )
            core.use {
                t.events.onLog(2u, "undra::dev", "Reloaded, state kept")
                t.events.onLog(2u, "app", "something else")
                t.events.onLog(2u, "undra::dev", "Reloaded, state reset: schema changed")
                eventually("both notices arrive") { notices.size == 2 }
                assertEq(listOf("Reloaded, state kept", "Reloaded, state reset: schema changed"), notices.map { it.second })
                assertTrue(notices.none { it.first == Thread.currentThread().name }, "not on the transport's thread: ${notices.map { it.first }}")
            }
        }

        case("an in-process core never fires onDevNotice, whatever its log says") {
            val notices = CopyOnWriteArrayList<String>()
            val t = FakeTransport(isSynchronous = true)
            val core = UndraCore.attach(
                t,
                LoadOptions(
                    mode = Mode.INPROC,
                    expectedSchemaHash = HASH,
                    defaultAdapters = false,
                    onDevNotice = { notices.add(it) },
                ),
                makeShared = false,
            )
            core.use {
                t.events.onLog(2u, "undra::dev", "Reloaded, state kept")
                Thread.sleep(200)
                assertEq(emptyList(), notices.toList())
            }
        }

        case("an onDevNotice that throws is logged and does not end the core or the next notice") {
            val notices = CopyOnWriteArrayList<String>()
            val t = FakeTransport(isSynchronous = false)
            val core = UndraCore.attach(
                t,
                LoadOptions(
                    mode = Mode.REMOTE,
                    remoteUrl = "ws://fake",
                    expectedSchemaHash = HASH,
                    defaultAdapters = false,
                    onDevNotice = { message ->
                        notices.add(message)
                        if (message == "boom") throw IllegalStateException("the bar broke")
                    },
                ),
                makeShared = false,
            )
            core.use {
                t.events.onLog(2u, "undra::dev", "boom")
                t.events.onLog(2u, "undra::dev", "after")
                eventually("the notice after the failing one arrives") { notices.size == 2 }
                assertEq(ConnectionState.Connected, core.connectionState.value)
            }
        }

        case("a core reports connecting, then connected; a drop is reconnecting, and the way back is connected") {
            Rig().use { rig ->
                assertEq(listOf("connecting", "connected"), rig.log())
                assertEq(ConnectionState.Connected, rig.core.connectionState.value)
                rig.t.drop()
                val dropped = rig.core.connectionState.value
                assertTrue(dropped is ConnectionState.Reconnecting && dropped.attempt == 1, "reconnecting 1, was $dropped")
                rig.t.retry(2)
                rig.t.retry(3)
                rig.t.reconnect()
                eventually("connected again") { rig.core.connectionState.value == ConnectionState.Connected }
                assertEq(listOf("connecting", "connected", "reconnecting 1", "reconnecting 2", "reconnecting 3", "connected"), rig.log())
            }
        }

        case("a drop fails what is in flight with an UndraTransportException (CONNECTION_LOST) at once, and the core stays open") {
            Rig().use { rig ->
                val outcomes = CopyOnWriteArrayList<Throwable>()
                val blocked = Thread {
                    try {
                        rig.core.callSync(TARGET, METHOD, NO_BYTES)
                    } catch (e: Throwable) {
                        outcomes.add(e)
                    }
                }
                val suspended = Thread {
                    try {
                        runBlocking { rig.core.call(TARGET, METHOD, NO_BYTES) }
                    } catch (e: Throwable) {
                        outcomes.add(e)
                    }
                }
                val streaming = Thread {
                    try {
                        runBlocking { rig.core.stream(TARGET, METHOD, NO_BYTES).toList() }
                    } catch (e: Throwable) {
                        outcomes.add(e)
                    }
                }
                listOf(blocked, suspended, streaming).forEach { it.isDaemon = true; it.start() }
                eventually("all three were sent") { rig.t.calls.size == 3 }
                rig.t.drop(UndraException("socket reset"))
                listOf(blocked, suspended, streaming).forEach { it.join(10_000) }
                assertEq(3, outcomes.size, "nothing hangs: $outcomes")
                for (outcome in outcomes) {
                    assertTrue(outcome is UndraTransportException, "typed: $outcome")
                    val e = outcome as UndraTransportException
                    assertEq(UndraTransportException.Reason.CONNECTION_LOST, e.reason)
                    assertTrue(e.message!!.contains("reconnecting") && e.message!!.contains("socket reset"), e.message!!)
                    // What a generated call throws for it (ADR-032 amendment A): the closed set's Unavailable.
                    val mapped = UndraCallError.mapped(e) as UndraCallError.Unavailable
                    assertTrue(mapped.transport === e)
                }
                assertEq(0, rig.core.stats().hostPendingCalls)
                assertTrue(rig.core.connectionState.value is ConnectionState.Reconnecting)
            }
        }

        case("calls, constructors and observations made while reconnecting fail at once with the same type, Unavailable once mapped") {
            Rig().use { rig ->
                rig.t.drop()
                val e = assertThrows<UndraTransportException> { rig.core.callSync(TARGET, METHOD, NO_BYTES) }
                assertTrue(e.message!!.contains("reconnecting"), e.message!!)
                val failures = listOf(
                    e,
                    assertThrows<UndraTransportException> { runBlocking { rig.core.call(TARGET, METHOD, NO_BYTES) } },
                    assertThrows<UndraTransportException> { rig.core.construct(1u, 2u, NO_BYTES) },
                    assertThrows<UndraTransportException> { rig.core.observe(STORE, UInt.MAX_VALUE, true) },
                    assertThrows<UndraCallError.Unavailable> { rig.core.constructObject(1u, 2u, NO_BYTES) }.transport,
                )
                for (failure in failures) {
                    assertEq(UndraTransportException.Reason.CONNECTION_LOST, failure.reason)
                    assertTrue(UndraCallError.mapped(failure) is UndraCallError.Unavailable)
                }
                assertEq(0, rig.t.observes.size, "nothing reached the transport")
            }
        }

        case("a command that fails because the connection is down is not handed to onError, but is logged; a core the app closed still is") {
            val reports = CopyOnWriteArrayList<UndraUnhandledError>()
            LogCapture("dev.undra.runtime").use { log ->
                Rig(onError = { reports.add(it) }).use { rig ->
                    // Connected: a failure that is not the connection reaches the handler.
                    rig.core.report(UndraReplyException(ReplyStatus.CANCELLED, NO_BYTES), "Todos.toggle")
                    assertEq(listOf("Todos.toggle"), reports.map { it.operation })

                    // Reconnecting: a command tapped meanwhile fails with Unavailable(CONNECTION_LOST); the state already says so.
                    rig.t.drop()
                    val tapped = assertThrows<UndraTransportException> { rig.core.callSync(TARGET, METHOD, NO_BYTES) }
                    rig.core.report(tapped, "Todos.toggle")
                    assertEq(1, reports.size, "not reported while reconnecting")
                    assertTrue(
                        log.records.any { it.level == Level.WARNING && it.message.contains("Todos.toggle") && it.message.contains("connectionState") },
                        "logged at warning level: ${log.messages()}",
                    )
                    assertTrue(log.records.none { it.level == Level.SEVERE && it.message.contains("the connection to the core was lost") })

                    // The way back: failures reach the handler again.
                    rig.t.reconnect()
                    eventually("connected") { rig.core.connectionState.value == ConnectionState.Connected }
                    rig.core.report(UndraReplyException(ReplyStatus.CANCELLED, NO_BYTES), "Todos.toggle")
                    assertEq(2, reports.size)
                }
                // A core the app closed: a use after close is a programming error, reported.
                val closed = Rig(onError = { reports.add(it) })
                closed.core.close()
                val e = assertThrows<UndraTransportException> { closed.core.callSync(TARGET, METHOD, NO_BYTES) }
                assertEq(UndraTransportException.Reason.CLOSED, e.reason)
                closed.core.report(e, "Todos.toggle")
                assertEq(3, reports.size, "a call on a core the app closed is reported")
                // A timeout is not the connection state's news either.
                val live = Rig(onError = { reports.add(it) })
                live.core.report(UndraTransportException(UndraTransportException.Reason.TIMEOUT, "no answer"), "Todos.toggle")
                assertEq(4, reports.size)
                live.close()
            }
        }

        case("after the way back calls work again") {
            Rig().use { rig ->
                rig.t.drop()
                rig.t.reconnect()
                eventually("connected") { rig.core.connectionState.value == ConnectionState.Connected }
                rig.t.onCall = { call -> rig.t.replyOnCore(call.callId, ReplyStatus.OK, byteArrayOf(7)) }
                assertEq(listOf<Byte>(7), runBlocking { rig.core.call(TARGET, METHOD, NO_BYTES) }.toList())
            }
        }

        case("every store the app observes is observed again after a reconnect, and the mirror converges on the answer") {
            Rig().use { rig ->
                val seen = CopyOnWriteArrayList<UInt>()
                var value = 5u
                rig.t.onObserve = { handle, signal, on ->
                    if (on) rig.t.onCore { rig.t.events.onChangeSet(changeSet(1uL, full(handle, 0u, u32(value)))) }
                }
                rig.core.mirror.register(STORE) { _, _, r -> seen.add(Codecs.u32.decode(r)) }
                rig.core.mirror.register(OTHER) { _, _, _ -> }
                rig.core.observe(STORE, UInt.MAX_VALUE, true)
                rig.core.observe(OTHER, 0u, true)
                rig.core.observe(OTHER, 1u, true)
                eventually("the first values") { seen == listOf(5u) }

                rig.t.observes.clear()
                value = 50u // changed while the client was away
                rig.t.drop()
                rig.t.reconnect()
                eventually("the mirror has the new value") { seen == listOf(5u, 50u) }
                assertEq(
                    setOf(Triple(STORE, UInt.MAX_VALUE, true), Triple(OTHER, 0u, true), Triple(OTHER, 1u, true)),
                    rig.t.observes.toSet(),
                )
                eventually("connected") { rig.core.connectionState.value == ConnectionState.Connected }
            }
        }

        case("what the app stopped observing or released is not observed again") {
            Rig().use { rig ->
                rig.core.observe(STORE, UInt.MAX_VALUE, true)
                rig.core.observe(OTHER, 0u, true)
                rig.core.observe(OTHER, 0u, false)
                val third = 0x100000004L
                rig.core.observe(third, UInt.MAX_VALUE, true)
                rig.core.release(third)
                rig.t.observes.clear()
                rig.t.releases.clear()
                rig.t.drop()
                rig.t.reconnect()
                eventually("connected") { rig.core.connectionState.value == ConnectionState.Connected }
                assertEq(listOf(Triple(STORE, UInt.MAX_VALUE, true)), rig.t.observes.toList())
                assertEq(emptyList<Long>(), rig.t.releases.toList(), "released before the drop: nothing left to release")
            }
        }

        case("an object released while the connection is down is released at the server once it is back, without an error") {
            Rig().use { rig ->
                rig.core.observe(STORE, UInt.MAX_VALUE, true)
                rig.t.drop()
                rig.core.release(STORE)
                assertEq(emptyList<Long>(), rig.t.releases.toList(), "nothing can be sent yet")
                rig.t.observes.clear()
                rig.t.reconnect()
                eventually("connected") { rig.core.connectionState.value == ConnectionState.Connected }
                assertEq(listOf(STORE), rig.t.releases.toList())
                assertEq(emptyList<Triple<Long, UInt, Boolean>>(), rig.t.observes.toList(), "and not observed again")
            }
        }

        case("the core asks the server to resume only when it holds objects its constructors made") {
            Rig().use { rig ->
                val events = rig.core as TransportEvents
                assertEq(false, events.holdsObjects())
                rig.t.onCall = { call -> rig.t.replyOnCore(call.callId, ReplyStatus.OK, Codecs.handle.encodeToByteArray(0x200000003L)) }
                val handle = rig.core.construct(5u, 6u, NO_BYTES)
                assertEq(true, events.holdsObjects())
                rig.core.release(handle)
                assertEq(false, events.holdsObjects())
            }
        }

        case("a schema mismatch is final: closed with its reason, once, and every call fails") {
            Rig().use { rig ->
                rig.t.drop()
                val mismatch = UndraSchemaMismatchException(HASH, 0x77uL)
                rig.t.events.onClosed(mismatch)
                assertEq(listOf("connecting", "connected", "reconnecting 1", "closed:SCHEMA_MISMATCH"), rig.log())
                val state = rig.core.connectionState.value as ConnectionState.Closed
                assertTrue(state.cause === mismatch)
                val e = assertThrows<UndraTransportException> { rig.core.callSync(TARGET, METHOD, NO_BYTES) }
                assertTrue(e.cause === mismatch, "the cause is the mismatch: ${e.cause}")
                assertEq(UndraTransportException.Reason.CONNECTION_LOST, e.reason)
                assertTrue(UndraCallError.mapped(e) is UndraCallError.Unavailable)
            }
        }

        case("a lost session is final with its own reason") {
            Rig().use { rig ->
                rig.t.drop()
                rig.t.events.onClosed(UndraSessionLostException("session lost"))
                val state = rig.core.connectionState.value as ConnectionState.Closed
                assertEq(ClosedReason.SESSION_LOST, state.reason)
                assertTrue(state.cause is UndraSessionLostException)
                // After it every call is Unavailable, and the lost session maps there on its own as well.
                val e = assertThrows<UndraTransportException> { rig.core.callSync(TARGET, METHOD, NO_BYTES) }
                assertTrue(e.cause === state.cause)
                assertTrue(UndraCallError.mapped(e) is UndraCallError.Unavailable)
                assertTrue(UndraCallError.mapped(UndraSessionLostException()) is UndraCallError.Unavailable)
            }
        }

        case("a transport that gives up for another reason closes the core as failed, and close() is requested") {
            Rig().use { rig ->
                rig.t.events.onClosed(UndraException("protocol error"))
                assertEq(ClosedReason.FAILED, (rig.core.connectionState.value as ConnectionState.Closed).reason)
            }
            Rig().use { rig ->
                rig.t.drop()
                rig.core.close()
                assertEq(listOf("connecting", "connected", "reconnecting 1", "closed:REQUESTED"), rig.log())
                rig.core.close()
                assertEq(4, rig.states.size, "closing twice is not a second change")
            }
        }

        case("a loss that happens while the stores are being observed again does not announce a connection") {
            Rig().use { rig ->
                rig.core.observe(STORE, UInt.MAX_VALUE, true)
                val replaying = CountDownLatch(1)
                // The connection drops again in the middle of the replay: the second loss is the newer news.
                rig.t.onObserve = { _, _, _ ->
                    if (replaying.count == 1L) {
                        replaying.countDown()
                        rig.t.drop()
                    }
                }
                rig.t.drop()
                rig.t.reconnect()
                assertTrue(replaying.await(10, TimeUnit.SECONDS))
                Thread.sleep(300)
                assertTrue(rig.core.connectionState.value is ConnectionState.Reconnecting, "still reconnecting: ${rig.log()}")
                rig.t.onObserve = { _, _, _ -> }
                rig.t.reconnect()
                eventually("connected for real") { rig.core.connectionState.value == ConnectionState.Connected }
            }
        }

        case("a failing onConnectionChange does not break the core") {
            val t = FakeTransport(isSynchronous = false)
            val core = UndraCore.attach(
                t,
                LoadOptions(
                    mode = Mode.REMOTE,
                    remoteUrl = "ws://fake",
                    expectedSchemaHash = HASH,
                    defaultAdapters = false,
                    onConnectionChange = { error("a bug in the app") },
                ),
                makeShared = false,
            )
            LogCapture("dev.undra.runtime").use { log ->
                core.use {
                    t.drop()
                    t.reconnect()
                    eventually("connected") { core.connectionState.value == ConnectionState.Connected }
                }
                assertTrue(log.messages().any { it.contains("onConnectionChange failed") }, "the failure is logged, not thrown")
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
