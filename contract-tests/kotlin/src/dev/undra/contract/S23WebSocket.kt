package dev.undra.contract

import dev.undra.playground.core.Live
import dev.undra.playground.core.wsEcho
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.WsError
import dev.undra.runtime.adapters.WsMessage
import kotlinx.coroutines.runBlocking

/**
 * S23: the opt-in `WebSocket` port through the runtime's default adapter (`ClientWebSocketAdapter`, which `UndraCore.load`
 * installs with the JVM defaults) against the shared local server; the core's side is `ws_echo` and `Live`.
 */
fun s23WebSocket(w: World) {
    val ws = RealtimeServer.ws

    // 1. Echo, then the core's close reaches the server with (1000, "done").
    val sent = listOf(WsMessage.Text("a"), WsMessage.Binary(byteArrayOf(1, 2, 3)), WsMessage.Text("é"))
    val echoed = runBlocking { wsEcho("$ws/ws/echo", sent) }
    expectEq("the echoed messages", sent, echoed)
    awaitUntil("the server to see the client close the echo connection with (1000, \"done\")") {
        val seen = RealtimeServer.last("/ws/echo")
        seen.closeCode == 1000L && seen.closeReason == "done"
    }

    val live = Live()
    try {
        // 2. Subprotocol and headers.
        runBlocking {
            expectEq("the subprotocol the server chose", "v2", live.connect("$ws/ws/headers", listOf("v2", "v1"), listOf(Header("X-Token", "t"))))
            val first = live.read(1u)
            check(first.size == 1 && first[0] is WsMessage.Text) { "the first message of /ws/headers was $first" }
            val headers = Json.parseObject((first[0] as WsMessage.Text).value)
            expectEq("x-token of the upgrade request", "t", headers["x-token"])
            live.send(WsMessage.Text("ping"))
            expectEq("the echo of ping", listOf<WsMessage>(WsMessage.Text("ping")), live.read(1u))
            live.disconnect(4000u, "bye")
        }
        awaitUntil("the server to see (4000, \"bye\")") {
            val seen = RealtimeServer.last("/ws/headers")
            seen.closeCode == 4000L && seen.closeReason == "bye"
        }

        // 3. Credit: a core that stopped reading pulls no more.
        runBlocking {
            live.connect("$ws/ws/flood?n=1000", emptyList(), emptyList())
            expectEq("the first five", (0 until 5).map { WsMessage.Text("$it") }, live.read(5u))
        }
        Thread.sleep(200)
        val pulls = live.pulls()
        check(pulls <= 2uL) { "pulls after reading 5 and waiting 200 ms: $pulls (at most 2)" }
        runBlocking {
            expectEq("the other 995, in order", (5 until 1000).map { WsMessage.Text("$it") }, live.read(995u))
            val end = expectFailsAsync<WsError.Closed>("the read after the flood") { live.read(1u) }
            expectEq("the flood's end", WsError.Closed(1000u, "end"), end)
        }

        // 4. Typed ends.
        runBlocking {
            val refused = expectFailsAsync<WsError.Refused>("a refused upgrade") { wsEcho("$ws/ws/deny?status=401", listOf(WsMessage.Text("x"))) }
            expectEq("the status of the refused upgrade", 401.toUShort(), refused.status)
            live.connect("$ws/ws/close?code=4001&reason=kicked", emptyList(), emptyList())
            expectEq("the message before the close", listOf<WsMessage>(WsMessage.Text("hello")), live.read(1u))
            expectEq("the peer's close", WsError.Closed(4001u, "kicked"), expectFailsAsync<WsError.Closed>("the read after the close") { live.read(1u) })
            live.connect("$ws/ws/drop", emptyList(), emptyList())
            expectEq("the message before the drop", listOf<WsMessage>(WsMessage.Text("hello")), live.read(1u))
            expectFailsAsync<WsError.Network>("the read after the drop") { live.read(1u) }
        }

        // 5. A connection nobody closes is closed going away. Bounded by WAIT_MS, a hang detector: the port closes a dropped
        // connection fire and forget, on no timer, so a port that did not would leave it open; how soon the close frame reaches
        // the local server is the machine's.
        runBlocking { live.connect("$ws/ws/stall", emptyList(), emptyList()) }
        live.abandon()
        awaitUntil("the server to see the abandoned connection close with 1001", timeoutMs = WAIT_MS) {
            RealtimeServer.last("/ws/stall").closeCode == 1001L
        }
    } finally {
        live.close()
    }
}
