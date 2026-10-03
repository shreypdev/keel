package dev.undra.runtime

import dev.undra.runtime.adapters.ClientWebSocketAdapter
import dev.undra.runtime.adapters.JdkHttpSseAdapter
import dev.undra.runtime.adapters.UrlConnectionSseAdapter
import dev.undra.runtime.contracts.RealtimeAdapterContract
import dev.undra.runtime.contracts.SseSubject
import dev.undra.runtime.contracts.WebSocketSubject
import org.junit.jupiter.api.Test

/**
 * The default WebSocket and Sse adapters of the JVM and of Android against the shared failure-injection suite
 * (`test-support/.../RealtimeAdapterContract`: ADR-047, brief section 5, and the same suite runs OkHttp's adapters, ADR-060),
 * which uses `contract-tests/servers/realtime-server.mjs` run with Node. Skipped, saying why, without Node (failed with
 * `UNDRA_REQUIRE_TOOLCHAINS=1`).
 */
class RealtimeAdapterTests : RealtimeAdapterContract(
    webSocket = WebSocketSubject("ClientWebSocketAdapter", { ClientWebSocketAdapter(connectTimeoutMillis = 5_000) }),
    sse = listOf(
        SseSubject("java.net.http", ::JdkHttpSseAdapter, canAbortBlockedRead = true),
        // The JDK's HttpURLConnection cannot abort a blocked read (Android's can): those two cases run on the device.
        SseSubject("HttpURLConnection", { UrlConnectionSseAdapter() }, canAbortBlockedRead = false),
    ),
) {
    @Test
    fun allCases() {
        try {
            assertPassed()
        } finally {
            stopServer()
        }
    }
}
