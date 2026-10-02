package dev.undra.runtime

import dev.undra.runtime.adapters.DbAdapter
import dev.undra.runtime.adapters.DbConnection
import dev.undra.runtime.adapters.DbConstraint
import dev.undra.runtime.adapters.DbError
import dev.undra.runtime.adapters.DbExecuted
import dev.undra.runtime.adapters.DbMigration
import dev.undra.runtime.adapters.DbOpened
import dev.undra.runtime.adapters.DbPortAdapter
import dev.undra.runtime.adapters.DbRows
import dev.undra.runtime.adapters.DbValue
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.SseError
import dev.undra.runtime.adapters.SseEvent
import dev.undra.runtime.adapters.SsePortAdapter
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.WebSocketPortAdapter
import dev.undra.runtime.adapters.WsError
import dev.undra.runtime.adapters.WsMessage
import dev.undra.runtime.adapters.WsOpened
import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.ScriptedDb
import dev.undra.runtime.support.ScriptedSse
import dev.undra.runtime.support.ScriptedWebSocket
import dev.undra.runtime.adapters.dbPort
import dev.undra.runtime.adapters.ssePort
import dev.undra.runtime.adapters.webSocketPort
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.attach
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.handledBy
import dev.undra.runtime.support.portMethods
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.fail
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Payloads.PortReply
import dev.undra.runtime.wire.Payloads.PortStatus
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.decodeAll
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.logging.Level
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.jupiter.api.Test

/** Runs [block], which must throw exactly [expected]. */
private suspend fun expectError(expected: Throwable, block: suspend () -> Unit) {
    try {
        block()
    } catch (e: Throwable) {
        assertEq(expected, e)
        return
    }
    fail("expected $expected but nothing was thrown")
}

/** Runs [block], which must throw an [E]; returns it. */
private suspend inline fun <reified E : Throwable> expectThrows(crossinline block: suspend () -> Unit): E {
    try {
        block()
    } catch (e: Throwable) {
        if (e is E) return e
        fail("expected ${E::class.java.simpleName} but got $e")
    }
    fail("expected ${E::class.java.simpleName} but nothing was thrown")
}

private fun <T> blocking(block: suspend kotlinx.coroutines.CoroutineScope.() -> T): T = runBlocking { withTimeout(20_000) { block() } }

/** Calls port method [method] of [impl] with [args] and returns the reply body, or the typed error it answered. */
private fun <E : Throwable> portCall(impl: PortImpl, method: UInt, errors: UndraCodec<E>, args: UndraWriter.() -> Unit): Result<ByteArray> =
    runBlocking {
        try {
            Result.success(impl.methods.getValue(method)(UndraWriter().also(args).toByteArray()))
        } catch (e: UndraPortException) {
            Result.failure(errors.decodeAll(e.body))
        }
    }

private fun text(vararg values: String): List<WsMessage> = values.map { WsMessage.Text(it) }

private fun event(data: String, id: String? = null): SseEvent = SseEvent(id, "message", data, null)

/** Calls [methodId] of [portId] through [t]'s core, as the core would, and returns the `PortReply` the host answered with. */
private fun answer(t: FakeTransport, portId: UInt, methodId: UInt, args: ByteArray): PortReply {
    val callId = nextPortCallId.getAndIncrement().toUInt()
    assertEq(PortOutcome.Async, t.portCall(portId, methodId, callId, args), "the opt-in ports are async")
    var found: ByteArray? = null
    eventually("the reply to port call $callId") {
        found = t.portReplyBytes.firstOrNull { UndraReader(it).readU32() == callId }
        found != null
    }
    return PortReply.decode(found!!)
}

private val nextPortCallId = AtomicInteger(1)

private fun args(fill: UndraWriter.() -> Unit): ByteArray = UndraWriter().also(fill).toByteArray()

