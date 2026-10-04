# SDE: `okhttp-adapters` - the app's network stack inside the ports (wt/okhttp-adapters, 2026-10-02)

User feedback U5/T3: "the Android adapter uses HttpURLConnection, not OkHttp; our real network stack (token refresh,
interceptors, tracing, cert pinning) would need custom adapters; it sits beside our network stack, not inside it." The
README's answer ("`HttpURLConnection` on Android is OkHttp underneath") is true and beside the point: their auth, tracing and
pinning live in **their** `OkHttpClient`, which the platform's instance cannot see, so the core's requests went out
unauthenticated, untraced and unpinned. ADR-060 (proposed, written before the code) decides the fix: the ports already are the
answer, and the work is removing the cost of using them. No wire, ABI, schema or generated-code change; no new port.

## What landed

| Platform | What | Where |
|---|---|---|
| Android | the new optional module `okhttp-adapters` (`dev.undra:okhttp-adapters`; okhttp3 4.12.0 is its only new dependency, and no other module carries it): `OkHttpHttpAdapter`, `OkHttpWebSocketAdapter`, `OkHttpSseAdapter` (each also takes a `() -> OkHttpClient`, asked for on every request), `AndroidPlatformDefaults.installWithOkHttp(core, context, client, ..)` returning an `OkHttpPlatform` | `runtimes/kotlin/undra-runtime/okhttp-adapters` |
| Kotlin runtime | `SseStreamReader` and `ReadAheadSource` public behind `@UndraEmbeddingApi` (what the default adapters already used; no behavior change) | `:runtime` `adapters/SseAdapters.kt`, `PulledStream.kt` |
| iOS | `URLSessionWebSocketAdapter(session:maximumMessageSize:)`: connections are tasks of the app's session, the adapter is the task's delegate for the handshake and the close frame (iOS 15), never invalidates the session. `HttpAdapter(session:)` and `URLSessionSseAdapter(session:)` existed | `URLSessionWebSocketAdapter.swift` |
| TypeScript | no code: `fetchHttp({ fetch })`, `fetchSse({ fetch })`, `browserWebSocket({ WebSocket })` already took the app's own; the tests pin it | `test/app-network-stack.test.ts` |
| Docs | the module README, the Android and Swift READMEs, SPEC 8/11/17.2/17.3, `docs/ONBOARDING.md`, the ports page ("Your network stack inside"), the cookbook recipe "Your network stack" (page, index card, nav, snippets), the cookbook README | |

## The design, in the order it was decided

* **One module, not a default.** OkHttp in `android-adapters` would be a second HTTP stack for every app that has none, and the
  runtime modules' dependency rule is the reason adopting Undra adds nothing. The one-call is an extension on the existing
  object (`AndroidPlatformDefaults.installWithOkHttp`): `install`, then the three ports re-registered over the client on the same
  thread before any request can be made (replacing a port runs the old one's `detach`). Each of the three can be kept from the
  platform (`webSocket = ClientWebSocketAdapter()`).
* **`enqueue`, not `execute` on an IO thread.** The app's `Dispatcher` (five calls per host by default) must see the core's
  requests, or a client tuned to protect a backend sees them in addition. The body is read on the thread OkHttp gave the call
  (in `onResponse`), so one thread serves a call from start to last byte; cancelling the coroutine is `Call.cancel()`, which closes
  the socket and ends a blocked read.
* **The request's `timeoutMs` is a coroutine timeout over the whole exchange, not `callTimeout`**, so a client that has a call
  timeout of its own keeps it, and the error is `Timeout` either way (OkHttp's call timeout is an `InterruptedIOException("timeout")`,
  mapped; any other `InterruptedIOException` is `Cancelled`). Redirects, retries, the `Authenticator`, gzip, the cache and cookie jar
  are the client's, on purpose; the default client follows `https` to `http` where the other adapters never do (the README says so,
  with the one-line fix).
* **A failure travels as a value** (`Outcome`, `WsError?`, `Answer`) and is thrown by the adapter, not resumed through a coroutine:
  under `-ea` kotlinx-coroutines rebuilds a resumed exception by reflection, and a typed error with one `String` constructor comes
  back with its own message inside its `reason`.
* **SSE derives a client** (`newBuilder`: same pool, dispatcher, interceptors, authenticator, pinner, listener) with no read
  timeout, no call timeout and HTTP/1.1, because a stream outlives every timeout the client was built for. The reader is the
  default adapters' `SseStreamReader`.
* **WebSocket on OkHttp.** The reader thread cannot be paused, but it waits for the listener to return, and the listener does not
  return while the binding's buffer is full (one frame beyond the window), implemented on `ReadAheadSource`. The close handshake is
  answered in `onClosing` with the peer's code (1000 when it cannot be sent), `close` waits for `onClosed` for three seconds and
  cancels. A client without a ping interval gets 30 s (`pingIntervalMillis`), the default adapter's silence limit.

