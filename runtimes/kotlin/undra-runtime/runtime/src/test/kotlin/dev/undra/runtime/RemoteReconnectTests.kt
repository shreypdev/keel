package dev.undra.runtime

import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.WsTestServer
import dev.undra.runtime.support.changeSet
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.full
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Envelope
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.encodeToByteArray
import java.net.URI
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private val METHOD = 0x41u
private val TARGET = CallTarget.ObjectMethod(Handle(0x100000002L), METHOD)
private const val STORE = 0x100000002L

/** A sleeper that records each wait instead of waiting; while [hold] is set the reconnect loop stays in its sleep. */
private class RecordingSleeper : Sleeper {
    val waits = CopyOnWriteArrayList<Long>()

    @Volatile var hold: CountDownLatch? = null

    override fun sleep(millis: Long) {
        waits.add(millis)
        hold?.await()
    }
}

/**
 * A server that answers every connection's Hello with [hashFor] of the connection's number (1 for the first),
 * and everything else as [configure] says.
 */
private fun serve(
    server: WsTestServer,
    hashFor: (Int) -> ULong = { HASH },
    configure: (WsTestServer.Conn, Int) -> Unit = { _, _ -> },
) {
    val count = AtomicInteger()
    server.onConnect = { conn ->
        val n = count.incrementAndGet()
        conn.onMessage = { bytes ->
            val env = Envelope.decode(bytes)
            if (env.kind == Envelope.Kind.HELLO) conn.sendHello(hashFor(n))
        }
        configure(conn, n)
    }
}

private fun policy(max: Int = Int.MAX_VALUE) = ReconnectPolicy(random = { 0.0 }, maxAttempts = max)

private class Loaded(val core: UndraCore, val states: CopyOnWriteArrayList<ConnectionState>) : AutoCloseable {
    override fun close() = core.close()
}

private fun load(
    url: String,
    sleeper: Sleeper,
    reconnect: ReconnectPolicy? = policy(),
    timeout: kotlin.time.Duration = 5.seconds,
    pingAfterMillis: Long = 10_000L,
): Loaded {
    val states = CopyOnWriteArrayList<ConnectionState>()
    val core = UndraCore.attach(
        RemoteTransport(URI(url), timeout, reconnect, session = "tok-test", sleeper = sleeper, pingAfterMillis = pingAfterMillis),
        LoadOptions(
            mode = Mode.REMOTE,
            remoteUrl = url,
            expectedSchemaHash = HASH,
            defaultAdapters = false,
            remoteTimeout = timeout,
            onConnectionChange = { states.add(it) },
        ),
        makeShared = false,
    )
    return Loaded(core, states)
}

private fun short(state: ConnectionState): String = when (state) {
    ConnectionState.Connecting -> "connecting"
    ConnectionState.Connected -> "connected"
    is ConnectionState.Reconnecting -> "reconnecting ${state.attempt}"
    is ConnectionState.Closed -> "closed:${state.reason}"
}

