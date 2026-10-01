package dev.undra.runtime

import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.support.handledBy
import dev.undra.runtime.support.portMethods
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.LogCapture
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
import dev.undra.runtime.wire.Payloads.PortStatus
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.Payloads.StreamFlag
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private val METHOD = 0x41u
private val TARGET = CallTarget.ObjectMethod(Handle(0x100000002L), METHOD)

/** A server that answers the Hello like `undra dev` does; [configure] then sets up per-connection behaviour. */
private inline fun withServer(serverHash: ULong = HASH, crossinline configure: (WsTestServer.Conn) -> Unit = {}, body: (WsTestServer) -> Unit) {
    WsTestServer().use { server ->
        server.onConnect = { conn ->
            conn.onMessage = { bytes ->
                val env = Envelope.decode(bytes)
                if (env.kind == Envelope.Kind.HELLO) conn.sendHello(serverHash)
            }
            configure(conn)
        }
        body(server)
    }
}

private fun load(server: WsTestServer, timeout: kotlin.time.Duration = 5.seconds, adapters: Map<UInt, PortImpl> = emptyMap()): UndraCore =
    UndraCore.attach(
        RemoteTransport(java.net.URI(server.url), timeout),
        LoadOptions(mode = Mode.REMOTE, remoteUrl = server.url, expectedSchemaHash = HASH, adapters = adapters, defaultAdapters = false, remoteTimeout = timeout),
        makeShared = false,
    )

