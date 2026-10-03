# ADR-060: the app's network stack inside the ports: `okhttp-adapters` for Android, the app's `URLSession` for Apple, the app's `fetch` for the web

Status: **proposed** (2026-10-02; implemented on the same branch, `wt/okhttp-adapters`; reviewed the same day,
`.10x/reviews/2026-10-02-okhttp-adapters-review.md`, whose fixes are in the notes at the end). Touches SPEC 8
and 11 (a new optional Kotlin module and its public shape), 17 (the Kotlin adapter interfaces gain two building blocks behind
`@UndraEmbeddingApi`), `runtimes/kotlin` (module `okhttp-adapters`; `SseStreamReader` and `ReadAheadSource` of `:runtime` become
public), the Swift runtime (`URLSessionWebSocketAdapter(session:)`), the docs and the cookbook. **No wire, envelope, C ABI, wasm
ABI, schema or generated-code change; no new port; `:runtime` and `:android-adapters` gain no dependency.** The only new
dependency is `com.squareup.okhttp3:okhttp` 4.12.0, in the new module alone (the founder's yes, for a new optional module only).
Constitution R11 (a new public shape of a runtime, decided before the code), R1 (nothing here is described by `undra-meta`; it is
host code under the ports), R6, R12 (host side: the core stays deterministic).

## Context

User feedback U5/T3, in their words: *"The Android adapter uses HttpURLConnection, not OkHttp. Our real network stack (token
refresh, interceptors, tracing, cert pinning) would need custom adapters. It sits beside our network stack, not inside it."*

`android-adapters/README.md` answers a different question: that `HttpURLConnection` on Android is OkHttp underneath. It is, and it
does not help. Their refresh logic, their interceptors, their tracing and their certificate pinner live in **their**
`OkHttpClient`, an object in their app. The `OkHttpClient` inside the platform is another instance, configured by the OS, which no
app code can reach. A request the core makes through the default adapter therefore carries no token (so it gets a 401 the core
has to handle without the refresh the rest of the app has), appears in no trace, and **is not pinned**: for an app whose
security review says every request is pinned, that is a defect, not an inconvenience.

The ports are the right answer: the core never makes a request, the host does (SPEC 8), and an app can register any adapter for
`Http`, `WebSocket` and `Sse`. What is wrong is the price of using that. A custom adapter must reproduce the contract the core
relies on, and the contract is not small (ADR-025, ADR-047): typed outcomes (`InvalidUrl`, `Timeout`, `Cancelled`, `Network`; an
error status is a response), a timeout that bounds the body too, cancellation that closes the socket, a size cap, redirect rules,
for WebSocket and Sse the pull discipline and the read-ahead that make TCP push back, the UTF-8 and `text/event-stream` checks.
`AndroidHttpAdapter` and its `HttpRules` are 460 lines; the default WebSocket and Sse adapters another 600, over the runtime's `PulledStream` and parser. An
app that only wants its client in the path should not write them.

Where each platform stands today:

| | `Http` | `WebSocket` | `Sse` |
|---|---|---|---|
| Swift | `HttpAdapter(session:)` | `URLSessionWebSocketAdapter(configuration:)`: a session of its own per connection, so the app's delegate never sees it | `URLSessionSseAdapter(session:)` |
| Kotlin on Android | `AndroidHttpAdapter` (`HttpURLConnection`) only | the runtime's own RFC 6455 client over `java.net.Socket`, no HTTP stack at all | `UrlConnectionSseAdapter` (`HttpURLConnection`) only |
| TypeScript | `fetchHttp({ fetch })` | `browserWebSocket({ WebSocket })` | `fetchSse({ fetch })` |
| React Native | `reactNativeHttp()`: React Native's `fetch`, which is the app's networking module | the `WebSocket` global | `reactNativeSse()` over `fetch` |

