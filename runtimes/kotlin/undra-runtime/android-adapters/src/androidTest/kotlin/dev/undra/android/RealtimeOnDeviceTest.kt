package dev.undra.android

import dev.undra.runtime.adapters.ClientWebSocketAdapter
import dev.undra.runtime.adapters.SseAdapter
import dev.undra.runtime.adapters.UrlConnectionSseAdapter
import dev.undra.runtime.adapters.WebSocketAdapter

/**
 * The default WebSocket and Sse adapters on the device (`ClientWebSocketAdapter`, `UrlConnectionSseAdapter`) against the host's
 * realtime server: the cases are `RealtimeOnDeviceContract`'s, in `../adapter-contracts/device`, where OkHttp's adapters run them
 * too (ADR-060).
 */
class RealtimeOnDeviceTest : RealtimeOnDeviceContract() {
    override fun webSocket(): WebSocketAdapter = ClientWebSocketAdapter(connectTimeoutMillis = 5_000)

    override fun sse(): SseAdapter = UrlConnectionSseAdapter()
}