/** The remote transport reconnecting over real sockets: the backoff, the session in the URL, what survives, what is final. */
class RemoteReconnectTests : Suite() {
    init {
        case("a dropped connection is reconnected, the stores are observed again and the mirror resyncs") {
            WsTestServer().use { server ->
                var value = 5u
                serve(server) { conn, _ ->
                    val before = conn.onMessage
                    conn.onMessage = { bytes ->
                        before(bytes)
                        val env = Envelope.decode(bytes)
                        if (env.kind == Envelope.Kind.OBSERVE) {
                            val observe = Payloads.Observe.decode(env.payload)
                            conn.send(Envelope.Kind.CHANGE_SET, changeSet(1uL, full(observe.handle.raw, 0u, Codecs.u32.encodeToByteArray(value))))
                        }
                    }
                }
                load(server.url, RecordingSleeper()).use { loaded ->
                    val seen = CopyOnWriteArrayList<UInt>()
                    loaded.core.mirror.register(STORE) { _, _, r -> seen.add(Codecs.u32.decode(r)) }
                    loaded.core.observe(STORE, UInt.MAX_VALUE, true)
                    eventually("the first value") { seen == listOf(5u) }
                    assertEq(listOf("connecting", "connected"), loaded.states.map(::short))

                    value = 50u
                    server.connections.first().drop()
                    eventually("reconnected") { loaded.states.map(::short).lastOrNull() == "connected" && loaded.states.size > 2 }
                    eventually("the mirror converged") { seen == listOf(5u, 50u) }
                    assertEq(listOf("connecting", "connected", "reconnecting 1", "connected"), loaded.states.map(::short))
                    val second = server.connections[1]
                    val observe = Payloads.Observe.decode(second.awaitEnvelope(Envelope.Kind.OBSERVE).payload)
                    assertEq(Payloads.Observe(Handle(STORE), UInt.MAX_VALUE, true), observe)
                }
            }
        }

        case("every connection carries the same session token, and asks to resume only when the core holds objects") {
            WsTestServer().use { server ->
                serve(server) { conn, n ->
                    val before = conn.onMessage
                    conn.onMessage = { bytes ->
                        before(bytes)
                        val env = Envelope.decode(bytes)
                        if (env.kind == Envelope.Kind.CALL) {
                            val call = Payloads.Call.decode(env.payload)
                            conn.send(Envelope.Kind.REPLY, Payloads.Reply(call.callId, ReplyStatus.OK, Codecs.handle.encodeToByteArray(0x200000003L)).toByteArray())
                        }
                    }
                }
                load(server.url, RecordingSleeper()).use { loaded ->
                    val first = server.awaitConnection()
                    assertEq("tok-test", first.query["undra_session"])
                    assertEq(null, first.query["undra_resume"])

                    server.connections.first().drop()
                    eventually("reconnected") { server.connections.size == 2 }
                    val second = server.connections[1]
                    assertEq("tok-test", second.query["undra_session"])
                    assertEq(null, second.query["undra_resume"], "it constructed nothing: nothing to resume")
                    eventually("connected") { loaded.states.last() == ConnectionState.Connected }

                    loaded.core.construct(1u, 2u, NO_BYTES)
                    second.drop()
                    eventually("reconnected again") { server.connections.size == 3 }
                    assertEq("tok-test", server.connections[2].query["undra_session"])
                    assertEq("1", server.connections[2].query["undra_resume"])
                }
            }
        }

        case("the backoff doubles from 250 ms to 5 s while the server is down, then the first attempt that finds it wins") {
            val probe = WsTestServer()
            val port = probe.port
            serve(probe)
            val sleeper = RecordingSleeper()
            load(probe.url, sleeper).use { loaded ->
                probe.close() // the server goes away: every connect is refused now
                eventually("seven waits") { sleeper.waits.size >= 7 }
                assertEq(listOf(250L, 500L, 1000L, 2000L, 4000L, 5000L, 5000L), sleeper.waits.take(7))
                assertTrue(loaded.states.map(::short).containsAll(listOf("reconnecting 1", "reconnecting 2", "reconnecting 7")), loaded.states.map(::short).toString())
                WsTestServer(port).use { back ->
                    serve(back)
                    eventually("connected again") { loaded.states.last() == ConnectionState.Connected }
                    assertTrue(back.connections.isNotEmpty())
                }
            }
        }

        case("the policy gives up after maxAttempts and closes the core as failed") {
            val probe = WsTestServer()
            serve(probe)
            val sleeper = RecordingSleeper()
            load(probe.url, sleeper, reconnect = policy(max = 3)).use { loaded ->
                probe.close()
                eventually("the core is closed") { loaded.states.last() is ConnectionState.Closed }
                assertEq(ClosedReason.FAILED, (loaded.states.last() as ConnectionState.Closed).reason)
                assertEq(listOf(250L, 500L, 1000L), sleeper.waits.toList())
                assertEq(listOf("connecting", "connected", "reconnecting 1", "reconnecting 2", "reconnecting 3", "closed:FAILED"), loaded.states.map(::short))
            }
        }

        case("a schema change on reconnect is the mismatch error, once, with no loop") {
            WsTestServer().use { server ->
                serve(server, hashFor = { n -> if (n == 1) HASH else 0x77uL })
                val sleeper = RecordingSleeper()
                load(server.url, sleeper).use { loaded ->
                    server.awaitConnection().drop()
                    eventually("closed") { loaded.states.last() is ConnectionState.Closed }
                    val closed = loaded.states.last() as ConnectionState.Closed
                    assertEq(ClosedReason.SCHEMA_MISMATCH, closed.reason)
                    val mismatch = closed.cause as UndraSchemaMismatchException
                    assertEq(HASH, mismatch.expected)
                    assertEq(0x77uL, mismatch.got)
                    Thread.sleep(300)
                    assertEq(1, loaded.states.count { it is ConnectionState.Closed }, "reported once")
                    assertEq(1, sleeper.waits.size, "one attempt, not a loop")
                    assertEq(2, server.connections.size)
                    assertThrows<UndraException> { loaded.core.callSync(TARGET, METHOD, NO_BYTES) }
                }
            }
        }

        case("a close with code 4001 right behind the Hello is a lost session, final and reported once") {
            WsTestServer().use { server ->
                serve(server) { conn, n -> if (n >= 2) conn.sendClose(4001, "session lost: no session tok-test") }
                load(server.url, RecordingSleeper()).use { loaded ->
                    server.awaitConnection().drop()
                    eventually("closed") { loaded.states.last() is ConnectionState.Closed }
                    val closed = loaded.states.last() as ConnectionState.Closed
                    assertEq(ClosedReason.SESSION_LOST, closed.reason)
                    assertTrue((closed.cause as UndraSessionLostException).message!!.contains("session lost"))
                    // The close usually beats the announcement of the connection; if it does not, the core was told
                    // `connected` for an instant before `closed`. Never a loop either way.
                    assertTrue(loaded.states.count { it == ConnectionState.Connected } <= 2, loaded.states.map(::short).toString())
                    assertEq(1, loaded.states.count { it is ConnectionState.Closed })
                }
            }
        }

        case("a close from the server (a rebuild: 1001) is reconnected like a drop") {
            WsTestServer().use { server ->
                serve(server)
                load(server.url, RecordingSleeper()).use { loaded ->
                    server.awaitConnection().sendClose(1001)
                    eventually("reconnected") { server.connections.size == 2 && loaded.states.last() == ConnectionState.Connected }
                    assertTrue(loaded.states.map(::short).contains("reconnecting 1"))
                }
            }
        }

        case("a text frame is a protocol error: final, not retried") {
            WsTestServer().use { server ->
                serve(server)
                val sleeper = RecordingSleeper()
                load(server.url, sleeper).use { loaded ->
                    server.awaitConnection().sendText("hello")
                    eventually("closed") { loaded.states.last() is ConnectionState.Closed }
                    assertEq(ClosedReason.FAILED, (loaded.states.last() as ConnectionState.Closed).reason)
                    assertTrue(loaded.states.last().let { (it as ConnectionState.Closed).cause!!.message!!.contains("protocol error") })
                    assertEq(0, sleeper.waits.size)
                }
            }
        }

        case("a server that vanished without a FIN is noticed by the client's own ping, and reconnected") {
            WsTestServer().use { server ->
                serve(server)
                load(server.url, RecordingSleeper(), pingAfterMillis = 200).use { loaded ->
                    server.awaitConnection().answerPings = false
                    eventually("the loss is noticed", timeoutMs = 5_000) { loaded.states.map(::short).contains("reconnecting 1") }
                    eventually("and healed") { server.connections.size == 2 && loaded.states.last() == ConnectionState.Connected }
                }
            }
        }

        case("close() during the backoff ends the reconnecting for good") {
            WsTestServer().use { server ->
                serve(server)
                val sleeper = RecordingSleeper().also { it.hold = CountDownLatch(1) }
                val loaded = load(server.url, sleeper)
                server.awaitConnection().drop()
                eventually("it is waiting to retry") { sleeper.waits.size == 1 }
                loaded.core.close()
                Thread.sleep(300)
                sleeper.hold!!.countDown()
                Thread.sleep(300)
                assertEq(1, server.connections.size, "no attempt after close")
                assertEq(ClosedReason.REQUESTED, (loaded.states.last() as ConnectionState.Closed).reason)
                assertEq(1, loaded.states.count { it is ConnectionState.Closed })
            }
        }

        case("with no reconnect policy a drop closes the core as it always did") {
            WsTestServer().use { server ->
                serve(server)
                load(server.url, RecordingSleeper(), reconnect = null).use { loaded ->
                    server.awaitConnection().drop()
                    eventually("closed") { loaded.states.last() is ConnectionState.Closed }
                    assertEq(ClosedReason.FAILED, (loaded.states.last() as ConnectionState.Closed).reason)
                    assertEq(1, server.connections.size)
                }
            }
        }

        case("an initial connection that fails is still an exception from load, not a retry") {
            val port = java.net.ServerSocket(0).use { it.localPort }
            val sleeper = RecordingSleeper()
            val e = assertThrows<UndraException> { load("ws://127.0.0.1:$port", sleeper, timeout = 2.seconds) }
            assertTrue(e.message!!.contains("ws://127.0.0.1:$port"), e.message!!)
            assertEq(0, sleeper.waits.size)
        }

        case("load works from a thread that must not touch the network: no I/O on the calling thread") {
            WsTestServer().use { server ->
                serve(server)
                val callers = CopyOnWriteArrayList<String>()
                val threadNames = CopyOnWriteArrayList<String>()
                load(server.url, RecordingSleeper()).use { loaded ->
                    callers.add(Thread.currentThread().name)
                    runBlocking { loaded.core.observe(STORE, UInt.MAX_VALUE, true) }
                    server.awaitConnection().awaitEnvelope(Envelope.Kind.OBSERVE)
                    for (t in Thread.getAllStackTraces().keys) threadNames.add(t.name)
                    assertTrue(threadNames.contains("undra-ws-reader") && threadNames.contains("undra-ws-writer"), threadNames.toString())
                }
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