## Findings (each pinned by a test, none loosened)

* **OkHttp does not validate UTF-8 in a text frame**: it decodes text itself and puts U+FFFD for the bad bytes; the default
  adapter closes with 1007 and ends the stream `Protocol`. The shared realtime suite takes `rejectsMalformedText = false` for this
  adapter and a case of its own pins what happens (also on the device). The README and the ADR say an app that needs the check keeps
  `ClientWebSocketAdapter` for the port.
* **OkHttp gives the app's `EventListener` nothing for a WebSocket** (it derives a client with `EventListener.NONE`: measured, no
  `callStart`), and offers `permessage-deflate` by default. Interceptors, `Authenticator`, pinner, `Dns` and proxy do apply.
* **A message over OkHttp's 16 MiB outgoing queue closes the connection with 1001**; a core that stops reading longer than the
  ping interval loses the connection to the missing-pong timeout (the reader cannot read the pong).
* **OkHttp passed the Http and Sse contracts at the first run**; the first run of the WebSocket one needed only the UTF-8 flag.
* **The module cannot be built against OkHttp 5.x** with Kotlin 2.0.21 (5.x's metadata is 2.2), and AGP keeps compile and runtime
  class paths aligned, so a Gradle property cannot put a newer one only on the test runtime. The JVM unit tests of the day were run
  by hand against 5.3.2 (the Android artifact, Okio 3.16.4, stdlib 2.2.21, stubs for `android.util.Log` and `Build.VERSION`) and
  passed, 66 of 66.
* **The per-task delegate** delivers `didOpenWithProtocol` and the close frame on macOS 26.x: the WebSocket suite passes on an
  app's session. Not run on iOS 15 or 16 (the floor) or on macOS 15 (the hosted runner).
* `ios15-sample`'s bindings are generated without `--docs`, so `undra bindgen --check --docs` reports it as different while
  `--check` passes: the CI step checks the cookbook, Fieldbook and the playground with `--docs`, and all three pass. Nothing of this
  piece touches generated code.

## Tests (R4)

* The suites are written once. `AndroidHttpAdapterTest` became `HttpAdapterContract` (`adapter-contracts`) plus a subclass;
  `RealtimeAdapterTests` became `RealtimeAdapterContract` (`test-support`, with `Suite`, the assertions, the realtime server and
  `eventually`, moved from `runtime/src/test` in the same packages); `RealtimeOnDeviceTest` became `RealtimeOnDeviceContract`
  (`adapter-contracts/device`). The parameters are the places a platform may differ in what a test can observe. Counts did not
  move where they were not meant to: `:runtime` under `test-local.sh` 887 cases (the realtime suite 23), the testkit 32,
  `:android-adapters` 151 JVM results per variant with 1 skipped (150 on the device, 1 skipped).
* `:okhttp-adapters` JVM: 69 per variant (138 results): the Http contract and 12 cases of its own (`OkHttpHttpAdapterTest`, 51), the
  realtime suite through the bindings against the Node server (`OkHttpRealtimeAdapterTest`: 17 shared cases and 7 of its own), the pure
  rules (`OkHttpRulesTest`, 14), the recipe (`NetworkStackRecipeTest`, 3). On `emulator-5554` (AVD `undra`): 62 pass, 0 skipped
  (the 51 Http cases, `OkHttpHttpOnDeviceTest` 4, `OkHttpPlatformDefaultsOnDeviceTest` 3, `OkHttpRealtimeOnDeviceTest` 4 against the
  host's realtime server). The emulator CI job is on hold and not touched.
* Mutations shown to fail the suite: no `Call.cancel()` on cancellation, no wait in the WebSocket listener, the client's read
  timeout kept on a stream, restricted headers sent, a stream not cancelled on close; in Swift the `session:` branch replaced by a
  session of its own.
* Swift: `URLSessionWebSocketOnAppSessionTests` (the whole WebSocket suite on an app's session with a configuration header and a
  recording delegate, plus two cases) and `AppSessionTests` (Http, Sse and WebSocket carry the configuration and reach the
  delegate): 13 new XCTest cases, 21 in the filtered run, all passing.
* TypeScript: `app-network-stack.test.ts`, 5 cases (a wrapped `fetch` sees every request and changes a header, answers a 401 with a
  refreshed token and sends the request, body included, again; the same for the Sse adapter; an app's `WebSocket` constructor adds a
  token to the upgrade).
* No benchmark row: nothing here touches the boundary (R9 does not apply).

## Not verified

Certificate pinning end to end (a TLS server needs a certificate generator: `okhttp-tls`, another artifact than the approved one;
a pin that does not match is a unit test on the exception OkHttp throws); the Swift per-task delegate on iOS 15 and 16 and on
macOS 15; OkHttp 5.x as a build; the Android emulator CI job (on hold).