class RemoteTransportTests : Suite() {
    init {
        case("connecting sends a Hello with the expected schema hash and accepts the server's") {
            withServer { server ->
                load(server).use {
                    val conn = server.awaitConnection()
                    val hello = Payloads.Hello.decode(conn.awaitEnvelope(Envelope.Kind.HELLO).payload)
                    assertEq(HASH, hello.schemaHash)
                    assertEq("jvm", hello.platform)
                    assertEq("dev", hello.mode)
                    assertEq(Mode.REMOTE, it.mode)
                    assertEq(UndraStats.UNKNOWN, it.stats().liveHandles)
                }
            }
        }

        case("a server built from another schema is an UndraSchemaMismatchException, and the connection is closed") {
            withServer(serverHash = 0x77uL) { server ->
                val e = assertThrows<UndraSchemaMismatchException> { load(server) }
                assertEq(HASH, e.expected)
                assertEq(0x77uL, e.got)
                val conn = server.awaitConnection()
                eventually("the client closes the connection") { conn.closeCodes.isNotEmpty() }
            }
        }

        case("a call goes out as a CALL envelope stamped with the schema hash, and the REPLY envelope resumes it") {
            withServer(configure = { conn ->
                val previous = conn.onMessage
                conn.onMessage = { bytes ->
                    previous(bytes)
                    val env = Envelope.decode(bytes)
                    if (env.kind == Envelope.Kind.CALL) {
                        val call = Payloads.Call.decode(env.payload)
                        conn.send(Envelope.Kind.REPLY, Payloads.Reply(call.callId, ReplyStatus.OK, call.args.reversedArray()).toByteArray())
                    }
                }
            }) { server ->
                load(server).use { core ->
                    val body = runBlocking { core.call(TARGET, METHOD, byteArrayOf(1, 2, 3)) }
                    assertEq(listOf<Byte>(3, 2, 1), body.toList())
                    val conn = server.awaitConnection()
                    val env = conn.envelopes.first { it.kind == Envelope.Kind.CALL }
                    assertEq(HASH, env.schemaHash)
                    assertEq(TARGET, Payloads.Call.decode(env.payload).target)
                    // Sequence numbers run 0, 1, ... per direction: the Hello was 0, this call is 1.
                    assertEq(listOf(0u, 1u), conn.envelopes.map { it.seq })
                }
            }
        }

        case("callSync and construct block for the reply") {
            withServer(configure = { conn ->
                val previous = conn.onMessage
                conn.onMessage = { bytes ->
                    previous(bytes)
                    val env = Envelope.decode(bytes)
                    if (env.kind == Envelope.Kind.CALL) {
                        val call = Payloads.Call.decode(env.payload)
                        val body = if (call.target is CallTarget.Constructor) Codecs.handle.encodeToByteArray(0x200000003L) else Codecs.u32.encodeToByteArray(9u)
                        conn.send(Envelope.Kind.REPLY, Payloads.Reply(call.callId, ReplyStatus.OK, body).toByteArray())
                    }
                }
            }) { server ->
                load(server).use { core ->
                    assertEq(9u, Codecs.u32.decodeAll(core.callSync(TARGET, METHOD, NO_BYTES)))
                    assertEq(0x200000003L, core.construct(5u, 6u, NO_BYTES))
                }
            }
        }

        case("a server that never answers makes a blocking call time out") {
            withServer { server ->
                load(server, timeout = 300.milliseconds).use { core ->
                    val e = assertThrows<UndraException> { core.callSync(TARGET, METHOD, NO_BYTES) }
                    assertTrue(e.message!!.contains("did not answer"), e.message!!)
                    // The client told the core to drop the call.
                    val conn = server.awaitConnection()
                    val callId = Payloads.Call.decode(conn.awaitEnvelope(Envelope.Kind.CALL).payload).callId
                    assertEq(callId, Payloads.Cancel.decode(conn.awaitEnvelope(Envelope.Kind.CANCEL).payload).callId)
                }
            }
        }

        case("streams: credit goes out as STREAM_CREDIT envelopes and items come in as STREAM_ITEM envelopes") {
            withServer(configure = { conn ->
                val previous = conn.onMessage
                conn.onMessage = { bytes ->
                    previous(bytes)
                    val env = Envelope.decode(bytes)
                    when (env.kind) {
                        Envelope.Kind.CALL -> {
                            val call = Payloads.Call.decode(env.payload)
                            conn.send(Envelope.Kind.REPLY, Payloads.Reply(call.callId, ReplyStatus.STREAM_OPENED, NO_BYTES).toByteArray())
                        }
                        Envelope.Kind.STREAM_CREDIT -> {
                            val credit = Payloads.StreamCredit.decode(env.payload)
                            for (i in 0 until 3) {
                                conn.send(Envelope.Kind.STREAM_ITEM, Payloads.StreamItem(credit.callId, StreamFlag.ITEM, Codecs.u32.encodeToByteArray(i.toUInt())).toByteArray())
                            }
                            conn.send(Envelope.Kind.STREAM_ITEM, Payloads.StreamItem(credit.callId, StreamFlag.END, NO_BYTES).toByteArray())
                        }
                        else -> Unit
                    }
                }
            }) { server ->
                load(server).use { core ->
                    val items = runBlocking { core.stream(TARGET, METHOD, NO_BYTES).toList() }.map { Codecs.u32.decodeAll(it) }
                    assertEq(listOf(0u, 1u, 2u), items)
                    val credit = Payloads.StreamCredit.decode(server.awaitConnection().awaitEnvelope(Envelope.Kind.STREAM_CREDIT).payload)
                    assertEq(16u, credit.credit)
                }
            }
        }

        case("change-sets reach the mirror; observe and release go out as envelopes") {
            withServer { server ->
                load(server).use { core ->
                    val seen = CopyOnWriteArrayList<UInt>()
                    core.mirror.register(0x100000002L) { _, _, r -> seen.add(Codecs.u32.decode(r)) }
                    core.observe(0x100000002L, UInt.MAX_VALUE, true)
                    val conn = server.awaitConnection()
                    val observe = Payloads.Observe.decode(conn.awaitEnvelope(Envelope.Kind.OBSERVE).payload)
                    assertEq(Payloads.Observe(Handle(0x100000002L), UInt.MAX_VALUE, true), observe)
                    conn.send(Envelope.Kind.CHANGE_SET, changeSet(1uL, full(0x100000002L, 0u, Codecs.u32.encodeToByteArray(5u))))
                    eventually("the change is applied") { seen.isNotEmpty() }
                    assertEq(listOf(5u), seen.toList())
                    core.release(0x100000002L)
                    assertEq(Payloads.Release(Handle(0x100000002L)), Payloads.Release.decode(conn.awaitEnvelope(Envelope.Kind.RELEASE).payload))
                    core.event(1u, 2u, byteArrayOf(9))
                    assertEq(Payloads.Event(1u, 2u, byteArrayOf(9)), Payloads.Event.decode(conn.awaitEnvelope(Envelope.Kind.EVENT).payload))
                    core.timerFired(4u)
                    assertEq(4u, Payloads.TimerFired.decode(conn.awaitEnvelope(Envelope.Kind.TIMER_FIRED).payload).timerId)
                }
            }
        }

        case("port calls from the server are answered with PORT_REPLY envelopes: sync, async, and unavailable") {
            val ports = mapOf(
                0xA1u to PortImpl(true, portMethods(1u handledBy { a -> a + byteArrayOf(1) })),
                0xA2u to PortImpl(false, portMethods(1u handledBy { a -> kotlinx.coroutines.delay(20); a + byteArrayOf(2) })),
            )
            withServer { server ->
                load(server, adapters = ports).use {
                    val conn = server.awaitConnection()
                    conn.send(Envelope.Kind.PORT_CALL, Payloads.PortCall(0xA1u, 1u, 10u, byteArrayOf(5)).toByteArray())
                    conn.send(Envelope.Kind.PORT_CALL, Payloads.PortCall(0xA2u, 1u, 11u, byteArrayOf(6)).toByteArray())
                    conn.send(Envelope.Kind.PORT_CALL, Payloads.PortCall(0xA3u, 1u, 12u, byteArrayOf(7)).toByteArray())
                    eventually("three port replies") { conn.envelopes.count { it.kind == Envelope.Kind.PORT_REPLY } == 3 }
                    val replies = conn.envelopes.filter { it.kind == Envelope.Kind.PORT_REPLY }.map { Payloads.PortReply.decode(it.payload) }.associateBy { it.portCallId }
                    assertEq(Payloads.PortReply(10u, PortStatus.OK, byteArrayOf(5, 1)), replies.getValue(10u))
                    assertEq(Payloads.PortReply(11u, PortStatus.OK, byteArrayOf(6, 2)), replies.getValue(11u))
                    assertEq(Payloads.PortReply(12u, PortStatus.UNAVAILABLE, NO_BYTES), replies.getValue(12u))
                }
            }
        }

        case("LOG envelopes from the core reach java.util.logging") {
            withServer { server ->
                LogCapture("undra.test").use { log ->
                    load(server).use {
                        server.awaitConnection().send(Envelope.Kind.LOG, Payloads.Log(3u, "undra.test", "careful").toByteArray())
                        eventually("the record arrives") { log.records.isNotEmpty() }
                        assertEq("careful", log.records.single().message)
                        assertEq(java.util.logging.Level.WARNING, log.records.single().level)
                    }
                }
            }
        }

        case("a message the server sends in several frames is put back together") {
            withServer(configure = { conn ->
                val previous = conn.onMessage
                conn.onMessage = { bytes ->
                    previous(bytes)
                    val env = Envelope.decode(bytes)
                    if (env.kind == Envelope.Kind.CALL) {
                        val call = Payloads.Call.decode(env.payload)
                        val body = ByteArray(200_000) { (it % 251).toByte() }
                        conn.sendFragmented(Envelope.encode(Envelope.Kind.REPLY, 1u, HASH, Payloads.Reply(call.callId, ReplyStatus.OK, body).toByteArray()), 7_000)
                    }
                }
            }) { server ->
                load(server).use { core ->
                    val body = runBlocking { core.call(TARGET, METHOD, NO_BYTES) }
                    assertEq(200_000, body.size)
                    assertTrue(body.indices.all { body[it] == (it % 251).toByte() })
                }
            }
        }

        case("a large call and a large reply survive the framing") {
            withServer(configure = { conn ->
                val previous = conn.onMessage
                conn.onMessage = { bytes ->
                    previous(bytes)
                    val env = Envelope.decode(bytes)
                    if (env.kind == Envelope.Kind.CALL) {
                        val call = Payloads.Call.decode(env.payload)
                        conn.send(Envelope.Kind.REPLY, Payloads.Reply(call.callId, ReplyStatus.OK, call.args).toByteArray())
                    }
                }
            }) { server ->
                load(server).use { core ->
                    val big = ByteArray(3_000_000) { (it * 7).toByte() }
                    assertTrue(runBlocking { core.call(TARGET, METHOD, big) }.contentEquals(big))
                }
            }
        }

        case("concurrent calls over one connection are answered independently and sent in order") {
            withServer(configure = { conn ->
                val previous = conn.onMessage
                conn.onMessage = { bytes ->
                    previous(bytes)
                    val env = Envelope.decode(bytes)
                    if (env.kind == Envelope.Kind.CALL) {
                        val call = Payloads.Call.decode(env.payload)
                        conn.send(Envelope.Kind.REPLY, Payloads.Reply(call.callId, ReplyStatus.OK, call.args).toByteArray())
                    }
                }
            }) { server ->
                load(server).use { core ->
                    runBlocking {
                        val jobs = List(200) { i -> async(Dispatchers.Default) { core.call(TARGET, METHOD, byteArrayOf(i.toByte())) } }
                        jobs.forEachIndexed { i, j -> assertEq(listOf(i.toByte()), j.await().toList()) }
                    }
                    val seqs = server.awaitConnection().envelopes.map { it.seq.toInt() }
                    assertEq((0 until 201).toList(), seqs, "sequence numbers are gapless and increasing")
                }
            }
        }

        case("when the server drops the connection, pending calls fail and the core is closed") {
            withServer { server ->
                val core = load(server)
                val failure = CopyOnWriteArrayList<Throwable>()
                val worker = Thread {
                    try {
                        runBlocking { core.call(TARGET, METHOD, NO_BYTES) }
                    } catch (e: Throwable) {
                        failure.add(e)
                    }
                }
                worker.start()
                val conn = server.awaitConnection()
                conn.awaitEnvelope(Envelope.Kind.CALL)
                conn.drop()
                worker.join(10_000)
                assertTrue(failure.single() is UndraException, "got ${failure.singleOrNull()}")
                eventually("the core is closed") { runCatching { core.callSync(TARGET, METHOD, NO_BYTES) }.exceptionOrNull() is UndraException }
                core.close()
            }
        }

        case("a close frame from the server ends the connection with its code in the message") {
            withServer { server ->
                val core = load(server)
                server.awaitConnection().sendClose(1012)
                eventually("calls fail because of the close") { runCatching { runBlocking { core.call(TARGET, METHOD, NO_BYTES) } }.exceptionOrNull()?.cause != null }
                val e = assertThrows<UndraException> { runBlocking { core.call(TARGET, METHOD, NO_BYTES) } }
                assertTrue(e.cause?.message?.contains("1012") == true, "cause: ${e.cause?.message}")
            }
        }

        case("a text frame or a malformed envelope is a protocol error that closes the connection") {
            for (garbage in listOf<(WsTestServer.Conn) -> Unit>({ it.sendText("hello") }, { it.sendBinary(byteArrayOf(1, 2, 3)) })) {
                withServer { server ->
                    val core = load(server)
                    LogCapture("dev.undra.runtime").use {
                        garbage(server.awaitConnection())
                        eventually("the core notices") { runCatching { core.callSync(TARGET, METHOD, NO_BYTES) }.exceptionOrNull()?.cause != null }
                    }
                    val e = assertThrows<UndraException> { core.callSync(TARGET, METHOD, NO_BYTES) }
                    assertTrue(e.cause?.message?.contains("protocol error") == true, "cause: ${e.cause?.message}")
                }
            }
        }

        case("an envelope with another schema hash after the handshake ends the session with a schema mismatch") {
            withServer { server ->
                val core = load(server)
                server.awaitConnection().send(Envelope.Kind.LOG, Payloads.Log(2u, "x", "y").toByteArray(), schema = 0x99uL)
                eventually("the core closes") { runCatching { core.callSync(TARGET, METHOD, NO_BYTES) }.exceptionOrNull()?.cause != null }
                val e = assertThrows<UndraException> { core.callSync(TARGET, METHOD, NO_BYTES) }
                val cause = e.cause as? UndraSchemaMismatchException
                assertEq(0x99uL, cause?.got)
                assertEq(HASH, cause?.expected)
            }
        }

        case("host-only envelope kinds sent by the server are ignored, not fatal") {
            withServer(configure = { conn ->
                val previous = conn.onMessage
                conn.onMessage = { bytes ->
                    previous(bytes)
                    val env = Envelope.decode(bytes)
                    if (env.kind == Envelope.Kind.CALL) {
                        val call = Payloads.Call.decode(env.payload)
                        conn.send(Envelope.Kind.CANCEL, Payloads.Cancel(1u).toByteArray()) // only a host may send this
                        conn.send(Envelope.Kind.SNAPSHOT, byteArrayOf(0, 0, 0, 0))
                        conn.send(Envelope.Kind.REPLY, Payloads.Reply(call.callId, ReplyStatus.OK, byteArrayOf(1)).toByteArray())
                    }
                }
            }) { server ->
                LogCapture("dev.undra.runtime").use {
                    load(server).use { core -> assertEq(listOf<Byte>(1), runBlocking { core.call(TARGET, METHOD, NO_BYTES) }.toList()) }
                }
            }
        }

        case("connecting to a port with no server is an UndraException naming the URL") {
            val closed = java.net.ServerSocket(0).use { it.localPort } // a port that was free a moment ago
            val url = "ws://127.0.0.1:$closed"
            val e = assertThrows<UndraException> {
                UndraCore.attach(
                    RemoteTransport(java.net.URI(url), 2.seconds),
                    LoadOptions(mode = Mode.REMOTE, remoteUrl = url, expectedSchemaHash = HASH, defaultAdapters = false),
                    makeShared = false,
                )
            }
            assertTrue(e.message!!.contains(url), e.message!!)
        }

        case("a server that accepts but never says Hello makes connecting time out") {
            WsTestServer().use { server ->
                val e = assertThrows<UndraException> { load(server, timeout = 300.milliseconds) }
                assertTrue(e.message!!.contains("handshake"), e.message!!)
            }
        }

        case("snapshots are refused over a remote transport") {
            withServer { server ->
                load(server).use { core ->
                    assertThrows<UndraModeException> { core.snapshot() }
                    assertThrows<UndraModeException> { core.restore(byteArrayOf(0, 0, 0, 0)) }
                }
            }
        }

        case("UndraCore.load(REMOTE) connects, becomes shared, and close() clears it") {
            withServer(configure = { conn ->
                val previous = conn.onMessage
                conn.onMessage = { bytes ->
                    previous(bytes)
                    val env = Envelope.decode(bytes)
                    if (env.kind == Envelope.Kind.CALL) {
                        val call = Payloads.Call.decode(env.payload)
                        conn.send(Envelope.Kind.REPLY, Payloads.Reply(call.callId, ReplyStatus.OK, byteArrayOf(5)).toByteArray())
                    }
                }
            }) { server ->
                val core = UndraCore.load(LoadOptions(mode = Mode.REMOTE, remoteUrl = server.url, expectedSchemaHash = HASH, defaultAdapters = false))
                try {
                    assertTrue(UndraCore.shared === core)
                    val second = UndraCore.load(LoadOptions(mode = Mode.REMOTE, remoteUrl = server.url, expectedSchemaHash = HASH, defaultAdapters = false))
                    assertTrue(UndraCore.shared === core, "shared stays the first core")
                    second.close()
                    assertTrue(UndraCore.shared === core, "closing another core does not unset it")
                    assertEq(listOf<Byte>(5), runBlocking { UndraCore.shared.call(TARGET, METHOD, NO_BYTES) }.toList())
                } finally {
                    core.close()
                }
                assertThrows<UndraException> { UndraCore.shared }
            }
        }

        case("closing the core closes the socket") {
            withServer { server ->
                val core = load(server)
                val conn = server.awaitConnection()
                core.close()
                eventually("the server sees a close frame") { conn.closeCodes.isNotEmpty() }
                assertEq(1000, conn.closeCodes.first())
                assertThrows<UndraException> { runBlocking { core.call(TARGET, METHOD, NO_BYTES) } }
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