So the gap is Android (nothing to hand the app's client to), and on Apple the WebSocket adapter (it cannot take a session).

## Decision

1. **A new optional module, `okhttp-adapters`** (`runtimes/kotlin/undra-runtime/okhttp-adapters`, artifact
   `dev.undra:okhttp-adapters`, package `dev.undra.okhttp`): an Android library like `android-adapters`, depending on it and on
   `okhttp3` and on nothing else (Kotlin stdlib and kotlinx-coroutines come through `:runtime`). Included by `settings.gradle.kts`
   under the same condition as the other Android modules. An app that does not add it carries no OkHttp.

   ```kotlin
   class OkHttpHttpAdapter(client: OkHttpClient, maxResponseBytes: Int = 64 MiB)        // the Http port; request(..), portImpl()
   class OkHttpWebSocketAdapter(client: OkHttpClient, pingIntervalMillis: Long? = null)  // a WebSocketAdapter, served by WebSocketPortAdapter
   class OkHttpSseAdapter(client: OkHttpClient)                                          // an SseAdapter, served by SsePortAdapter
   fun AndroidPlatformDefaults.installWithOkHttp(core, context, client, http = .., webSocket = .., sse = .., + install's options): OkHttpPlatform
   ```

   Each adapter also has a constructor taking `() -> OkHttpClient`, asked for on every request, for the app that replaces its
   client at run time or builds it behind a dependency-injection provider. **Every request goes through the app's client**, so
   its application and network interceptors, `Authenticator` (a `401` is answered with a refreshed token and the request goes
   again), event listeners, `CertificatePinner`, `Dns`, proxy, cookie jar, cache, connection pool and dispatcher apply, with
   nothing Undra-specific configured.

   `installWithOkHttp` is the one call: `AndroidPlatformDefaults.install(...)` with the three network ports over the client in place
   of the platform's, through `AndroidPlatformDefaults.installWithNetworkPorts` (`:android-adapters`, behind `@UndraEmbeddingApi`),
   which registers them before `Kv`: a core replays its offline queue as soon as `Kv` answers, so the platform's network adapters
   are never registered, not even for the length of the install. To keep one of the platform's, pass it
   (`webSocket = ClientWebSocketAdapter()`).

2. **The contract is the same, and one suite checks it for every adapter** (R4). The Http cases of `AndroidHttpAdapterTest` become
   `HttpAdapterContract` (`runtimes/kotlin/undra-runtime/adapter-contracts`), run by `AndroidHttpAdapterTest` and by
   `OkHttpHttpAdapterTest`; the failure-injection suite of the WebSocket and Sse adapters, `RealtimeAdapterTests`, becomes
   `RealtimeAdapterContract` (`test-support`), run by `:runtime` on the default adapters and by `:okhttp-adapters` on OkHttp's; the
   on-device realtime cases likewise (`RealtimeOnDeviceContract`). Nothing is copied: the suites take the adapter as a
   parameter and name the few places a platform may differ in what a test can observe.

3. **How OkHttp's failures become the port's typed errors** (ADR-025, ADR-047 §5):

   | OkHttp | `HttpError` | `WsError` / `SseError` |
   |---|---|---|
   | `SocketTimeoutException` (connect, read, write timeout); `InterruptedIOException("timeout")` (the client's `callTimeout`); the request's `timeoutMs` | `Timeout` | `Network` |
   | the call cancelled by anyone but the caller; any other `InterruptedIOException` | `Cancelled` | `Network` |
   | the caller's coroutine cancelled | `CancellationException` (the call is cancelled first) | the same |
   | a URL that is not `http(s)` with a host, a header OkHttp refuses, a body on `GET` | `InvalidUrl` | `Refused` without a status |
   | `UnknownHostException`, `ConnectException`, `SSLException` (a pin that does not match: "Certificate pinning failure!"), `ProtocolException`, a cleartext policy | `Network` with OkHttp's text | `Network` / `Protocol` |
   | more than 20 redirects and authentication retries (`ProtocolException: Too many follow-up requests`) | `Network("too many redirects or authentication retries (more than 20)")` | |
   | an upgrade answered non-101 | | `Refused(status)` |

4. **What is the client's, not the adapter's** (the reason to use it, so said once and tested): redirects (`followRedirects`,
   `followSslRedirects`, OkHttp's rules and its limit of 20; the default client follows `https` to `http` where the other adapters
   never do: `followSslRedirects(false)` gives the same rule), retries (`retryOnConnectionFailure`), transparent gzip, the cache and
   cookie jar if it has them, the connect, read, write and call timeouts when the request has none of its own (the request's
   `timeoutMs` bounds the whole exchange on top, by a coroutine timeout, so a client with a `callTimeout` keeps it), and the
   dispatcher's limits (OkHttp's default of five calls to one host applies to the core's requests, which wait their turn with the
   app's). A request is enqueued, not executed on an IO thread, so the dispatcher decides; the body is read on the thread
   OkHttp gave the call.

5. **Streaming needs three things the client cannot know, so `Sse` derives a client** (`newBuilder`, which shares the pool,
   dispatcher, interceptors, authenticator, pinner and listeners): no read timeout and no call timeout (a quiet stream is not a
   dead one; the call timeout covers the body), and HTTP/1.1 only (as `okhttp-sse` and the default adapters do: the unread data of
   a stalled HTTP/2 stream counts against the connection's window, which the app's other requests share). The reader is the one the
   default adapters use (`SseStreamReader`: UTF-8, the HTML standard's parser, one chunk read only while the binding's buffer has room).

6. **WebSocket on OkHttp: what it cannot do**, found by running the shared suite and pinned by tests:
   * OkHttp reads frames on its own thread and cannot pause it; it does wait for the listener to return before reading the next
     frame, so the adapter's listener does not return while the binding's buffer is full: TCP pushes back, with one frame read
     beyond the window. (A core that stops reading for longer than the ping interval loses the connection to OkHttp's missing-pong
     timeout, because the reader cannot read the pong; a client with `pingInterval(0)` does not.)
   * **A text frame that is not UTF-8 is delivered with U+FFFD for the bad bytes**, not `Protocol`: OkHttp decodes text itself and
     the adapter never sees the bytes (RFC 6455 section 8.1 wants 1007). The shared suite marks this adapter
     `rejectsMalformedText = false` and a case of its own pins what happens. An app that needs the check keeps `ClientWebSocketAdapter`
     for the port. OkHttp also has no inbound size limit, and a message over its 16 MiB outgoing queue closes the connection with 1001.
   * OkHttp opens a WebSocket on a client it derives from the app's with **no event listener** (`EventListener.NONE` in
     `RealWebSocket.connect`) and runs the upgrade **without the client's network interceptors**: OkHttp's rules, not the adapter's,
     pinned by a test. Application interceptors, the authenticator, the pinner, `Dns` and the proxy apply.
   * A connection the network silently dropped is noticed only by sending: unless the client has a ping interval, the adapter gives
     the connection 30 s (the default adapter's silence limit); `pingIntervalMillis` overrides, `0` disables.

7. **`:runtime` makes two classes public, behind `@UndraEmbeddingApi`**: `SseStreamReader` (an `SseStream` over any blocking
   `InputStream`: the reader thread, the parser, the gate) and `ReadAheadSource` (what a connection implements for the binding to
   tell it how much room its buffer has). They are what the default adapters already used; an adapter over another HTTP client needs
   them or must write them again. No behavior changes; the opt-in says they may change between releases.

8. **Apple.** `URLSessionWebSocketAdapter(session:)` is added: connections are tasks of the app's session (`session.webSocketTask`),
   with the adapter as the task's delegate (iOS 15, the package's floor), so the session's delegate (server trust, pinning, the
   authentication challenge), its configuration (`httpAdditionalHeaders`, `protocolClasses`, proxy, cookie storage) and its
   delegate queue apply to the upgrade; the adapter never invalidates a session it was given. `HttpAdapter(session:)` and
   `URLSessionSseAdapter(session:)` exist. An app gives all three its session, and gives the event streams a session whose
   `timeoutIntervalForRequest` allows a quiet stream and whose `httpMaximumConnectionsPerHost` allows many (the defaults of
   `URLSessionSseAdapter.makeDefaultSession()`), sharing the delegate. Documented in the Swift runtime's README and in the
   cookbook.

