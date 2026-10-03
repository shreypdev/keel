# okhttp-adapters

The optional `:okhttp-adapters` Gradle module of the Kotlin runtime (ADR-060): the `Http`, `WebSocket` and `Sse` ports over **your
own `OkHttpClient`**, so that its interceptors, its `Authenticator` (token refresh), its event listeners (tracing) and its
certificate pinner apply to everything the core sends, with nothing Undra-specific to configure. Artifact
`dev.undra:okhttp-adapters:0.1.0-SNAPSHOT` (a composite build resolves it to this module, as the playground app does), minSdk 26.

It is its own module because **OkHttp is a dependency the other modules must not carry**: `:runtime` is Kotlin stdlib and
kotlinx-coroutines, `:android-adapters` is the Android SDK and `:runtime`, so adopting Undra adds nothing to an app. An app that does
not add this module carries no OkHttp. It depends on `:android-adapters` (for `AndroidPlatformDefaults`) and on
`com.squareup.okhttp3:okhttp` 4.12.0; `settings.gradle.kts` includes it under the same condition as the other Android modules.

## Why

`android-adapters/README.md` says that `HttpURLConnection` on Android is OkHttp underneath. True, and not what an app with a network
stack needs: its token refresh, interceptors, tracing and pinning live in **its** client, which is another instance than the
platform's. A request through the default adapter carries no token, appears in no trace and is not pinned. The ports are the
answer (the core never makes a request; the host does), and this module removes the cost of using them: the three adapters keep
the contract the core relies on (ADR-025, ADR-047), so the app does not write them.

## Your stack inside

```kotlin
class MyApp : Application() {
    override fun onCreate() {
        super.onCreate()
        val core = UndraPlaygroundCore.load(
            LoadOptions(mirror = MirrorOptions(framePacer = ChoreographerFramePacer())),
        )
        // In place of AndroidPlatformDefaults.install(core, this): the same ports, and Http, WebSocket and Sse go through your client.
        AndroidPlatformDefaults.installWithOkHttp(core, this, appGraph.okHttpClient)
    }
}
```

`installWithOkHttp(core, context, client, http, webSocket, sse, requireValidatedNetwork, reportLifecycle, onBackgroundWorkPending)`
is `AndroidPlatformDefaults.install` with the three network ports registered over `client` in place of the platform's (through
`installWithNetworkPorts`, before `Kv`: a core replays its offline queue as soon as `Kv` answers, and that request goes through your
client too; the platform's network adapters are never registered). It returns an `OkHttpPlatform`: the platform of `install` (`.platform`),
the adapters that took the three ports (`.http`, `.webSocket`, `.sse`) and a `close()` that stops the event sources and closes
what is open.

Each adapter is also usable alone: `OkHttpHttpAdapter(client).portImpl()` is a `PortImpl` for `LoadOptions.adapters` or
`core.registerPort`; `WebSocketPortAdapter(OkHttpWebSocketAdapter(client)).portImpl()` and
`SsePortAdapter(OkHttpSseAdapter(client)).portImpl()` are the other two. To keep one of the platform's adapters for a port, pass it
to `installWithOkHttp` (`webSocket = ClientWebSocketAdapter()`). **A client you replace at run time** (after a login, or built behind
a dependency-injection provider) is followed by the adapters' second constructors, which take `() -> OkHttpClient` and ask for it on
every request: `OkHttpHttpAdapter { appGraph.okHttpClient }`.

## What goes through your client

| Port | Adapter | Through your client |
|---|---|---|
| `Http` | `OkHttpHttpAdapter(client, maxResponseBytes = 64 MiB)` | the call itself: application and network interceptors, `Authenticator`, event listener, pinner, `Dns`, proxy, cookie jar, cache, pool, dispatcher |
| `WebSocket` (ADR-047) | `OkHttpWebSocketAdapter(client, pingIntervalMillis)`, served by `WebSocketPortAdapter` | the upgrade request: application interceptors, `Authenticator`, pinner, `Dns`, proxy, pool, dispatcher. OkHttp runs a WebSocket's upgrade **without your network interceptors and without your event listener** (below) |
| `Sse` (ADR-047) | `OkHttpSseAdapter(client)`, served by `SsePortAdapter` | the request, on a client derived from yours (`newBuilder`: same pool, dispatcher, interceptors, `Authenticator`, pinner and listener) with **no read timeout, no call timeout and HTTP/1.1**, which a stream needs. `Last-Event-ID` goes out as its UTF-8 bytes, so an id that is not ASCII resumes too |

**The contract is the other adapters'** and the same suites check it (below): an error status is a response; failures are typed
(`InvalidUrl`, `Timeout`, `Cancelled`, `Network`; for the realtime ports `Refused`, `Network`, `Protocol`, `Closed`, `Ended`); the
request's `timeoutMs` bounds the whole exchange, body included; cancelling the calling coroutine cancels the call and closes the
socket; a response body over `maxResponseBytes` fails instead of exhausting the heap; WebSocket and Sse are pulled, and a core that
stops pulling stalls the server (TCP pushes back).

