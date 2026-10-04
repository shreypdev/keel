# The dev loop (Android remote mode, reconnect, session resume) - adversarial review

**Date:** 2026-10-01 · **Reviewer:** Claude Opus 5.5 (adversarial pass: the Kotlin WebSocket client against a
hostile server, session resume races on the dev server, reconnect semantics in three runtimes, the Android
wiring, the docs) · **Piece:** `wt/dev-loop` at `3edb375` (10 commits), `main` (`08e4e39`) merged in, review
fixes committed on top · **Implementer's record:** `.10x/decisions/sde/dev-loop.md` · **Decision:**
ADR-051 (drafted as ADR-034, which `main` already uses for `WeakCtx`; renumbered here, every reference updated,
the `WeakCtx` references on `main` untouched).

## Verdict

The design holds and the server half is solid: the session token is URL material only, `resume_grace` is
off by default in `ServerConfig` and on (ten minutes) only in the dev runner and the `serve` example, every
retained handle has exactly one owner at a time (`retain`, `take`, `supersede`, the reaper and `stop` all move
the `Retained` out under one lock), and three new race tests (an expiry racing a resume, 40 rounds landing
both ways; a resume and a new client racing for the slot; a shutdown racing a resume) found no lost or
leaked object. The client half needed work. The Kotlin WebSocket client handled the happy path and the RFC
basics (accept key, masking, control-frame limits, fragmentation with a cap checked before allocation,
ping/pong, `wss` with endpoint identification) but a hostile or broken server could (a) send it into a
reconnect loop that never ends, because RFC 6455 violations were plain I/O errors, (b) kill the app, because a
listener exception or an `OutOfMemoryError` escaped the reader thread, and (c) hold `UndraCore.load` (on
Android's main thread) for minutes with a trickled upgrade response. Swift could resurrect a transport that
`shutdown()` had closed if the shutdown landed at the start of a reconnect attempt, leaving a socket nobody
closes on the dev server's only client slot. And, pre-existing but now advertised by the banner and the docs,
the **web template served `?undra=<url>` in production builds**: any link could point a deployed page's core at
a stranger's server. All of these are fixed here with tests that fail on the old code where a deterministic
test was possible. **Threat model for session resume:** `undra dev` is an unauthenticated development server
(loopback by default; `--addr 0.0.0.0` exposes it to the LAN; browser pages are limited to local-network
origins), so anyone who can reach the port can already attach while no client is, construct objects and call
any handle it can name. The token does not move that boundary and is not a credential: it is 128 CSPRNG bits
(Kotlin `SecureRandom`, Swift `UUID()`, TypeScript `crypto.getRandomValues`, with a `Math.random` fallback only
where WebCrypto is missing) that let a returning client be recognised. A peer that learns it can adopt the
dropped client's objects (reachable by handle anyway) and evict its live socket (a denial of service on one
developer's session). The dev server logs only its first eight characters and never the URL. The ADR now says
this in a "Threat model" section. Ready to merge once the integrator has read the ADR's new section.

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| H1 | High (pre-existing, amplified) | `crates/undra-cli/templates/web/src/undra.ts:16`, `examples/playground/web/src/undra.ts:39` | **A production web page took its core's address from its own URL.** `?undra=` (and `VITE_UNDRA_DEV_URL`) were read in every build. A link `https://app.example/?undra=wss://evil/` makes the victim's page load the attacker's "core" (the schema hash is in the bundle), which then receives the page's port calls (`Kv`, `Http` from the page's origin, `SecureStore` plaintext) and drives its UI. Android and iOS gate the dev URL on debug builds; the web did not, and this piece adds `?undra=` to the `undra dev` banner, help and `DEV_LOOP.md`. | **Fixed**: read only when `import.meta.env.DEV` (`vite dev`); a production build ignores both. Template, playground, `init` README text and `DEV_LOOP.md` say so. Web playground 102/102. |
| M1 | Medium | `runtimes/swift/.../Adapters/WebSocketTransport.swift` `openConnection` | **`shutdown()` racing a reconnect attempt brought the transport back.** `openConnection` (now shared by `start` and every retry) reset `isShutDown = false`. A shutdown between the attempt's liveness check and its connect left a live, handshaken socket that nothing closes; it holds `undra dev`'s single client slot, so the next core (another token) is told 1013 for ever. | **Fixed**: only `start` clears the flag; `openConnection` opens nothing for a shut-down transport. `testShutdownThatRacesAStartingAttemptDoesNotBringTheTransportBack` (shutdown from inside `holdsObjects()`) fails on the old code (a second socket, `reconnected`, frames still sent) and passes now. |
| M2 | Medium | `runtimes/kotlin/.../WebSocketClient.kt` `readLoop` | **A callback that threw, or an `OutOfMemoryError` on a large message, killed the reader thread silently.** On Android an uncaught exception on any thread ends the process; on the JVM the connection was left half-dead (no reads, no pings, no `onError`, calls waiting for their timeout). R6. | **Fixed**: listener callbacks run under `tell {}`; a throw ends the connection through `onError` (itself guarded). The payload allocation turns an OOM into a 1009 protocol error. Hostile-server case "an owner whose callback throws ...". |
| M3 | Medium | `WebSocketClient.kt`, `RemoteTransport.kt` `Connection.onError` | **RFC 6455 violations were retried for ever.** A masked server frame, a reserved bit, an unknown opcode, a malformed control or close frame, or a message over the 64 MiB cap surfaced as a plain `IOException`; `RemoteTransport` treated it as a dropped connection, reconnected, resumed, re-observed, and met the same server again (an over-cap change-set is re-sent on every resume). No close frame was sent (RFC 7.1.7 SHOULD). | **Fixed**: `WebSocketProtocolException` (code 1002, or 1009 for size); the client sends that close frame best effort before dropping; `RemoteTransport` maps it to its final `ProtocolError` (`closed(failed)`), like a text frame. New `RemoteReconnectTests` case "a server that breaks RFC 6455 ... final, not retried" (no backoff wait, one connection). |
| M4 | Medium | `WebSocketClient.kt` `connect`, `readHead`, `checkResponse` | **The opening handshake had no overall deadline and skipped RFC 6455 4.1 MUSTs.** Each byte of the response was bounded by the socket timeout, the whole by nothing: a server trickling it held `connect` (and `UndraCore.load`, from Android's main thread in the playground) for up to 16 KiB x the timeout. `Upgrade: websocket` and `Connection: Upgrade` were not checked, nor an extension or subprotocol the client never offered. | **Fixed**: a handshake deadline (about twice the timeout overall: connect, then handshake); the four checks. Hostile-server cases for each, and "a server that trickles its upgrade response is given up on at the deadline" (500 ms timeout, gives up in under 2 s). |
| L1 | Low | `WebSocketClient.kt` close path | The echo of a server's close raced the owner: `onClose` ran before the writer flushed the echo, and `RemoteTransport.lost` aborts the socket from `onClose`, so the server usually saw an abnormal closure. | **Fixed**: the echo is flushed (bounded) before `onClose`. Case "the close is echoed before the owner hears of it ...". |
| L2 | Low | `WebSocketClient.kt` close path | A one-byte close payload and close codes no endpoint may send (1005, 1006, 1015, below 1000, ...) were accepted. | **Fixed**: protocol errors (1002); 1012 to 1014 (IANA) stay valid since the server sends 1013. |
| L3 | Low | `WebSocketClient.kt` `sendCloseFrame` | The close reason was cut at 123 bytes without regard to UTF-8. | **Fixed** (`utf8Prefix`). |
| L4 | Low | `WebSocketClient.kt` `openSocket` | `wss`: a TLS failure on one address followed by a refusal on the next reported the refusal (`localhost` resolving to `::1` and `127.0.0.1`), hiding the certificate error. Android's `SSLSocket` is not documented to verify the host name on every release. | **Fixed**: a reached-and-refused failure wins over a later `ConnectException`; on Android the platform `HostnameVerifier` checks the session after the handshake (the JDK's endpoint identification does it on the JVM). Injectable `SSLSocketFactory` (internal) for the new test: a certificate for `localhost` is accepted at `wss://localhost`, refused at `wss://127.0.0.1` (name) and with the default trust store (trust). |
| L5 | Low | `WebSocketClient.kt` `abort` | `abort()` closed the socket on the caller's thread; for TLS that writes `close_notify`, which Android refuses on its main thread (`RemoteTransport.close()` during an attempt). | **Fixed**: the socket is released on a thread of the client's own; a `RuntimeException` from `close` is caught. |
| L6 | Low | `WebSocketClient.kt` `connect` | "the URL ... has no host" printed the URL with the session token. | **Fixed**. |
| L7 | Low | `runtimes/ts/@undra/runtime/src/core.ts` `#reconnected` | A replay that threw (the socket broke under it) still announced `connected`; Kotlin and Swift use a loss epoch. | **Fixed**: `connected` only after a complete replay. New test fails on the old code. |
| L8 | Low | `remote.ts` `reconnectDelayMs`, Swift `UndraReconnectPolicy.delay` | `initialDelay` 0 gave a hot reconnect loop (Kotlin refuses it); in TypeScript `2 ** (attempt - 1)` overflowed to `Infinity`, and with a zero initial delay to `NaN` (a 0 ms timer), after about 1,000 attempts. | **Fixed**: at least 1 ms, at most 30 doublings, as Kotlin. Tests in both. |
| L9 | Low | `WebSocketTransport.swift` `URLSessionSocketConnector` | Each transport's `URLSession` holds its delegate until invalidated and was never invalidated: one session and connector leaked per loaded core, i.e. per rebuild in the dev loop. | **Fixed**: `SocketConnector.finish()` (default no-op) invalidates the session (`finishTasksAndInvalidate`) on shutdown and on a final failure; a later connect makes a fresh one. Checked live against the real Rust server with a throwaway XCTest (not committed): connect, 1001 on restart, `reconnecting 1, 2`, 4001, `closed`, two shutdowns, no crash. |
| L10 | Low | `crates/undra-transport/interop/run.sh` | `stop_server` left `SERVER_PID` set, so the EXIT trap could signal a recycled PID. | **Fixed**. The `kill 0` fix is right, and no other script kills a process group (grep for `kill 0`, `kill --`, `kill(-`, `killpg`, `setsid`: none; `packaging/test-install.sh` guards its PID). |
| L11 | Low | `.10x/adrs/` | ADR-034 collided with `WeakCtx` on `main`. | **Fixed**: ADR-051, 59 references updated (SPEC, `DEV_LOOP.md`, the SDE record and index, code comments in Rust, TS, Kotlin and Swift, tests, interop). |
| L12 | Low | `docs/SPEC.md` 11.0, `docs/DEV_LOOP.md`, ADR-051 | SPEC 11.0 listed the final outcomes without protocol errors (final in TS and Kotlin, logged and dropped in Swift); no threat model anywhere; the Kotlin cap and `wss` rules undocumented. | **Fixed**: SPEC 11.0, `DEV_LOOP.md` (Android notes, "the token is not a password"), ADR-051 "Threat model of the session token". |
| I1 | Info | all clients | After a rebuild a client announces `connected` for an instant before `closed(sessionLost)`: the server's `Hello` (the schema check) comes before the 4001 close by design. Visible as a green flash in the status bars. | Accepted (the Kotlin test documents it). |
| I2 | Info | the three cores | A `Release` sent between the socket dying and the core hearing of it is lost, as is a constructor reply in flight; the server keeps those objects until the session ends (bounded by the one retained session). | Accepted. |
| I3 | Info | `examples/playground/android/.../UndraApp.kt` | The playground loads the remote core on the main thread (documented; `remoteTimeout = 5.seconds`): an unreachable LAN address can still approach an ANR. M4 bounds the handshake; the connect is the timeout. | Accepted for a dev path; loading off the main thread is a template improvement for later. |
| I4 | Info | Android debug builds | The exported launcher activity accepts `--es undra_dev_url` from any app on the device; the named server then serves the debug app's core and receives its port calls. | Stated in the threat model; release builds ignore it (verified). |
| I5 | Info | `crates/undra-cli/src/adb.rs` | `adb reverse` mappings stay after `undra dev` exits. | Accepted: the command is idempotent (rebinds) and scoped (`-s <serial>`, `ANDROID_SERIAL`). |

## What was checked and how

* **Hostile server (Kotlin), `WebSocketHostileServerTests`, 16 cases, all pass**: wrong or missing
  `Sec-WebSocket-Accept`; missing `Upgrade`/`Connection`, an unoffered extension; a trickled handshake; every
  client frame masked with a fresh mask (8 frames, 8 masks; the close masked too); a masked server frame (1002
  back, typed); a 2^62-byte frame and a negative length (refused before allocation, 1009/1002 back); fragments
  over the cap (1009); a fragmented message with a ping between fragments (reassembled, ponged); a frame cut off
  mid-payload and mid-length (a dropped connection, not a protocol error); a close without a status (reported
  1005, echoed 1000); the close echo against an owner that aborts at once; a one-byte close and forbidden close
  codes; an oversized and a fragmented control frame; reserved bits and unknown opcodes; a throwing callback;
  `wss` trusted+named / misnamed / untrusted. Plus `RemoteReconnectTests` "a server that breaks RFC 6455 is
  final". On the old client, the upgrade-header, trickle, close-validation, throwing-callback, typed-1002/1009
  and `wss`-untrusted cases fail.
* **Session resume races (Rust, `tests/resume.rs`, 17 tests, 3 new)**: expiry vs resume (25 ms grace, 40
  rounds, about 17 resumed and 23 told 4001 per run, five runs: the client always finds its object alive or is
  told 4001, and every round ends with zero live handles); resume vs a new client (whoever wins owns the
  objects, the other gets 1013); shutdown vs resume (8 rounds, zero live handles after shutdown). Eviction aborts
  the stale socket through an `Arc<Conn>` (safe Rust, `#![forbid(unsafe_code)]`); its teardown retains before
  it vacates, so the evicting connection's `claim` finds the objects. Shutdown joins every connection before
  `Resume::stop` releases what is kept, so no `retain` can follow it.
* **Main-thread safety (Kotlin)**: `load` connects and handshakes on `undra-connect`; `observe`, `release`,
  `call`, `cancel` only enqueue; `close` enqueues, and `abort` now releases the socket off the caller's thread.
* **Swift close codes**: the delegate's `didCloseWith` records the code; `connectionLost` never waits on the
  delegate queue, re-checks on its own queue at most ten times 20 ms apart, and the locks it takes are leaves
  (state, then `reported`; the delegate takes `connections`, then `reported`): no deadlock, no spin.
* **State API idioms**: TS `core.connection` is a `Signal` (`useSignal` renders it), Kotlin a `StateFlow`
  plus a callback that hears every change, Swift a `@MainActor @Observable` object, an `AsyncStream` and a
  callback. Each fine for its platform.
* **Android wiring**: `./gradlew --offline :app:processReleaseManifest :app:processDebugManifest` on the
  playground (merged outputs regenerated): release has neither `INTERNET` nor `usesCleartextTraffic`, debug has
  both. The template keeps `INTERNET` in `main` as before this piece (its `Http` port) and cleartext in debug
  only. `DevServer.requested` returns `null` unless `BuildConfig.DEBUG`; the release `UNDRA_DEV_URL` field is
  empty. `10.0.2.2` appears only in docs, comments, CLI output and a debug-only problem screen. The playground
  compiles (debug and release Kotlin) against the changed runtime.
* **Interop**: `crates/undra-transport/interop/run.sh ts` and `kotlin` pass (reconnect, resume with the same
  object and state, restart reported as a lost session once).

## Suites after the fixes

`cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test
--workspace` 2,267 pass, 0 fail, 10 ignored (was 2,264); TypeScript runtime 966 (was 964); web playground
102; Kotlin 555 cases, 0 failed, 2 skipped (was 538; three consecutive runs); Swift 457 (was 455); the
playground's Android Kotlin compiles (debug and release); interop TS and Kotlin OK;
`contract-tests/run-all.sh` 54/54; `node site/scripts/build-all.mjs` up to date and `check-links` 20 pages
OK.
