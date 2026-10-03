# ADR-047: `WebSocket` and `Sse` standard ports: pull-based inbound streams with the core's credit, reconnection in the core

Status: **Accepted** (2026-10-02, after the adversarial review of `wt/ports-v2`,
`.10x/reviews/2026-10-02-ports-review.md`; see "Implementation notes" at the end for the deviations). Proposed
2026-10-01 (piece G2 of the v1.x plan, `wt/ports-v2`; the founder approved the bet in
Amendment A of `.10x/specs/2026-10-01-v1x-default-choice-design.md`). Touches SPEC 8 (two opt-in ports and five
types), 10.5 (the `stdlib` table), 11 (default adapters), 17 (the runtimes' adapter interfaces); `undra-ports`,
`undra` (features), `undra-bindgen` (`stdlib`), the Swift, Kotlin, TypeScript and React Native runtimes.
**No wire, envelope, C ABI or wasm ABI change; no existing schema hash moves** (decision 1). R5, R6, R11, R12.

## Context

Real-time is matrix rows 12 and CRUX-5 of the competitive catalogue: RN and Flutter ship WebSocket, KMP's
Android Ktor engine has none, Crux leaves SSE as an example. Undra has WebSocket only as its dev transport.
The port model is request/reply (PortCall/PortReply, SPEC 3.6) plus broadcast events (kind 11). Neither carries
a per-connection inbound stream with backpressure: events have no credit and fan out to every subscriber, and
§3.7 streams flow core→host only. A host→core stream kind would be a wire revision (R7) for one feature.

## Decision

1. **Opt-in, so nothing moves for anyone else.** The ports live in `undra-ports` behind cargo features
   `websocket` and `sse` (forwarded by the facade: `undra = { features = ["websocket", "sse"] }`), off by default.
   A core without them has the schema, hash and wasm it has today (0 bytes: the code is not compiled). Ports are
   registered by `#[undra::port]` inside `undra-ports`, so ADR-052's "link by use" cannot select them; a feature is
   the only switch that keeps registrations out of the schema.
2. **The ports** (all methods async with an error channel, ids per SPEC 1.1):
   ```rust
   #[undra::port] pub trait WebSocket {
       async fn connect(&self, url: String, protocols: Vec<String>, headers: Vec<Header>) -> Result<WsOpened, WsError>;
       async fn send(&self, conn: u32, message: WsMessage) -> Result<(), WsError>;
       async fn receive(&self, conn: u32, max: u32) -> Result<Vec<WsMessage>, WsError>;
       async fn close(&self, conn: u32, code: u16, reason: String) -> Result<(), WsError>;
   }
   #[undra::port] pub trait Sse {
       async fn open(&self, url: String, headers: Vec<Header>, last_event_id: Option<String>) -> Result<u32, SseError>;
       async fn next(&self, stream: u32, max: u32) -> Result<Vec<SseEvent>, SseError>;
       async fn close(&self, stream: u32) -> Result<(), SseError>;
   }
   WsOpened { conn: u32, protocol: String }            // protocol "" when none was negotiated
   WsMessage { Text(String) = 0, Binary(Bytes) = 1 }
   WsError  { Refused { status: Option<u16>, message: String } = 0, Network(String) = 1, Protocol(String) = 2,
              Closed { code: u16, reason: String } = 3 }
   SseEvent { id: Option<String>, event: String, data: String, retry_ms: Option<u32> }   // event "message" by default
   SseError { Refused { status: Option<u16>, message: String } = 0, Network(String) = 1, Protocol(String) = 2, Ended = 3 }
   ```
   Connection and stream ids are chosen by the adapter, unique per adapter instance, never reused.
3. **Inbound is pulled, and the pull is the credit (§3.7's numbers).** `receive(conn, max)` / `next(stream, max)`
   answers with at most `max` items once it has `max`, or once it has at least one and 2 ms passed without a new
   one, or 8 ms after its first (a burst is one reply, not one per frame; a lone message waits 2 ms); `Ok([])` means the stream ended because
   *the core* closed it; an `Err` ends it otherwise. At most one pull is outstanding per connection. The adapter
   reads ahead at most `max` items of the latest pull (16 before the first) beyond what the core has received:
   where the platform can pause reading (URLSession `receive()`, the Kotlin client's reader, an SSE byte stream)
   it pauses, so TCP pushes back on the server; where it cannot (browser, Node and React Native `WebSocket`) it
   buffers up to 4,096 messages or 16 MiB, then closes with 1008 and ends the stream with
   `Closed { code: 1008, reason: "the core did not keep up" }`. The Rust stream keeps one pull in flight and asks
   for 16, issuing the next pull while it still holds fewer than 8, so delivery is pipelined, never one round
   trip per message. A port call per batch, not per message: a burst of 16 frames is one crossing.
4. **Outbound waits for the platform.** `send` completes when the platform accepted the message and its
   outbound buffer is under 1 MiB (browser: `bufferedAmount` polled per frame), so a core that awaits its sends
   cannot outrun the network. Inbound and outbound are independent: a slow reader does not block a sender.
5. **Close and error semantics** (every end is typed, nothing is read from text):
   | What happened | inbound stream ends with |
   |---|---|
   | the core called `close` | clean end (`Ok([])` → `None`) |
   | the peer sent a close frame (1000 included) | `Err(Closed { code, reason })` |
   | the connection dropped without a close frame | `Err(Network(..))` |
   | RFC 6455 violation, a text frame that is not UTF-8, an SSE reply that is not `text/event-stream` | `Err(Protocol(..))` |
   | the upgrade / SSE request was answered non-2xx, the URL is unusable | `connect`/`open` → `Err(Refused { status, .. })` |
   | the SSE response body ended | `Err(Ended)` |
   `Refused.status` is `None` where the platform hides it (browser `WebSocket`). Headers a platform cannot send
   (browser `WebSocket`) are refused, never dropped: `Refused { status: None, "this platform cannot send
   WebSocket headers: put the credential in the URL or a subprotocol" }`. `From<PortError>`: `Unavailable` →
   `Network("the WebSocket port has no adapter registered (E0062: ..)")`, `Cancelled` → `Network("cancelled")`,
   `Decode` → `Protocol("malformed port reply: ..")`, `Failed(bytes)` → the decoded error (as `HttpError`).
6. **Reconnection is not in the port; the core does it with `Timer`.** The policy is app logic (which close codes
   retry, backoff, re-subscribe messages, refreshing a token, resuming SSE from `last_event_id` after `retry_ms`)
   and must be deterministic and testable with `FakeClock` (R12); a port that reconnected by itself would lose
   or replay messages the core cannot see and would be written five times (ADR-051's transport reconnect was
   three implementations of one formula). `undra_ports::Backoff` (ADR-051's formula, jitter from `Rng`) and a
   documented 20-line loop over `weak.sleep` (ADR-034) are the whole story.
7. **Rust surface** (`undra_ports::ws`, `undra_ports::sse`): `WsConnection::connect(ctx, url, WsOptions { protocols,
   headers }) -> Result<WsConnection, WsError>` with `send`, `send_text`, `send_binary`, `messages() ->
   impl Stream<Item = Result<WsMessage, WsError>> + Send`, `close(code, reason)`; `sse::subscribe(ctx, url, headers,
   last_event_id) -> impl Stream<Item = Result<SseEvent, SseError>> + Send` (an `open` failure is its only item).
   Dropping either without closing closes it through a `WeakCtx` (fire and forget, ADR-034).
8. **Fakes** (`undra_ports::fakes`, deterministic, no threads): `FakeWebSocket` scripts the server per URL
   (`accept(protocol)`, `refuse(status)`), `push(conn, msg)`, `close_from_server(conn, code, reason)`,
   `drop_connection(conn)`, an `echo()` mode; records sends and closes; and exposes `pulls(conn)` (every `max`
   asked) and `delivered(conn)` for backpressure assertions. `FakeSse` likewise (`push`, `end`, `fail`, the
   `last_event_id` each open carried). `fakes::install` installs them when the features are on.
9. **Platforms.** The runtimes ship the types and a binding (`webSocketPort(adapter)`) that owns ids, the pull and
   the read-ahead, over an adapter interface that reads natively (R3):
   | | adapter interface | default WebSocket | default SSE |
   |---|---|---|---|
   | Swift | `WebSocketAdapter.connect(..) async throws(WsError) -> any WebSocketConnection` (`messages: AsyncThrowingStream<WsMessage, Error>`, `send`, `close`) | `URLSessionWebSocketTask`, `receive()` only while the window has room | `URLSession.bytes(for:)`, lines parsed |
   | Kotlin (JVM, Android) | `WebSocketAdapter.connect(..): WebSocketConnection` (`messages: Flow<WsMessage>`, `suspend send`, `close`) | the runtime's own RFC 6455 client (ADR-051), gaining text, subprotocols, headers and a read gate; `java.net.http.WebSocket` is absent on Android at every API level (minSdk 26) | `java.net.http` on the JVM (`HttpURLConnection` cannot abort a blocked chunked read there), `HttpURLConnection` on Android |
   | TypeScript (browser, Node; main and worker) | `WebSocketAdapter.connect(..): Promise<WebSocketConnection>` (`messages(): AsyncIterable<WsMessage>`) | `WebSocket` (Node 22+: the global, with `headers`) | `fetch` + body stream + parser; not `EventSource`, which sends no headers, hides a refusal's status and reconnects on its own |
   | React Native | as TypeScript | the `WebSocket` global (its `{ headers }` argument) | `fetch` body streams where present, else `XMLHttpRequest` progress events |
   Swift and Kotlin (both Android via `AndroidPlatformDefaults.install`) and React Native include them in their
   defaults; TypeScript ships them as an opt-in subpath (`@undra/runtime/realtime`) so the hello-world bundle
   (ADR-052's 26,000-byte gate) does not grow. In `wasm-worker` mode both ports cross to the main thread like every
   async port (ADR-049 §2); a pending pull is one pending port call per connection.
10. **`stdlib`** gains the two ports and five types (pinned ids, `feature` noted); bindgen leaves them out of a
    core that has them and refers to the runtime's types; goldens: `stdlib` gets a case with the features on.

## Alternatives considered

* **A host→core stream kind on the wire** (credit-carrying `PortStreamItem`): the exact shape of §3.7 inverted, but
  a wire revision, a C/wasm ABI addition and five runtime transports for what a batched pull does with today's
  wire at one crossing per 16 messages.
* **Events for inbound messages**: no credit, broadcast to all subscribers, no per-connection end.
* **Reconnect in the adapter** (6): rejected; **`EventSource` on the web** (9): rejected; **OkHttp on Android**: a
  dependency in `android-adapters` for what the runtime's own client already does on both JVM and Android.
* **Unconditional ports**: every core's hash would move and the hello world would grow (~4 KB gz measured for a
  port of this size by ADR-052's row 9).

## Consequences and proof (R4)

* Contract scenarios **S23** (WebSocket: echo, credit-bounded read-ahead, typed closes, refusal, drop-closes) and
  **S24** (SSE: events with ids, `last_event_id` resume after `Ended`, refusal) on the Swift, Kotlin and TypeScript
  columns, against a scripted in-process server written to each runtime's public adapter interface; the default
  adapters are covered by each runtime's own failure-injection suite against a local server (refused upgrade,
  peer close, abrupt drop, flood under a stalled reader, non-UTF-8 text, wrong content type).
* Bench row `ports/ws_roundtrip` (one message out and back through the port path, `bench/budgets.toml`).
* SPEC 8, 10.5, 11, 17 and `site/docs/realtime.html`. Hello-world wasm and JS: unchanged within tolerance.

## Implementation notes (2026-10-02, accepted after the review)

What the implementation and the review decided where this text left room, and where they depart from it:

* **A pull answers a burst as one reply** (decision 3, added during the work): at once with `max`, else once one is
  there and 2 ms passed without another, or 8 ms after its first. The Swift runner measured 6 pulls in S23.3
  without it. React Native's timers fire on frame boundaries, so there the quiet period is one frame: a lone
  message reaches the core about 16.7 ms after it arrived (measured on both OSes; documented in
  `docs/REACT_NATIVE.md`).
* **SSE on the JVM is `java.net.http`** (decision 9's table said `HttpURLConnection` for both): on JDK 17
  `HttpURLConnection.disconnect()` does not abort a read blocked on a chunked body, so S24.3 could not pass. Android
  keeps `HttpURLConnection`, where it does.
* **TypeScript registers the ports through `LoadOptions.ports`**, not adapter keys (adapter keys would pull binding
  code into the main entry). The review moved the ids out of the main entry too: `OptInPortIds` (`WebSocket`, `Sse`,
  `Db`) is exported by `@undra/runtime/realtime` and `@undra/runtime/db`, because ADR-052's 26,000-byte gate had 16
  bytes of headroom and `PortIds` carrying them put the hello world 132 bytes over. What the main entry still
  carries is the `PortImpl.dispose` call when the core closes (25,996 B gz after the review); a crash restart
  (ADR-049) disposes from `recovery.ts`, and `registerPort` no longer disposes the port it replaces.
* **S23's TypeScript column runs `nodeWebSocket()`** (Node's `http` upgrade with the runtime's RFC 6455 framing):
  Node's global `WebSocket` hides a refusal's status, refuses to send 1001 and reports bad UTF-8 as a drop.
  `nodeWebSocket` pauses the socket, but only after parsing one socket read (at most 64 KiB on the wire, measured
  up to ~13,000 tiny frames queued against a read-ahead of 16): bounded, TCP pushes back, not "at most `max`"
  (open item).
* **Kotlin's error fields are `reason`** (`WsError.Closed(code, reason)`, `Refused(status, reason)`, like
  `HttpError.Network(reason)`): `message` stays the exception's Display text, which is what Kotlin readers expect
  of `message`.
* **Platform ends that differ** (typed everywhere, documented): a browser or React Native `WebSocket` reports every
  failure before `open` as `Refused { status: null }` (DNS included); script cannot send 1001 or 1008, so a server
  sees 1005 when the binding closes going away; React Native on iOS may report a dropped connection as
  `Closed(1001, "Stream end encountered")` (it forwards no `wasClean`; seen once in the review, `Network` in the final
  device run), Android as `Network`.
* **Review fixes:** a `connect`/`open` whose caller was cancelled while it crossed left the connection or stream
  open (Rust runs those calls in a task of its own that closes what the late answer names; the Kotlin and
  TypeScript bindings close what opens after a detach); a crash-restarted web core now releases what the trapped
  instance held (the bindings count disposals instead of dying); `URLSession`'s default of 6 connections per host
  left a 7th SSE stream to one host waiting in `open` (the default session allows 1,024); `browserWebSocket`
  refused one message larger than its byte limit with nothing queued; `fetchSse` sent a non-Latin-1
  `Last-Event-ID` (now UTF-8 bytes); a raw `WsError`/`SseError` from its own port answered status 2 on Kotlin.


## Amendment: the Swift adapter delivers chunks, 2026-10-02

No public shape, wire, ABI or schema change (`URLSessionSseAdapter.init(session:)`, `SseAdapter`, `SseStream` and every decision above
stand), so no new ADR; the implementation of decision 9's Swift row changes. User feedback U1 measured the iOS SSE ceiling 2.4x below
what it should be, and the code confirmed it: `URLSessionSseStream` pulled `URLSession.AsyncBytes` one byte at a time and gave the parser
`CollectionOfOne(byte)`. The Kotlin adapter reads chunks, and so does the TypeScript one.

* **What it is now.** The default Swift Sse adapter is the delegate of a `URLSession` data task (`task.delegate`, so the session passed to
  `init(session:)` is used as before). `urlSession(_:dataTask:didReceive:)` hands whole `Data` chunks to `SseParser.push`, once per chunk,
  and the events queue for the binding's pump. The head is checked in `didReceive response` (the same refusals: non-2xx with its status, a
  204, a wrong content type); the end is `Ended`, a failure `Network`, bytes that are not UTF-8 `Protocol` (after the events before the
  bad line, through an internal `SseParser.push(_:into:)`); `close` cancels the task; cancelling the caller of `open` cancels the request.
* **Backpressure is decision 3, by the Kotlin adapter's rule, with the same names** (`SseStreamReader`): the socket is read only while
  fewer events `waiting` than the binding's buffer has `room` for (the window, 16 before the first pull). A delegate has no pull, so the
  task is suspended when `waiting` reaches `room` and resumed when a pull takes it below, and URLSession stops reading and TCP pushes back.
  The read-ahead is `room` plus at most one chunk plus the few chunks already in flight. **The task is suspended before a chunk is parsed,
  not after**: parsing takes time, URLSession reads on meanwhile and hands the lot over in one piece (3.7 MB chunks; its dispatch-data
  concatenation dominated a profile), so a suspend that came after the parse stopped nothing and the stalled-core flood test wrote 41,451
  of 100,000 events (it holds at 43 KB read with the suspend first).
* **Why not keep `AsyncBytes`.** Measured both on this machine. Iterating `AsyncBytes` is cheap (about 0.7 ns a byte counting alone, 4 KB
  events); the old loop around it cost about 19 ns a byte. `AsyncBytes` has no way to take what is already buffered, so the only
  bulk unit that keeps an event's latency is a line: feeding the parser once per line over `AsyncBytes` reached 143 to 177 MB/s on 4,096
  byte events against 344 to 408 MB/s for the delegate (equal at 512 bytes). The delegate has the higher ceiling and the same shape as
  the Kotlin adapter.
* **Measured** (Apple M5 Pro, macOS 26.5, Swift 6.3.3, the shared Node `/sse/flood`, best of three passes; release by a throwaway harness
  against the package, debug by `swift test`'s `BENCH swift sse/...` lines in `SseThroughputTests`):

  | | before | after |
  |---|---|---|
  | release, 4,096 B events | 12,600 events/s (52 MB/s) | 84,000 to 99,000 events/s (344 to 408 MB/s): 7.8x |
  | release, 512 B events | 46,000 to 48,000 events/s (24 to 26 MB/s) | 96,000 to 104,000 events/s (51 to 55 MB/s): 2.1x |
  | release, 64 B events | 39,000 to 43,000 events/s | 36,000 to 46,000 events/s: unchanged |
  | debug, 4,096 B events | 967 events/s (4.0 MB/s) | 4,030 to 4,130 events/s (16.6 to 17.0 MB/s): 4.2x |
  | debug, 512 B events | 6,501 events/s (3.4 MB/s) | 27,600 to 29,200 events/s (14.6 to 15.5 MB/s): 4.3x |

  At 64 bytes an event the limit is URLSession's own receive path, which no adapter passes: a delegate that does nothing read 100,000
  events of 49 bytes at 11,000 a second.
* **Proof (R4).** `SseChunkTests.swift`: a body cut at every byte, one byte a chunk and seeded three-way cuts parse as the whole; a
  recorded task shows the suspend and resume rule; a loopback server that sends exactly what a test says shows a real task seeing every
  byte as its own chunk, resuming after a pull and cancelling with `open`; `testAnSseFloodStallsTheServerWhileTheCoreDoesNotPull` and every
  other SSE test pass unchanged; the Swift contract column is 33 of 33 (S24 included). Record: `.10x/decisions/sde/sse-chunks.md`.
* **Review (`.10x/reviews/2026-10-02-sse-chunks-review.md`).** *The app's session delegate.* The stream implements only the response,
  data and completion callbacks; URLSession forwards the rest to the session's delegate ("methods not implemented on this delegate will
  still be forwarded", `NSURLSession.h`). Over TLS (`SseSessionDelegateTests`): a session-level pinning delegate decides the stream's
  server trust (a pin that does not match refuses it), a task-level one gets the trust and HTTP Basic challenges, both get the metrics.
  This is better than before: `bytes(for:)` never asked a session-level `urlSession(_:didReceive:completionHandler:)`. The trade-off is
  that the parse runs on the session's delegate queue (main, if the app's session uses `.main`; documented on the type). *Counted
  suspends.* `URLSessionTask` counts suspends, and a `resume` of a running task is not a no-op (it cancels the next `suspend`), so the
  stream calls them strictly in turn (a seeded test). *Cancellation.* A cancelled pull ends the stream with `Network("cancelled")` and
  cancels the request, as an `AsyncBytes` read did (the first version left it waiting). *The parser* now splits lines on bytes, not
  `Character`s (a combining mark after `:`, the space or NUL was joined to it, unlike Kotlin, TypeScript and the standard), and takes
  runs between line ends: the parser alone in release 310 to 2,246 MB/s on 4,096 B events, and the adapter end to end (release harness,
  interleaved, a loaded machine) 57,000-67,000 to 95,000-117,000 events/s on 4,096 B events.