**What is yours, not ours** (the reason to use it): redirects (`followRedirects`, `followSslRedirects`, at most 20; the default client
follows `https` to `http`, which the other adapters never do: `followSslRedirects(false)` gives the same rule), `retryOnConnectionFailure`,
the `Authenticator` on a `401`, transparent gzip, the client's cache and cookie jar if it has them, its connect, read, write and call
timeouts when the request has none of its own, and the `Dispatcher`'s limits (the core's requests wait their turn with yours).
A header value must be printable ASCII (OkHttp's rule); anything else is `InvalidUrl`, naming the header. Your interceptors run on
OkHttp's threads: one that throws anything but an `IOException` is rethrown there by OkHttp (it crashes an Android app, as it does for
your own calls), and the core's request ends `Cancelled`.

**What OkHttp's WebSocket cannot do** (ADR-060, decision 6), so it is not what the adapter does either:

* a text frame that is not UTF-8 arrives with U+FFFD for the bad bytes; the default adapter fails it with `Protocol` (RFC 6455, 1007).
  An app that needs the check keeps `ClientWebSocketAdapter` for the port;
* no network interceptor and no event listener on the upgrade: OkHttp opens a WebSocket on a client it derives with
  `EventListener.NONE` and leaves network interceptors out of a WebSocket's call. Application interceptors, the `Authenticator`, the
  pinner, `Dns` and the proxy do apply. Tracing that is a network interceptor or an `EventListener` does not see the core's WebSockets;
  a test pins this, so an OkHttp that changes it is noticed;
* no limit on the size of an inbound message (OkHttp offers `permessage-deflate`, so a small frame can inflate to a large one), and a
  message over OkHttp's 16 MiB outgoing queue closes the connection with 1001;
* reading cannot pause: the adapter holds OkHttp's reader thread while the binding's buffer is full, which stalls the server, with
  one frame read beyond the window. A core that stops reading for longer than the ping interval loses the connection to OkHttp's
  missing-pong timeout (`pingIntervalMillis = 0`, or a client with `pingInterval(0)` and your own dead-connection policy, avoids it);
* a connection the network silently dropped is noticed only by sending: unless your client has a ping interval, the adapter gives
  it 30 s (`pingIntervalMillis` overrides, `0` disables).

## Using another OkHttp

Built and tested against `4.12.0` (`gradle/libs.versions.toml`). Gradle takes the highest version in the app, so an app on OkHttp 5
runs these classes against it. That works for what the module uses: its 66 JVM unit tests (the Http contract, the realtime contract
against the Node server, the rules) were run once against `5.3.2` (the Android artifact, with the build classes of this module and a
stubbed `android.util.Log` and `Build.VERSION`) and passed, 66 of 66. The module itself cannot be *built* against 5.x here: 5.x's Kotlin
metadata (2.2) is newer than the compiler this repository pins (2.0.21).

## Tests

The suites every adapter of the Kotlin runtime meets are written once and run here on OkHttp; this module's own cases are about what
OkHttp does.

* **JVM unit tests** (`./gradlew :okhttp-adapters:testDebugUnitTest`, or `:okhttp-adapters:test` for both variants):
  * `OkHttpHttpAdapterTest` (`src/sharedTest`, so on the JVM and on the device): `HttpAdapterContract` (`../adapter-contracts`, the
    cases of `AndroidHttpAdapterTest`) on a plain client, then what the module is for: an interceptor sees every request (the
    application one each call, the network one each hop) and changes a header, an `Authenticator` refreshes a token and the request,
    its body included, goes again, an event listener sees each call, a client provider is asked every time, a client's call timeout and
    redirect policy are respected, gzip is decoded, a header that is not ASCII is refused naming it, cancelled and timed-out calls leave
    no call running in the dispatcher and no connection in use;
  * `OkHttpRealtimeAdapterTest`: `RealtimeAdapterContract` (`../test-support`, the failure-injection suite `:runtime` runs on the default
    adapters) through `WebSocketPortAdapter` and `SsePortAdapter` against `contract-tests/servers/realtime-server.mjs` (Node; skipped,
    saying why, without it, failed with `UNDRA_REQUIRE_TOOLCHAINS=1`), then: malformed text is repaired, an interceptor tags the
    upgrade and the stream, a network interceptor and the event listener do not see the upgrade (OkHttp's rule, pinned), the
    `Authenticator` answers an upgrade's `401`, a client provider is asked at every connect, the ping interval, a client's read and
    call timeouts end neither a quiet stream nor a quiet WebSocket, closed streams and connections leave no reader on the dispatcher
    and no connection in use, the derived clients share the app's pool, dispatcher and interceptors;
  * `OkHttpRulesTest`: the pure decisions (URLs, headers, how OkHttp's exceptions become the port's typed errors, a pin that does not match).
* **Instrumented tests** (`./gradlew :okhttp-adapters:connectedDebugAndroidTest`, on a booted emulator or device; set `ANDROID_SERIAL`
  when several are attached): the shared Http cases again on the device, `OkHttpHttpOnDeviceTest` (a request from the main thread does
  its work on another thread, `localhost` resolves), `OkHttpPlatformDefaultsOnDeviceTest` (`installWithOkHttp`: the ports, `Http` and an
  event stream through the app's client, and a request the core makes while the install runs goes through it too) and `OkHttpRealtimeOnDeviceTest` (`RealtimeOnDeviceContract`, with the host's realtime server
  started first and its port passed, as in the [android-adapters README](../android-adapters/README.md#tests)).