/** The bindings of the opt-in ports (ADR-047, ADR-048) over scripted adapters: ids, the pull, ends, transactions. */
class PortsV2BindingTests : Suite() {
    init {
        // ---- WebSocket -------------------------------------------------------------------------------------------------

        case("WebSocket: connect numbers connections from 1, never reusing an id, and answers the subprotocol") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                assertEq(WsOpened(1u, "v2"), port.connect("ws://a.test/x", listOf("v2", "v1"), listOf(Header("X-Token", "t"))))
                port.close(1u, 1000u, "")
                assertEq(WsOpened(2u, ""), port.connect("WSS://b.test", emptyList(), emptyList()))
                assertEq(listOf("v2", "v1"), ws.connections[0].protocols)
                assertEq(listOf(Header("X-Token", "t")), ws.connections[0].headers)
            }
        }

        case("WebSocket: a URL that is not ws:// or wss:// is refused before the adapter is asked") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                expectError(WsError.Refused(null, "invalid URL: http://a.test")) { port.connect("http://a.test", emptyList(), emptyList()) }
                expectError(WsError.Refused(null, "invalid URL: ")) { port.connect("", emptyList(), emptyList()) }
            }
            assertEq(0, ws.connections.size)
        }

        case("WebSocket: the adapter's typed failure is the port's; anything else it throws is Network") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                ws.failConnect = WsError.Refused(401u, "no")
                expectError(WsError.Refused(401u, "no")) { port.connect("ws://a.test", emptyList(), emptyList()) }
                ws.failConnect = IllegalStateException("socket factory broke")
                expectError(WsError.Network("socket factory broke")) { port.connect("ws://a.test", emptyList(), emptyList()) }
                ws.failConnect = null
                assertEq(1u, port.connect("ws://a.test", emptyList(), emptyList()).conn, "failed connects take no id")
            }
        }

        case("WebSocket: the pump reads ahead 16 before the first receive, then the window of the latest receive") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                val conn = port.connect("ws://a.test", emptyList(), emptyList()).conn
                val inbound = ws.connections[0].inbound
                for (i in 0 until 100) inbound.push(WsMessage.Text("$i"))
                eventually("the pump took its window") { inbound.taken == 16 }
                delay(100)
                assertEq(16, inbound.taken, "no read beyond the window")
                assertEq(text("0", "1", "2", "3", "4"), port.receive(conn, 5u))
                delay(100)
                assertEq(16, inbound.taken, "11 still buffered, at least the window of 5: the pump waits")
                assertEq(text("5", "6", "7", "8", "9"), port.receive(conn, 5u))
                assertEq((10 until 15).map { WsMessage.Text("$it") }, port.receive(conn, 5u))
                // One left in the buffer, below the window of 5: the pump takes up to 5.
                eventually("the pump refilled to the window") { inbound.taken == 20 }
                val rest = ArrayList<WsMessage>()
                while (rest.size < 85) rest.addAll(port.receive(conn, 1000u))
                assertEq((15 until 100).map { WsMessage.Text("$it") }, rest)
            }
        }

        case("WebSocket: receive answers at once with what is buffered, at most max, and waits when there is nothing") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                val conn = port.connect("ws://a.test", emptyList(), emptyList()).conn
                val inbound = ws.connections[0].inbound
                val waiting = async { port.receive(conn, 16u) }
                delay(50)
                assertTrue(!waiting.isCompleted, "nothing buffered: the receive waits")
                inbound.push(WsMessage.Binary(byteArrayOf(1, 2)))
                assertEq(listOf<WsMessage>(WsMessage.Binary(byteArrayOf(1, 2))), waiting.await())
                inbound.push(WsMessage.Text("a"))
                inbound.push(WsMessage.Text("b"))
                eventually("both buffered") { inbound.taken == 3 }
                assertEq(text("a"), port.receive(conn, 1u))
                assertEq(text("b"), port.receive(conn, 0u), "a max of 0 is read as 1")
            }
        }

        case("WebSocket: a burst is answered as one reply; a lone message after 2 ms of quiet; a trickle within about 8 ms") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                val conn = port.connect("ws://a.test", emptyList(), emptyList()).conn
                val inbound = ws.connections[0].inbound
                // A burst arriving over a few milliseconds: one reply.
                val burst = async {
                    for (i in 0 until 6) {
                        inbound.push(WsMessage.Text("$i"))
                        delay(1)
                    }
                }
                val first = port.receive(conn, 16u)
                burst.await()
                assertTrue(first.size >= 3, "a burst comes back as one reply, got ${first.size}")
                val rest = ArrayList(first)
                while (rest.size < 6) rest.addAll(port.receive(conn, 16u))
                assertEq((0 until 6).map { WsMessage.Text("$it") }, rest)
                // A lone message: answered once nothing more came for 2 ms.
                inbound.push(WsMessage.Text("lone"))
                val started = System.nanoTime()
                assertEq(text("lone"), port.receive(conn, 16u))
                assertTrue((System.nanoTime() - started) / 1_000_000 < 500, "a lone message is not held back")
                // A trickle that never pauses for 2 ms: answered about 8 ms after the first message, not when 16 are there.
                val trickle = async {
                    for (i in 0 until 60) {
                        inbound.push(WsMessage.Text("t$i"))
                        delay(1)
                    }
                }
                val part = port.receive(conn, 16u)
                assertTrue(part.isNotEmpty() && part.size < 60, "a trickle is answered in parts: ${part.size}")
                trickle.await()
                // The pump has filled the window meanwhile: max items are answered at once.
                eventually("the window filled") { inbound.taken >= 7 + part.size + 16 }
                assertEq(16, port.receive(conn, 16u).size)
            }
        }

        case("WebSocket: one receive per connection at a time") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                val conn = port.connect("ws://a.test", emptyList(), emptyList()).conn
                val first = async { port.receive(conn, 16u) }
                delay(50)
                expectError(WsError.Protocol("a receive is already pending on connection 1")) { port.receive(conn, 16u) }
                ws.connections[0].inbound.push(WsMessage.Text("x"))
                assertEq(text("x"), first.await())
            }
        }

        case("WebSocket: the connection's end comes after what was buffered, and stays") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                val conn = port.connect("ws://a.test", emptyList(), emptyList()).conn
                val inbound = ws.connections[0].inbound
                inbound.push(WsMessage.Text("hello"))
                inbound.fail(WsError.Closed(4001u, "kicked"))
                assertEq(text("hello"), port.receive(conn, 16u))
                expectError(WsError.Closed(4001u, "kicked")) { port.receive(conn, 16u) }
                expectError(WsError.Closed(4001u, "kicked")) { port.receive(conn, 16u) }
                expectError(WsError.Closed(4001u, "kicked")) { port.send(conn, WsMessage.Text("late")) }
                assertEq(0, ws.connections[0].sent.size)
            }
        }

        case("WebSocket: a stream that completes on its own ends with Network; one that throws something else, Network with its text") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                val a = port.connect("ws://a.test", emptyList(), emptyList()).conn
                ws.connections[0].inbound.finish()
                expectError(WsError.Network("the connection ended")) { port.receive(a, 16u) }
                val b = port.connect("ws://a.test", emptyList(), emptyList()).conn
                ws.connections[1].inbound.fail(IllegalStateException("decoder broke"))
                expectError(WsError.Network("decoder broke")) { port.receive(b, 16u) }
            }
        }

        case("WebSocket: send goes to the adapter; its typed failure is the port's") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                val conn = port.connect("ws://a.test", emptyList(), emptyList()).conn
                port.send(conn, WsMessage.Text("a"))
                port.send(conn, WsMessage.Binary(byteArrayOf(9)))
                assertEq(listOf(WsMessage.Text("a"), WsMessage.Binary(byteArrayOf(9))), ws.connections[0].sent.toList())
                ws.connections[0].failSend = WsError.Network("reset")
                expectError(WsError.Network("reset")) { port.send(conn, WsMessage.Text("b")) }
                ws.connections[0].failSend = java.io.IOException("broken pipe")
                expectError(WsError.Network("broken pipe")) { port.send(conn, WsMessage.Text("c")) }
            }
        }

        case("WebSocket: close answers a waiting receive with [], drops the buffer, and later calls see the core's close") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            blocking {
                val conn = port.connect("ws://a.test", emptyList(), emptyList()).conn
                val waiting = async { port.receive(conn, 16u) }
                delay(50)
                port.close(conn, 4000u, "bye")
                assertEq(emptyList<WsMessage>(), waiting.await())
                assertEq(listOf(4000 to "bye"), ws.connections[0].closes.toList())
                assertEq(emptyList<WsMessage>(), port.receive(conn, 16u))
                expectError(WsError.Closed(4000u, "bye")) { port.send(conn, WsMessage.Text("x")) }
                port.close(conn, 1000u, "again")
                assertEq(1, ws.connections[0].closes.size, "a second close is fine and does nothing")
                assertEq(0, port.openConnections)

                val other = port.connect("ws://a.test", emptyList(), emptyList()).conn
                ws.connections[1].inbound.push(WsMessage.Text("unread"))
                eventually("buffered") { ws.connections[1].inbound.taken == 1 }
                port.close(other, 1000u, "")
                assertEq(emptyList<WsMessage>(), port.receive(other, 16u), "what was buffered is dropped")
            }
        }

        case("WebSocket: an unknown connection is Network") {
            val port = WebSocketPortAdapter(ScriptedWebSocket())
            blocking {
                expectError(WsError.Network("no WebSocket connection 7")) { port.receive(7u, 16u) }
                expectError(WsError.Network("no WebSocket connection 7")) { port.send(7u, WsMessage.Text("x")) }
                expectError(WsError.Network("no WebSocket connection 7")) { port.close(7u, 1000u, "") }
            }
        }

        case("WebSocket: detaching (the core closes) closes every open connection with 1001") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            val impl = port.portImpl()
            blocking {
                val a = port.connect("ws://a.test", emptyList(), emptyList()).conn
                port.connect("ws://b.test", emptyList(), emptyList())
                port.close(a, 1000u, "done")
            }
            impl.detach!!.invoke()
            eventually("the open one closed going away") { ws.connections[1].closes.toList() == listOf(1001 to "") }
            assertEq(listOf(1000 to "done"), ws.connections[0].closes.toList(), "the closed one is not closed again")
            assertEq(0, port.openConnections)
        }

        case("WebSocket: a connect in flight when the core detaches is closed going away once it opens") {
            val ws = ScriptedWebSocket()
            val handshake = CompletableDeferred<Unit>()
            ws.connectGate = handshake
            val port = WebSocketPortAdapter(ws)
            val impl = port.portImpl()
            val opened = blocking {
                val opening = async { port.connect("ws://late.test", emptyList(), emptyList()) }
                while (ws.connectsStarted.get() == 0) delay(1)
                // The core closes while the handshake is in flight: the detach cannot see the connection yet.
                impl.detach!!.invoke()
                handshake.complete(Unit)
                opening.await()
            }
            eventually("the late connection closed going away") { ws.connections.single().closes.toList() == listOf(1001 to "") }
            assertEq(0, port.openConnections)
            blocking { assertEq(emptyList<WsMessage>(), port.receive(opened.conn, 16u)) }
        }

        case("WebSocket: a core that closes detaches its ports, and a registration that replaces one detaches it") {
            val ws = ScriptedWebSocket()
            val port = WebSocketPortAdapter(ws)
            val t = FakeTransport()
            val core = attach(t, adapters = mapOf(StandardPorts.WebSocket.PORT_ID to port.portImpl()))
            blocking { port.connect("ws://a.test", emptyList(), emptyList()) }
            core.close()
            eventually("closed with 1001") { ws.connections[0].closes.toList() == listOf(1001 to "") }

            val second = ScriptedWebSocket()
            val replaced = WebSocketPortAdapter(second)
            val core2 = attach(FakeTransport(), adapters = mapOf(StandardPorts.WebSocket.PORT_ID to replaced.portImpl()))
            blocking { replaced.connect("ws://a.test", emptyList(), emptyList()) }
            core2.registerPort(StandardPorts.WebSocket.PORT_ID, WebSocketPortAdapter(ScriptedWebSocket()).portImpl())
            eventually("the replaced binding closed its connection") { second.connections[0].closes.toList() == listOf(1001 to "") }
            core2.close()
        }

        case("through the port registry: typed errors are status 1, a raw WsError, SseError or DbError from its own port too; others are status 2") {
            // The bindings answer their typed errors as UndraPortExceptions.
            val t = FakeTransport()
            attach(
                t,
                adapters = mapOf(
                    StandardPorts.WebSocket.PORT_ID to webSocketPort(ScriptedWebSocket()),
                    StandardPorts.Sse.PORT_ID to ssePort(ScriptedSse()),
                    StandardPorts.Db.PORT_ID to dbPort(ScriptedDb()),
                ),
            ).use {
                val receive = answer(t, StandardPorts.WebSocket.PORT_ID, StandardPorts.WebSocket.RECEIVE, args { writeU32(7u); writeU32(16u) })
                assertEq(PortStatus.ERROR, receive.status)
                assertEq(WsError.Network("no WebSocket connection 7"), WsError.decodeAll(receive.body))
                val next = answer(t, StandardPorts.Sse.PORT_ID, StandardPorts.Sse.NEXT, args { writeU32(9u); writeU32(16u) })
                assertEq(PortStatus.ERROR, next.status)
                assertEq(SseError.Network("no event stream 9"), SseError.decodeAll(next.body))
                val query = answer(t, StandardPorts.Db.PORT_ID, StandardPorts.Db.QUERY, args { writeU32(99u); writeStr("SELECT 1"); Codecs.vec(DbValue).encode(this, emptyList()) })
                assertEq(PortStatus.ERROR, query.status)
                assertEq(DbError.Unavailable("no open database or transaction 99"), DbError.decodeAll(query.body))
                // Arguments that do not decode are a bug, not the port's error: unavailable.
                assertEq(PortStatus.UNAVAILABLE, answer(t, StandardPorts.WebSocket.PORT_ID, StandardPorts.WebSocket.RECEIVE, args { writeU32(7u) }).status)
            }

            // A hand-written implementation that throws the port's own error type is answered like Kv's StorageError (ADR-049).
            val raw = FakeTransport()
            LogCapture("dev.undra.runtime").use { log ->
                attach(
                    raw,
                    adapters = mapOf(
                        StandardPorts.WebSocket.PORT_ID to PortImpl(false, portMethods(StandardPorts.WebSocket.SEND handledBy { _: ByteArray -> throw WsError.Closed(1000u, "x") }, StandardPorts.WebSocket.CLOSE handledBy { _: ByteArray -> throw DbError.Busy })),
                        StandardPorts.Sse.PORT_ID to PortImpl(false, portMethods(StandardPorts.Sse.NEXT handledBy { _: ByteArray -> throw SseError.Ended })),
                        StandardPorts.Db.PORT_ID to PortImpl(false, portMethods(StandardPorts.Db.QUERY handledBy { _: ByteArray -> throw DbError.Busy })),
                    ),
                ).use {
                    val send = answer(raw, StandardPorts.WebSocket.PORT_ID, StandardPorts.WebSocket.SEND, ByteArray(0))
                    assertEq(PortStatus.ERROR, send.status, "WsError from WebSocket")
                    assertEq(WsError.Closed(1000u, "x"), WsError.decodeAll(send.body))
                    val next = answer(raw, StandardPorts.Sse.PORT_ID, StandardPorts.Sse.NEXT, ByteArray(0))
                    assertEq(PortStatus.ERROR, next.status, "SseError from Sse")
                    assertEq(SseError.Ended, SseError.decodeAll(next.body))
                    val query = answer(raw, StandardPorts.Db.PORT_ID, StandardPorts.Db.QUERY, ByteArray(0))
                    assertEq(PortStatus.ERROR, query.status, "DbError from Db")
                    assertEq(DbError.Busy, DbError.decodeAll(query.body))
                    // Another port's error type would not decode as this port's error in the core.
                    assertEq(PortStatus.UNAVAILABLE, answer(raw, StandardPorts.WebSocket.PORT_ID, StandardPorts.WebSocket.CLOSE, ByteArray(0)).status, "DbError from WebSocket")
                    eventually("one ERROR record") { log.records.count { it.level == Level.SEVERE } == 1 }
                    val message = log.records.single { it.level == Level.SEVERE }.message
                    assertTrue(message.contains("the WebSocket.close port method"), message)
                }
            }
        }

        case("WebSocket: the port methods decode their arguments and answer typed errors as the port's error body") {
            val ws = ScriptedWebSocket()
            val impl = WebSocketPortAdapter(ws).portImpl()
            assertTrue(!impl.sync)
            val opened = portCall(impl, StandardPorts.WebSocket.CONNECT, WsError) {
                writeStr("ws://a.test")
                Codecs.vec(Codecs.string).encode(this, listOf("chat"))
                Codecs.vec(Header).encode(this, emptyList())
            }
            assertEq(WsOpened(1u, "chat"), WsOpened.decodeAll(opened.getOrThrow()))
            ws.connections[0].inbound.push(WsMessage.Text("hi"))
            val received = portCall(impl, StandardPorts.WebSocket.RECEIVE, WsError) {
                writeU32(1u)
                writeU32(16u)
            }
            assertEq(text("hi"), Codecs.vec(WsMessage).decodeAll(received.getOrThrow()))
            val sent = portCall(impl, StandardPorts.WebSocket.SEND, WsError) {
                writeU32(1u)
                WsMessage.encode(this, WsMessage.Text("yo"))
            }
            assertEq(0, sent.getOrThrow().size)
            val closed = portCall(impl, StandardPorts.WebSocket.CLOSE, WsError) {
                writeU32(1u)
                writeU16(1000u)
                writeStr("done")
            }
            assertEq(0, closed.getOrThrow().size)
            val refused = portCall(impl, StandardPorts.WebSocket.CONNECT, WsError) {
                writeStr("ftp://x")
                Codecs.vec(Codecs.string).encode(this, emptyList())
                Codecs.vec(Header).encode(this, emptyList())
            }
            assertEq(WsError.Refused(null, "invalid URL: ftp://x"), refused.exceptionOrNull())
        }

        // ---- Sse --------------------------------------------------------------------------------------------------------

        case("Sse: open numbers streams from 1, carries the headers and Last-Event-ID, and refuses non-HTTP URLs") {
            val sse = ScriptedSse()
            val port = SsePortAdapter(sse)
            blocking {
                assertEq(1u, port.open("https://feed.test", listOf(Header("A", "b")), "41"))
                assertEq(2u, port.open("http://feed.test", emptyList(), null))
                assertEq("41", sse.streams[0].lastEventId)
                assertEq(listOf(Header("A", "b")), sse.streams[0].headers)
                expectError(SseError.Refused(null, "invalid URL: ws://feed.test")) { port.open("ws://feed.test", emptyList(), null) }
                sse.failOpen = SseError.Refused(204u, "stop")
                expectError(SseError.Refused(204u, "stop")) { port.open("http://feed.test", emptyList(), null) }
                sse.failOpen = RuntimeException("dns")
                expectError(SseError.Network("dns")) { port.open("http://feed.test", emptyList(), null) }
            }
        }

        case("Sse: next pulls with the same discipline, and the body's end comes after the events") {
            val sse = ScriptedSse()
            val port = SsePortAdapter(sse)
            blocking {
                val id = port.open("http://feed.test", emptyList(), null)
                val inbound = sse.streams[0].inbound
                for (i in 0 until 40) inbound.push(event("$i", "$i"))
                inbound.fail(SseError.Ended)
                eventually("the window") { inbound.taken == 16 }
                delay(50)
                assertEq(16, inbound.taken)
                assertEq(listOf(event("0", "0"), event("1", "1")), port.next(id, 2u))
                val rest = ArrayList<SseEvent>()
                while (rest.size < 38) rest.addAll(port.next(id, 16u))
                assertEq((2 until 40).map { event("$it", "$it") }, rest)
                expectError(SseError.Ended) { port.next(id, 16u) }
                expectError(SseError.Ended) { port.next(id, 16u) }
            }
        }

        case("Sse: a stream that completes on its own has Ended; one pending next; close answers it with []") {
            val sse = ScriptedSse()
            val port = SsePortAdapter(sse)
            blocking {
                val a = port.open("http://feed.test", emptyList(), null)
                sse.streams[0].inbound.finish()
                expectError(SseError.Ended) { port.next(a, 16u) }
                val b = port.open("http://feed.test", emptyList(), null)
                val waiting = async { port.next(b, 16u) }
                delay(50)
                expectError(SseError.Protocol("a next is already pending on stream 2")) { port.next(b, 16u) }
                port.close(b)
                assertEq(emptyList<SseEvent>(), waiting.await())
                assertEq(1, sse.streams[1].closed)
                port.close(b)
                assertEq(1, sse.streams[1].closed, "closing again is fine")
                assertEq(emptyList<SseEvent>(), port.next(b, 16u))
                expectError(SseError.Network("no event stream 9")) { port.next(9u, 16u) }
                expectError(SseError.Network("no event stream 9")) { port.close(9u) }
            }
        }

        case("Sse: detaching closes every open stream") {
            val sse = ScriptedSse()
            val port = SsePortAdapter(sse)
            blocking {
                port.open("http://a.test", emptyList(), null)
                port.open("http://b.test", emptyList(), null)
            }
            port.portImpl().detach!!.invoke()
            eventually("both closed") { sse.streams.all { it.closed == 1 } }
            assertEq(0, port.openStreams)
        }

        case("Sse: an open in flight when the core detaches is closed once it opens") {
            val sse = ScriptedSse()
            val request = CompletableDeferred<Unit>()
            sse.openGate = request
            val port = SsePortAdapter(sse)
            val impl = port.portImpl()
            val id = blocking {
                val opening = async { port.open("http://late.test", emptyList(), null) }
                while (sse.opensStarted.get() == 0) delay(1)
                impl.detach!!.invoke()
                request.complete(Unit)
                opening.await()
            }
            eventually("the late stream closed") { sse.streams.single().closed == 1 }
            assertEq(0, port.openStreams)
            blocking { assertEq(emptyList<SseEvent>(), port.next(id, 16u)) }
        }

        case("Sse: the port methods speak bytes") {
            val sse = ScriptedSse()
            val impl = SsePortAdapter(sse).portImpl()
            val opened = portCall(impl, StandardPorts.Sse.OPEN, SseError) {
                writeStr("http://feed.test")
                Codecs.vec(Header).encode(this, emptyList())
                Codecs.option(Codecs.string).encode(this, "2")
            }
            assertEq(1u, Codecs.u32.decodeAll(opened.getOrThrow()))
            sse.streams[0].inbound.push(SseEvent("3", "tick", "x", 1500u))
            val next = portCall(impl, StandardPorts.Sse.NEXT, SseError) {
                writeU32(1u)
                writeU32(16u)
            }
            assertEq(listOf(SseEvent("3", "tick", "x", 1500u)), Codecs.vec(SseEvent).decodeAll(next.getOrThrow()))
            sse.streams[0].inbound.fail(SseError.Ended)
            val ended = portCall(impl, StandardPorts.Sse.NEXT, SseError) {
                writeU32(1u)
                writeU32(16u)
            }
            assertEq(SseError.Ended, ended.exceptionOrNull())
            assertEq(0, portCall(impl, StandardPorts.Sse.CLOSE, SseError) { writeU32(1u) }.getOrThrow().size)
        }

        // ---- Db ---------------------------------------------------------------------------------------------------------

        case("Db: open sets the pragmas, runs pending migrations in one BEGIN IMMEDIATE, and answers the version") {
            val db = ScriptedDb()
            val port = DbPortAdapter(db)
            blocking {
                val migrations = listOf(DbMigration(1u, "CREATE TABLE a (x)"), DbMigration(2u, "CREATE TABLE b (y); CREATE INDEX i ON b (y)"))
                assertEq(DbOpened(1u, 2u), port.open("app", migrations))
                val sequence = db.calls.map { "${it.kind} ${it.sql}" }
                assertEq(
                    listOf(
                        "query PRAGMA foreign_keys = ON", "query PRAGMA busy_timeout = 5000", "query PRAGMA journal_mode = WAL",
                        "query PRAGMA user_version", "execute BEGIN IMMEDIATE", "script CREATE TABLE a (x)",
                        "script CREATE TABLE b (y); CREATE INDEX i ON b (y)", "execute PRAGMA user_version = 2", "execute COMMIT",
                    ),
                    sequence,
                )
                db.calls.clear()
                val three = migrations + DbMigration(3u, "CREATE TABLE c (z)")
                assertEq(DbOpened(2u, 3u), port.open("app", three), "a second open of the same name runs only what is new")
                assertEq(listOf("CREATE TABLE c (z)"), db.calls("script").map { it.sql })
                db.calls.clear()
                assertEq(DbOpened(3u, 0u), port.open(":memory:", emptyList()))
                assertTrue(db.calls.none { it.sql.contains("journal_mode") }, "no WAL for an in-memory database")
                assertTrue(db.calls.none { it.sql == "BEGIN IMMEDIATE" }, "nothing to migrate, no transaction")
            }
        }

        case("Db: names and migration versions are checked before the adapter is asked") {
            val db = ScriptedDb()
            val port = DbPortAdapter(db)
            blocking {
                for (bad in listOf("", ".hidden", "a/b", "a b", "..", "x".repeat(65), "é", "../escape")) {
                    val e = expectThrows<DbError.Unavailable> { port.open(bad, emptyList()) }
                    assertTrue(e.reason.startsWith("invalid database name \"$bad\""), e.reason)
                }
                for (good in listOf("app", "a.b_c-1", "x".repeat(64), ":memory:")) port.open(good, emptyList())
                expectError(DbError.Migration(0u, "migration versions must strictly increase, starting at 1")) {
                    port.open("v", listOf(DbMigration(0u, "")))
                }
                expectError(DbError.Migration(2u, "migration versions must strictly increase, starting at 1")) {
                    port.open("v", listOf(DbMigration(2u, ""), DbMigration(2u, "")))
                }
            }
            assertEq(4, db.opened.size, "only the valid names were opened")
        }

        case("Db: a database newer than the newest migration is refused and closed") {
            val db = ScriptedDb()
            db.versions["app"] = 3L
            val port = DbPortAdapter(db)
            blocking {
                expectError(DbError.Migration(3u, "the database is at version 3, newer than the newest migration (2)")) {
                    port.open("app", listOf(DbMigration(1u, "a"), DbMigration(2u, "b")))
                }
                assertTrue(db.opened.single().closed)
                assertEq(DbOpened(1u, 3u), port.open("app", emptyList()), "without migrations any version opens")
            }
        }

        case("Db: a failing migration rolls every pending one back and is Migration of its version") {
            val db = ScriptedDb()
            db.failOn("nowhere", DbError.Sql("no such table: nowhere"))
            val port = DbPortAdapter(db)
            blocking {
                expectError(DbError.Migration(2u, "SQL error: no such table: nowhere")) {
                    port.open("m", listOf(DbMigration(1u, "CREATE TABLE a (x)"), DbMigration(2u, "INSERT INTO nowhere VALUES (1)")))
                }
                val sequence = db.calls.map { "${it.kind} ${it.sql}" }
                assertEq("execute ROLLBACK", sequence[sequence.size - 2])
                assertEq("close ", sequence.last())
                assertTrue(sequence.none { it.contains("user_version =") }, "the version is not moved")
            }
        }

        case("Db: statements run on the database, with their parameters, one at a time") {
            val db = ScriptedDb()
            db.statementMillis = 5
            val port = DbPortAdapter(db)
            blocking {
                val id = port.open("app", emptyList()).db
                assertEq(DbExecuted(1uL, 42L), port.execute(id, "INSERT INTO t VALUES (?, ?)", listOf(DbValue.Integer(1), DbValue.Text("a"))))
                assertEq(DbRows(listOf("n"), listOf(listOf(DbValue.Integer(2)))), port.query(id, "SELECT ?, ?", listOf(DbValue.Null, DbValue.Blob(byteArrayOf(1)))))
                val all = (0 until 20).map { i -> async { port.execute(id, "UPDATE t SET x = $i", emptyList()) } }
                all.forEach { it.await() }
                assertTrue(!db.opened.single().overlapped, "the serial queue never overlaps two calls on a connection")
                assertEq(listOf(DbValue.Integer(1), DbValue.Text("a")), db.calls("execute").first { it.sql.startsWith("INSERT") }.params)
            }
        }

        case("Db: a transaction id runs inside it; the database id waits for it and is Busy after the busy timeout") {
            val db = ScriptedDb()
            val port = DbPortAdapter(db, busyTimeoutMillis = 200)
            blocking {
                val id = port.open("app", emptyList()).db
                val tx = port.begin(id)
                assertEq(2u, tx, "transactions share the counter of databases")
                port.execute(tx, "INSERT INTO t VALUES (1)", emptyList())
                val started = System.nanoTime()
                expectError(DbError.Busy) { port.execute(id, "INSERT INTO t VALUES (2)", emptyList()) }
                assertTrue((System.nanoTime() - started) / 1_000_000 >= 180, "it waited for the busy timeout")
                expectError(DbError.Busy) { port.begin(id) }
                // A statement that waits less than the busy timeout runs once the transaction ends.
                val waiting = async { port.query(id, "SELECT 1", emptyList()) }
                delay(50)
                assertTrue(!waiting.isCompleted)
                port.commit(tx)
                waiting.await()
                val sql = db.calls.map { it.sql }
                assertTrue(sql.indexOf("COMMIT") < sql.indexOf("SELECT 1"), "the outer statement ran after the commit")
                expectError(DbError.Unavailable("transaction 2 is over")) { port.execute(tx, "SELECT 1", emptyList()) }
                expectError(DbError.Unavailable("transaction 2 is over")) { port.commit(tx) }
                expectError(DbError.Sql("a transaction cannot begin inside a transaction")) { port.begin(port.begin(id)) }
            }
        }

        case("Db: rollback ends the transaction; a failed COMMIT is rolled back and ends it too") {
            val db = ScriptedDb()
            val port = DbPortAdapter(db)
            blocking {
                val id = port.open("app", emptyList()).db
                val tx = port.begin(id)
                port.rollback(tx)
                assertEq("ROLLBACK", db.calls.last().sql)
                db.failOn("COMMIT", DbError.Constraint(DbConstraint.FOREIGN_KEY, "FOREIGN KEY constraint failed"))
                val tx2 = port.begin(id)
                expectError(DbError.Constraint(DbConstraint.FOREIGN_KEY, "FOREIGN KEY constraint failed")) { port.commit(tx2) }
                assertEq(listOf("COMMIT", "ROLLBACK"), db.calls.takeLast(2).map { it.sql })
                expectError(DbError.Unavailable("transaction $tx2 is over")) { port.rollback(tx2) }
                port.execute(id, "SELECT 1", emptyList())
            }
        }

        case("Db: close rolls back a running transaction; again is fine; statements afterwards are Unavailable") {
            val db = ScriptedDb()
            val port = DbPortAdapter(db)
            blocking {
                val id = port.open("app", emptyList()).db
                val tx = port.begin(id)
                port.close(id)
                assertEq(listOf("ROLLBACK", ""), db.calls.takeLast(2).map { it.sql })
                assertTrue(db.opened.single().closed)
                port.close(id)
                expectError(DbError.Unavailable("no open database or transaction 1")) { port.execute(id, "SELECT 1", emptyList()) }
                expectError(DbError.Unavailable("transaction 2 is over")) { port.execute(tx, "SELECT 1", emptyList()) }
                expectError(DbError.Unavailable("no open database or transaction 99")) { port.query(99u, "SELECT 1", emptyList()) }
                expectError(DbError.Unavailable("no open database or transaction 99")) { port.close(99u) }
                expectError(DbError.Unavailable("no open database or transaction 99")) { port.begin(99u) }
            }
        }

        case("Db: a statement waiting for a transaction is Unavailable when the database closes") {
            val db = ScriptedDb()
            val port = DbPortAdapter(db)
            blocking {
                val id = port.open("app", emptyList()).db
                port.begin(id)
                val waiting = async { runCatching { port.execute(id, "SELECT 1", emptyList()) } }
                delay(50)
                port.close(id)
                assertEq(DbError.Unavailable("no open database or transaction 1"), waiting.await().exceptionOrNull())
            }
        }

        case("Db: the adapter's typed errors pass through; anything else is Sql (Unavailable for open)") {
            val db = ScriptedDb()
            val port = DbPortAdapter(db)
            blocking {
                db.failOpen = DbError.Corrupt("file is not a database")
                expectError(DbError.Corrupt("file is not a database")) { port.open("x", emptyList()) }
                db.failOpen = IllegalStateException("no disk")
                expectError(DbError.Unavailable("no disk")) { port.open("x", emptyList()) }
                db.failOpen = null
                val id = port.open("x", emptyList()).db
                db.failOn("dup", DbError.Constraint(DbConstraint.UNIQUE, "UNIQUE constraint failed: t.id"))
                db.failOn("bug", IllegalArgumentException("index out of range"))
                expectError(DbError.Constraint(DbConstraint.UNIQUE, "UNIQUE constraint failed: t.id")) { port.execute(id, "INSERT dup", emptyList()) }
                expectError(DbError.Sql("index out of range")) { port.query(id, "SELECT bug", emptyList()) }
            }
        }

        case("Db: detaching closes every database, rolling back what runs") {
            val db = ScriptedDb()
            val port = DbPortAdapter(db)
            blocking {
                val a = port.open("a", emptyList()).db
                port.open("b", emptyList())
                port.begin(a)
            }
            port.portImpl().detach!!.invoke()
            eventually("both closed") { db.opened.all { it.closed } && port.openDatabases == 0 }
            assertTrue(db.calls.any { it.db == "a" && it.sql == "ROLLBACK" })
        }

        case("Db: an open in flight when the core detaches is closed once it opens") {
            val db = ScriptedDb()
            val file = CompletableDeferred<Unit>()
            db.openGate = file
            val port = DbPortAdapter(db)
            val impl = port.portImpl()
            val opened = blocking {
                val opening = async { port.open("late", emptyList()) }
                while (db.opensStarted.get() == 0) delay(1)
                impl.detach!!.invoke()
                file.complete(Unit)
                opening.await()
            }
            eventually("the late database closed") { db.opened.single().closed && port.openDatabases == 0 }
            blocking {
                expectError(DbError.Unavailable("no open database or transaction ${opened.db}")) { port.query(opened.db, "SELECT 1", emptyList()) }
            }
        }

        case("Db: an open cancelled while it prepares the database closes the connection it opened") {
            // Like the real adapters, this connection closes on a thread of its own: a close attempted from the cancelled
            // caller without care would not run at all, and the file would stay open.
            val closed = AtomicBoolean(false)
            val preparing = CompletableDeferred<Unit>()
            val adapter = object : DbAdapter {
                override suspend fun open(name: String): DbConnection = object : DbConnection {
                    override suspend fun execute(sql: String, params: List<DbValue>): DbExecuted = DbExecuted(0uL, 0L)

                    override suspend fun query(sql: String, params: List<DbValue>): DbRows {
                        preparing.complete(Unit)
                        awaitCancellation()
                    }

                    override suspend fun executeScript(sql: String) = Unit

                    override suspend fun close() {
                        withContext(Dispatchers.Default) { closed.set(true) }
                    }
                }
            }
            val port = DbPortAdapter(adapter)
            blocking {
                val opening = launch { port.open("x", emptyList()) }
                preparing.await()
                opening.cancelAndJoin()
            }
            assertTrue(closed.get(), "the connection of the cancelled open was closed")
            assertEq(0, port.openDatabases)
        }

        case("Db: the port methods speak bytes") {
            val db = ScriptedDb()
            val impl = DbPortAdapter(db).portImpl()
            val opened = portCall(impl, StandardPorts.Db.OPEN, DbError) {
                writeStr("app")
                Codecs.vec(DbMigration).encode(this, listOf(DbMigration(1u, "CREATE TABLE t (x)")))
            }
            assertEq(DbOpened(1u, 1u), DbOpened.decodeAll(opened.getOrThrow()))
            val executed = portCall(impl, StandardPorts.Db.EXECUTE, DbError) {
                writeU32(1u)
                writeStr("INSERT INTO t VALUES (?)")
                Codecs.vec(DbValue).encode(this, listOf(DbValue.Real(1.5)))
            }
            assertEq(DbExecuted(1uL, 42L), DbExecuted.decodeAll(executed.getOrThrow()))
            val rows = portCall(impl, StandardPorts.Db.QUERY, DbError) {
                writeU32(1u)
                writeStr("SELECT x FROM t")
                Codecs.vec(DbValue).encode(this, emptyList())
            }
            assertEq(DbRows(listOf("n"), listOf(listOf(DbValue.Integer(0)))), DbRows.decodeAll(rows.getOrThrow()))
            val tx = Codecs.u32.decodeAll(portCall(impl, StandardPorts.Db.BEGIN, DbError) { writeU32(1u) }.getOrThrow())
            assertEq(2u, tx)
            assertEq(0, portCall(impl, StandardPorts.Db.COMMIT, DbError) { writeU32(tx) }.getOrThrow().size)
            assertEq(DbError.Unavailable("transaction 2 is over"), portCall(impl, StandardPorts.Db.ROLLBACK, DbError) { writeU32(tx) }.exceptionOrNull())
            assertEq(0, portCall(impl, StandardPorts.Db.CLOSE, DbError) { writeU32(1u) }.getOrThrow().size)
        }
    }

    @Test
    fun allCases() = assertPassed()
}
