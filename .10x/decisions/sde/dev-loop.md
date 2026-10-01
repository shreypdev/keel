# SDE — the dev loop: Android remote mode and dev-client reconnect (wt/dev-loop, 2026-10-01)

Tracks B1 and B2 of the v1.x design (`.10x/specs/2026-10-01-v1x-default-choice-design.md`). The decision is
ADR-034 (written first, R11: it touches the dev transport contract and public runtime API; the envelope, the C
ABI, the wasm ABI, the schema and every generated shape are unchanged, and bindgen goldens are byte-identical).
`docs/DEV_LOOP.md` is the user-facing page; this note is the record for the integrator.

## What was missing for Android (all of it, as the brief guessed)

1. **The transport.** The Kotlin `RemoteTransport` was built on `java.net.http.WebSocket`: not on Android. The
   runtime now has its own RFC 6455 client (`WebSocketClient.kt`, ~330 lines over `java.net.Socket`): handshake with
   `Sec-WebSocket-Accept` check, masked client frames, fragmentation, ping/pong, the closing handshake, a size cap,
   `wss` through the platform's `SSLSocketFactory`, a client-side ping of a quiet server (declares it dead after two
   intervals, so a vanished laptop is noticed in seconds). The same code runs under the JVM tests and on a phone.
2. **Network on the main thread.** Android throws `NetworkOnMainThreadException` for socket I/O on the main
   thread. The client has a reader and a writer thread; `sendBinary` only enqueues; `RemoteTransport.connect` runs
   the connect and handshake on a thread of its own. `UndraCore.load(REMOTE)` therefore works from the main thread
   (it blocks it for the connect, like any dev-only synchronous call) and nothing else touches a socket on it.
3. **The app.** The playground manifest had no `INTERNET` permission and no cleartext policy, and `UndraApp`
   always loaded the in-process core. Now: a `debug` build type with `buildConfigField UNDRA_DEV_URL`
   (`-PundraDevUrl=`), `src/debug/AndroidManifest.xml` (`INTERNET`, `usesCleartextTraffic`) merged into debug only
   (verified: the merged release manifest has neither), the `undra_dev_url` launch extra as the run-time switch
   (extra wins over build property; release ignores both), `UndraApp.start(url)` called from the activity,
   an unreachable-server screen with Retry, a status bar, and the restart on a lost session.
4. **`undra dev`.** It printed no Android address and the help said "no remote transport on Android". It now prints
   the emulator URL (`10.0.2.2:<port>`), the `adb reverse` line and the launch command (from `undra.toml`'s id), and
   `--android` runs `adb reverse` for every ready device (or `ANDROID_SERIAL`), at start and after each restart
   (`crates/undra-cli/src/adb.rs`, tested with `FakeSys`).
5. **Docs.** `cli.rs`, the site's cli and getting-started pages (generated files rebuilt with
   `node site/scripts/build-all.mjs`), the playground and Android READMEs, `undra init`'s README texts, the adopt
   guide, both runtime READMEs and SPEC 3.2/11/17 said Android had no remote mode or "reload the app".

## B2: the reconnect design

Layering: the transport owns the socket and the timer; the core owns what is replayed.

* **Transport** (`RemoteTransport` in TS and Kotlin, `WebSocketTransport` in Swift): on an unexpected close it tells
  its core `reconnecting(1, error)` (attempt 1 is the loss), waits `min(5 s, 250 ms * 2^(n-1))` less up to half at random,
  connects (at most 5 s per attempt), and tells the core `reconnected`. Final (`closed`): the app's `close()`, a schema
  mismatch on the server's `Hello` (also checked on every reconnect, reported once), close code 4001, `maxAttempts`, a
  protocol error (TS, Kotlin). The jitter source and the clock are injectable: TS uses fake timers, Kotlin a recording
  `Sleeper`, Swift a `ManualScheduler`; the schedule is a table in each runtime's tests.
* **Core**: tracks the observed `(handle, signal)` pairs, the handles its constructors made and the handles released
  while down. On `reconnecting(1)` it fails everything in flight with the platform's existing outcome; calls while down fail
  at once the same way (nothing is queued). On `reconnected` it sends `Release`s and `Observe`s and only then reports
  `connected`; a loss that happens during the replay does not announce a connection (an epoch counter).
* **State API**: TS `core.connection` (a `Signal`) + `onConnectionChange`; Kotlin `connectionState` (`StateFlow`) +
  `LoadOptions.onConnectionChange`; Swift `connectionState`, `@Observable` `connection.state`, `connectionStates()`,
  `LoadOptions.onConnectionChange`. States: connecting / connected / reconnecting(attempt) / closed(reason).
* **Server** (`crates/undra-transport/src/resume.rs`, `session.rs`): the deviation worth reading. A reconnect that
  re-observes is hollow if the server already released the client's objects at disconnect (`release_on_disconnect`),
  so a client now puts a random token in its URL (`?undra_session=`; `&undra_resume=1` when it holds constructed
  objects) and `ServerConfig::resume_grace` (default 0 = off; the dev runner sets 10 min) keeps a dropped client's
  constructed objects. Released on expiry, when another client attaches, or at shutdown (so at most one launch's
  objects are held). Resume adopts them into the new connection; a resume the server cannot honour (core restarted) is
  answered with the server's `Hello` and close code **4001**. A client back under its own token evicts its stale
  socket at once. The server logs connect / reconnect / kept / released / expired / refused.
