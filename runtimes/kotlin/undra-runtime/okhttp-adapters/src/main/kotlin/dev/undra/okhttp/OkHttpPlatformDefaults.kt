package dev.undra.okhttp

import android.content.Context
import dev.undra.android.AndroidPlatform
import dev.undra.android.AndroidPlatformDefaults
import dev.undra.runtime.UndraCore
import dev.undra.runtime.adapters.SseAdapter
import dev.undra.runtime.adapters.SsePortAdapter
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.WebSocketAdapter
import dev.undra.runtime.adapters.WebSocketPortAdapter
import okhttp3.OkHttpClient

/**
 * What [installWithOkHttp] installed: [AndroidPlatformDefaults.install]'s platform, and the adapters that took the place of its
 * three network ports.
 *
 * @property platform everything `AndroidPlatformDefaults.install` made. Its `http`, `webSocket` and `sse` are the platform's
 *   own, which are no longer registered; use this class's.
 * @property http the `Http` adapter, over the app's client.
 * @property webSocket the binding of the opt-in `WebSocket` port (ADR-047), over the adapter given to [installWithOkHttp].
 * @property sse the binding of the opt-in `Sse` port (ADR-047), over the adapter given to [installWithOkHttp].
 */
public class OkHttpPlatform internal constructor(
    public val platform: AndroidPlatform,
    public val http: OkHttpHttpAdapter,
    public val webSocket: WebSocketPortAdapter,
    public val sse: SsePortAdapter,
) : AutoCloseable {
    /** Stops reporting `Connectivity` and `Lifecycle`, cancels pending timers and closes every open WebSocket and event stream. */
    override fun close() {
        platform.close()
        webSocket.close()
        sse.close()
    }
}

/**
 * [AndroidPlatformDefaults.install] with the app's own [OkHttpClient] behind the network ports (ADR-060): the one call an app that
 * has a network stack makes in place of `install`. `Http`, `WebSocket` and `Sse` go through [client], so its interceptors,
 * authenticator (token refresh), event listeners (tracing) and certificate pinner apply to the core's traffic; everything else is
 * what `install` registers.
 *
 * ```kotlin
 * class MyApp : Application() {
 *     override fun onCreate() {
 *         super.onCreate()
 *         val core = UndraPlaygroundCore.load(LoadOptions(mirror = MirrorOptions(framePacer = ChoreographerFramePacer())))
 *         AndroidPlatformDefaults.installWithOkHttp(core, this, appGraph.okHttpClient)
 *     }
 * }
 * ```
 *
 * The three ports are registered right after `install` returns, on the same thread and before any request can be made, so the
 * platform's `HttpURLConnection` adapters never serve one. To keep one of the platform's (the default WebSocket adapter checks that
 * text is UTF-8, which OkHttp's does not), pass `ClientWebSocketAdapter()` or `UrlConnectionSseAdapter()` for it. To follow a client the
 * app replaces at run time, pass adapters built with a provider (`OkHttpHttpAdapter { appGraph.okHttpClient }`).
 *
 * @param core the core its load (`Undra<Namespace>.load`) returned.
 * @param context any context of the app; only its application context is kept.
 * @param client the app's client.
 * @param http the `Http` adapter, to change its size limit or to give it a client provider.
 * @param webSocket the adapter of the opt-in `WebSocket` port.
 * @param sse the adapter of the opt-in `Sse` port.
 * @param requireValidatedNetwork as in `AndroidPlatformDefaults.install`.
 * @param reportLifecycle as in `AndroidPlatformDefaults.install`.
 * @param onBackgroundWorkPending as in `AndroidPlatformDefaults.install`.
 * @return the platform and the adapters, and a handle that stops the event sources and closes what is open.
 */
public fun AndroidPlatformDefaults.installWithOkHttp(
    core: UndraCore,
    context: Context,
    client: OkHttpClient,
    http: OkHttpHttpAdapter = OkHttpHttpAdapter(client),
    webSocket: WebSocketAdapter = OkHttpWebSocketAdapter(client),
    sse: SseAdapter = OkHttpSseAdapter(client),
    requireValidatedNetwork: Boolean = false,
    reportLifecycle: Boolean = true,
    onBackgroundWorkPending: (() -> Unit)? = null,
): OkHttpPlatform {
    val platform = install(core, context, requireValidatedNetwork = requireValidatedNetwork, reportLifecycle = reportLifecycle, onBackgroundWorkPending = onBackgroundWorkPending)
    val webSocketBinding = WebSocketPortAdapter(webSocket)
    val sseBinding = SsePortAdapter(sse)
    // Replacing a port detaches the one it replaces: the platform's WebSocket and Sse bindings close what they hold (nothing yet).
    core.registerPort(StandardPorts.Http.PORT_ID, http.portImpl())
    core.registerPort(StandardPorts.WebSocket.PORT_ID, webSocketBinding.portImpl())
    core.registerPort(StandardPorts.Sse.PORT_ID, sseBinding.portImpl())
    return OkHttpPlatform(platform, http, webSocketBinding, sseBinding)
}