9. **TypeScript** needs no code: `fetchHttp({ fetch })`, `fetchSse({ fetch })` and `browserWebSocket({ WebSocket })` take the
   app's own (a wrapper that adds a token, retries a `401`, traces); a test pins that a wrapped `fetch` sees every request and
   can change a header or answer again. React Native's `reactNativeHttp()` is React Native's `fetch`, which is the app's networking
   module (its OkHttp on Android, `NSURLSession` on iOS), so the app's customizations of that module apply as they do to every other
   request. Documented in the cookbook recipe.

## Alternatives considered

* **Keep `HttpURLConnection` only and document "write a custom adapter"** (the status quo): rejected. The contract is hundreds of
  lines, and each team that writes it gets a different subset right. The ports are a good design whose price is too high for the
  common case, which is what this removes.
* **OkHttp as the default adapter in `android-adapters`**: rejected. An app without OkHttp would carry a second HTTP stack next to the
  platform's; the runtime modules' dependency rule (`:runtime` is stdlib and coroutines, `:android-adapters`
  is the Android SDK and `:runtime`) exists so that adopting Undra adds nothing. It also would not help the app whose client is
  configured: the default would be a client of Undra's.
* **An interceptor or middleware port of Undra's own** (a chain around `Http`, `WebSocket` and `Sse`): rejected. It reinvents
  OkHttp's interceptors, `URLSession`'s delegate and `fetch` wrappers, three times, in a place the app would have to configure
  a second time, with an ordering rule to learn; the app's stack is already the one place its policy lives.
