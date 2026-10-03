package dev.undra.okhttp

import dev.undra.android.RealtimeOnDeviceContract
import dev.undra.runtime.adapters.SseAdapter
import dev.undra.runtime.adapters.WebSocketAdapter
import java.util.concurrent.TimeUnit
import okhttp3.OkHttpClient

/**
 * OkHttp's WebSocket and Sse adapters on the device, against the host's realtime server: the cases are
 * `RealtimeOnDeviceContract`'s (`../adapter-contracts/device`), where the default adapters run them too (ADR-060). Run with
 * `-Pandroid.testInstrumentationRunnerArguments.undra.realtimePort=<port>` (see the contract).
 */
class OkHttpRealtimeOnDeviceTest : RealtimeOnDeviceContract() {
    private fun client(): OkHttpClient = OkHttpClient.Builder().connectTimeout(5, TimeUnit.SECONDS).build()

    override fun webSocket(): WebSocketAdapter = OkHttpWebSocketAdapter(client())

    override fun sse(): SseAdapter = OkHttpSseAdapter(client())

    // OkHttp decodes text frames itself (U+FFFD for bad bytes); ADR-060 decision 6.
    override val rejectsMalformedText: Boolean get() = false
}
