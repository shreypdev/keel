# ADR-047: `WebSocket` and `Sse` standard ports: pull-based inbound streams with the core's credit, reconnection in the core

Status: **Proposed** (2026-10-01, piece G2 of the v1.x plan, `wt/ports-v2`; the founder approved the bet in
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
   answers as soon as at least one item is buffered, with at most `max`; `Ok([])` means the stream ended because
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
   | Kotlin (JVM, Android) | `WebSocketAdapter.connect(..): WebSocketConnection` (`messages: Flow<WsMessage>`, `suspend send`, `close`) | the runtime's own RFC 6455 client (ADR-051), gaining text, subprotocols, headers and a read gate; `java.net.http.WebSocket` is absent on Android at every API level (minSdk 26) | `HttpURLConnection` streaming |
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