* **`Call.Factory` instead of `OkHttpClient`**: rejected. `OkHttpClient` is what apps hold; a WebSocket needs `WebSocket.Factory`
  and the stream needs `newBuilder`. The `() -> OkHttpClient` constructors cover a client that changes.
* **Executing the call on `Dispatchers.IO` with `execute()`**: rejected. It bypasses the app's `Dispatcher`, so a client tuned to
  two calls per host (to protect a backend) would see the core's requests in addition.
* **Ktor, Cronet or a pluggable `HttpEngine` interface in the module**: out of scope; the ports are that interface.

## Consequences and proof (R4)

* Tests. `:okhttp-adapters` on the JVM and on the device (`emulator-5554`, AVD `undra`): the Http contract (the cases of
  `AndroidHttpAdapterTest` plus one per OkHttp setting: an interceptor sees every request and changes a header, an `Authenticator`
  refreshes a token and the request, its body included, goes again, an event listener sees each call, a client provider is asked
  every time, a call timeout and a redirect policy are respected, gzip is decoded, a header that is not ASCII is refused naming
  it), the realtime contract through `WebSocketPortAdapter` and `SsePortAdapter` against `contract-tests/servers/realtime-server.mjs`
  (plus: an interceptor tags the upgrade and the stream, a client's read and call timeouts do not end a quiet stream, the derived
  clients share the app's pool, dispatcher and interceptors, the malformed-text behavior), and `installWithOkHttp` on the device.
  The Swift WebSocket suite runs a second time on an app's session, with a recording delegate; the TypeScript suite gains the
  wrapped-`fetch` cases. Each OkHttp mutation that matters (no cancel on cancellation, no read gate, the client's read timeout kept
  on a stream, restricted headers kept, a stream that is not cancelled on close) was shown to fail the suite.
* No benchmark row (R4, R9): nothing here touches the boundary; the adapters are host code under the ports and do the same
  per-call work as the ones they sit beside.
* Size: nothing for an app that does not add the module; for one that does, OkHttp is already in its APK.
* Compatibility: built against OkHttp 4.12.0 (the line most apps are on) and run once against 5.3.2 (the same 66 JVM unit tests, which
  pass), since Gradle gives an app on 5.x the highest version. No other module depends on it.
* Risk and what to watch: the list in decision 6 is OkHttp's, not ours; the one with a security flavor is that malformed UTF-8 in a
  WebSocket text frame is repaired, not rejected, which is what the app's other OkHttp sockets do too.

## Implementation notes (2026-10-02)

What the implementation found or decided where the text above left room:

* **The shared suites came out of the existing ones, and the existing counts did not move.** `AndroidHttpAdapterTest` is now
  `HttpAdapterContract` plus a 20-line subclass (`:android-adapters`: 151 JVM results per variant, 1 skipped, as before; 150 on the
  device, 1 skipped); `RealtimeAdapterTests` is `RealtimeAdapterContract` plus a subclass (`:runtime` under `test-local.sh`: the same
  suites, the realtime one 23 cases). To let `:okhttp-adapters` run them, `Suite`, its assertions, the realtime test server and
  `eventually` moved from `runtime/src/test` to `test-support` (same packages, so no import changed), and the loopback HTTP server,
  `RecordingCore` and the Http contract live in `adapter-contracts` (JUnit 4, which both Android modules use). The contract
  parameters are the things a platform may differ in that a test can observe (`maxRedirects`, whether `PATCH` can be sent, whether a
  cancelled call closes the socket mid-body, the order of a repeated header, `rejectsMalformedText`, `canAbortBlockedRead`).
* **OkHttp passed the Http and Sse contracts at the first run, and the WebSocket one with one declared difference** (decision 6, the
  UTF-8 check). Nothing was loosened to get there: the differences are the parameters above and the cases that say what OkHttp does.
* **A failure travels as a value and is thrown by the adapter, not resumed through a coroutine** (`Outcome` in the Http adapter, the
  `WsError?` of `connect`, `Answer` in the Sse adapter): under `-ea` (every Gradle test run) kotlinx-coroutines rebuilds an exception
  that crosses a resume by reflection, and a typed error with one `String` constructor would come back with its own message as its
  `reason`.
* **Mutations shown to fail the suite**: no `Call.cancel()` on cancellation (three Http contract cases), no wait in the WebSocket
  listener (the realtime suite's stalled reader), the client's read timeout kept on a stream (the quiet-stream case), restricted
  headers sent, a stream not cancelled on close. The Swift `session:` branch replaced by a session of its own fails the header and
  the delegate assertions.
* **OkHttp 5.x**: the module cannot be built against 5.x with this repository's Kotlin 2.0.21 (5.x's metadata is 2.2), and AGP
  keeps the compile and runtime class paths aligned, so a Gradle property cannot put a newer OkHttp only on the test runtime. The
  66 JVM unit tests of that day were run by hand against OkHttp 5.3.2 (the module's classes compiled against 4.12.0, the 5.3.2 Android
  artifact, Okio 3.16.4, Kotlin stdlib 2.2.21, stubs for `android.util.Log` and `Build.VERSION`): 66 of 66 passed.
* **The per-task delegate** (Swift, decision 8) delivers the WebSocket handshake and close callbacks on macOS 26.x here: the whole
  WebSocket suite passes on an app's session. Not run: iOS 15 or 16 (the package's floor), where `URLSessionTask.delegate` exists but
  was not exercised; the existing platform notes on the close frame (`waitForTheClientsClose`) apply to both forms.