* **What a rebuild looks like until B3**: the new runner has no session, so the client is told 4001 and closes the
  core (`sessionLost`) instead of failing every call with a stale handle. The playgrounds and templates load the new
  core and start over (web: `location.reload()`; iOS: `coreLost` -> `reload()` + `.id(epoch)`; Android: `UndraApp`
  -> epoch -> the activity restarts). State is not preserved across a rebuild: that is B3, and the seam is a plain
  `(token, handles)` entry in the server's retained slot (a restored session can be seeded there).

## Findings and things the integrator should know

* **Foundation does not give the close code reliably.** On `URLSessionWebSocketTask` the failed `receive` arrives
  before the close code, `task.closeCode` is 1005 until Foundation has read the frame, and blocking the callback
  (it runs on the session's delegate queue) keeps it from arriving at all. The Swift connector therefore reports the
  code from `URLSessionWebSocketDelegate.urlSession(_:webSocketTask:didCloseWith:reason:)`, falls back to polling
  `closeCode` off the delegate queue (every 20 ms, at most 10 times, 1005 counted as "not yet"), and treats a close reason
  starting with `session lost` as the same news. Measured on the simulator: a rebuild is reported as a lost session by the
  first or second refused attempt (the dev log shows it). The fake connector tests cover the logic.
* **Pre-existing bug fixed in `crates/undra-transport/interop/run.sh`**: its `trap ... kill "${SERVER_PID:-0}"` was
  `kill 0` for the TypeScript-only run, which killed the caller's process group.
* The dev runner's log shows ~50 `port call ... Kv` debug lines at startup with no client attached (the query layer
  hydrating through the client's `Kv`); pre-existing, unchanged, noisy in demos at the default log level.
* `undra dev` still serves one client at a time; a second one is told 1013 and now retries by itself.
* Out of scope and not done: state-preserving reload (B3), devtools (B4), a `doctor` check for `adb`, a Gradle/Xcode
  plugin that bakes the URL in.

## What was verified

* `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`: 2,263 pass
  (13 new transport integration tests in `tests/resume.rs`, resume/query unit tests, 6 `adb` tests, 3 CLI tests).
* TypeScript runtime 964 (+33: `remote.test.ts` with a fake WebSocket and `vi.useFakeTimers()`); the web playground 102.
* Kotlin runtime 538 cases (+38: `WebSocketClientTests` 12, `ReconnectCoreTests` 13, `RemoteReconnectTests` 13); the
  22 existing `RemoteTransportTests` pass unchanged on the new client.
* Swift 455 (+30: `ReconnectCoreTests` 16 over the droppable `FakeTransport`, `RemoteReconnectTests` 14 over a scripted
  `SocketConnector` and a manual scheduler).
* `crates/undra-transport/interop/run.sh ts|kotlin`: the shipped TS and Kotlin transports against the real Rust server
  through a proxy that cuts connections: reconnect, resume (same object, same state, mirror converged), restart ->
  lost session once.
* Live, on the booted AVD `undra` (emulator-5554) and the iPhone 17 Pro simulator, and in a browser: `undra dev
  -C examples/playground --android`; the Android app launched with `--es undra_dev_url ws://10.0.2.2:7450`, tapped the
  counter (100 -> 101), `Signal::new(0)` became `Signal::new(100)` in `counter.rs`, the dev log shows the rebuild in
  0.9 s and `client disconnected ... 1 object(s) kept`, `asked to resume session ... told to load a new core`,
  `client connected`; logcat shows `Connected -> Reconnecting(1: 1001) -> Reconnecting(2: refused) -> Closed(SESSION_LOST)
  -> Connecting -> Connected`; the screen showed 100 and then 101 after a tap. A proxy cut the connection with the counter
  at 103: amber bar, then green, `client reconnected ... (away 6.0 s, 1 object(s) kept)`, a tap gave 104 (state kept).
  iOS and web did the same edit cycle (counter 100 -> 200 -> ... -> 700 on iOS, 700 -> 800 on web). Screenshots in
  `examples/playground/.proof/dev-loop/`.
* The generated Android template builds (`undra init` + Gradle `assembleDebug`); the merged release manifest has no
  `INTERNET` or cleartext entry.

## Files

New: `.10x/adrs/ADR-034-*`, `docs/DEV_LOOP.md`, `crates/undra-transport/src/resume.rs`, `crates/undra-transport/tests/resume.rs`,
`crates/undra-cli/src/adb.rs`, `runtimes/kotlin/.../WebSocketClient.kt`, `ConnectionState.kt`, Swift `Core/Connection.swift`, the
Kotlin/TS/Swift reconnect tests, playground and template `DevServer.kt`, `DevStatus.kt`, `DevStatusBar.swift`, `dev-banner.ts`,
`src/debug/AndroidManifest.xml` (playground and template). Changed: the three runtimes' remote transports and cores,
`undra-transport` (server, session, conn, tracker, bridge, ws, README), the dev runner template, `undra dev` and its help,
the playground apps, the templates, the READMEs, the site's generated files, SPEC 3.2/11/17.