* **What no test shows**: certificate pinning end to end. A TLS test server needs a certificate generator (`okhttp-tls`, which is
  another artifact than the approved one); the mapping of a pin that does not match to `Network` is a unit test on the exception
  OkHttp throws, and the pinner sits in the client the tests already show is in the path.
* **Docs**: `okhttp-adapters/README.md`, the Android and Swift READMEs, `docs/SPEC.md` (8, 11, 17.2, 17.3), the ports page, the
  cookbook recipe "Your network stack" (the Kotlin lines are compiled and run in `:okhttp-adapters`, which has OkHttp; the Swift and
  TypeScript lines by `snippets/check.sh`), the test table of `docs/ONBOARDING.md`.

## Review fixes (2026-10-02)

The adversarial review (`.10x/reviews/2026-10-02-okhttp-adapters-review.md`) changed four things, each with a test that failed first:

* **`installWithOkHttp` let the platform's adapters serve a request.** It called `install`, which registered `Kv` and then the
  platform's `Http`, and replaced the network ports only when `install` returned; a core replays its offline queue as soon as `Kv`
  answers, so a queued request could go out through `HttpURLConnection`, without the app's token or pin. `:android-adapters` gains
  `AndroidPlatformDefaults.installWithNetworkPorts(core, context, http: PortImpl, webSocket: WebSocketPortAdapter, sse:
  SsePortAdapter, ..)` behind `@UndraEmbeddingApi` (its `AndroidPlatform.http` is an `AndroidHttpAdapter` that is not registered),
  and `install` registers the three network ports before `Kv` (for the defaults too: a replay no longer meets an `Http` port that is
  not there yet).
* **`OkHttpSseAdapter` refused an event id that is not ASCII** (OkHttp's checked header), so such a stream could not resume. It sends
  `Last-Event-ID` as its UTF-8 bytes (`Headers.Builder.addUnsafeNonAscii`), as the fetch standard and Android's `HttpURLConnection`
  do. The contract has the case; the desktop JVM's `java.net.http`, which writes header values as US-ASCII, declares that it cannot
  (`sendsNonAsciiLastEventId = false`).
* **A `GET` or `HEAD` with an empty body** was `InvalidUrl` on OkHttp and a `POST` on `HttpURLConnection` (`doOutput`); both send the
  method without a body now.
* **`undra upgrade` left `dev.undra:okhttp-adapters` behind** the runtime it is compiled against; it moves it with the others.

Contract cases added: the core's close racing the peer's close frame (the pending receive is answered once, the stream then ends
cleanly), the core's close reaching a server it stopped reading (code and reason), a non-ASCII `Last-Event-ID`, the empty-body
`GET`/`HEAD`; and for OkHttp: cancelled and timed-out calls leave nothing in the dispatcher or the pool, a quiet WebSocket outlives
the client's read and call timeouts, and the upgrade is seen by application interceptors only.
