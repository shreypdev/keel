# The app's network stack inside the ports (ADR-060, `okhttp-adapters`) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** adversarial (`docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/okhttp-adapters` at
`d6b770b` (`main` `fd7abb4` merged; `main` has not moved since) · **Read:** `CLAUDE.md` (R4, R5, R6, R12), ADR-060, ADR-047,
ADR-048, ADR-025, `docs/SPEC.md` 8, 11, 17.2, `.10x/decisions/sde/okhttp-adapters.md`, both modules' READMEs, the whole diff
(`okhttp-adapters`, the moved suites in `adapter-contracts` and `test-support`, `SseStreamReader` / `ReadAheadSource`, the Swift
`URLSessionWebSocketAdapter(session:)` and its tests, the TypeScript test, the cookbook recipe and the site), and for comparison
`AndroidHttpAdapter` / `HttpRules`, `ClientWebSocketAdapter`, `WebSocketClient`, `UrlConnectionSseAdapter`, `JdkHttpSseAdapter`,
`PulledStream`, `WebSocketPortAdapter`, `PortRegistry`, `AndroidPlatformDefaults`, `undra-query`'s start-up hydration and
OkHttp 4.12's `RealWebSocket`, `RealCall` and `Dispatcher`. · **Fixes:** `d5153ab`, `e9ca7a1`, `25d7fa9`, `44d82de`, `bb126b4`,
`cd0d699`, `e09ea03`, `40d69b2`.

## Verdict

**Sound after the fixes; merge.** The adapters keep the contract where it matters most: a cancelled `Http` call is cancelled on
OkHttp (`Call.cancel()`), not abandoned, and leaves nothing running in the dispatcher or in use in the pool; the WebSocket reader is
bounded (OkHttp's reader thread is held in the listener while the binding's buffer is full: one frame beyond the window, TCP pushes
back, shown by the flood case and by a mutant); the Sse reader is the default adapters' gate; every OkHttp exception maps to the same
typed reason as on `HttpURLConnection` (timeouts, DNS, TLS and pins, cancellation, status, size, early close); no callback runs the
core's code on OkHttp's threads (the port replies run on the registry's `Dispatchers.Default` scope, the pumps on the bindings').

One High, fixed: `installWithOkHttp` let the platform's `HttpURLConnection` serve a request made while it ran (H1), which defeats the
module's purpose for an app whose security review says every request is pinned. Seven Mediums, all fixed but M7 (CI), which is a
shared file. Both findings the piece reported hold and are now pinned honestly: OkHttp repairs malformed UTF-8 (a declared contract
parameter, not a skip), and the missing event listener is OkHttp's (`EventListener.NONE` in `RealWebSocket.connect`), not the
adapter's; so is a second limit the piece had not seen, network interceptors skipped for a WebSocket's upgrade (M4).

## Findings

| # | Sev | Where (at `d6b770b`) | Finding | Status |
|---|---|---|---|---|
| H1 | High | `OkHttpPlatformDefaults.kt:81-87`, `AndroidPlatformDefaults.kt:160-170` | `installWithOkHttp` called `install`, which registers `Kv` first and then the platform's `Http`, and re-registered the three network ports only after `install` returned (after `connectivity.attach` and `lifecycle.attach`, binder calls). A core hydrates and replays its offline queue as soon as `Kv` answers (`undra-query` `hydrate`, online by default), so a queued mutation could go out through `AndroidHttpAdapter`: no token, no trace, no pin, and a `401` that rejects the mutation. The KDoc and ADR said "before any request can be made". Repro: a core that calls `Http` the moment it is registered: two registrations, the first request without the app's interceptor header. | **Fixed** (`d5153ab`): `AndroidPlatformDefaults.installWithNetworkPorts(core, context, http: PortImpl, webSocket, sse, ..)` behind `@UndraEmbeddingApi`; the network ports are registered before `Kv` (for `install` too); `installWithOkHttp` is built on it. Device test `no_request_goes_through_the_platforms_adapters_not_even_one_made_while_installing` failed before, passes. |
| M1 | Medium | `OkHttpSseAdapter.kt:115-119` | `Last-Event-ID` went through OkHttp's checked header, which refuses anything but ASCII: an event stream whose ids are not ASCII could never resume (`Refused` without a status). Android's `HttpURLConnection` (the default) and the fetch standard send UTF-8 bytes; ADR-047's review fixed the same in `fetchSse`. | **Fixed** (`25d7fa9`): `Headers.Builder.addUnsafeNonAscii`. Contract case on the JVM and on the device (`é`, `日本-7`), which failed on OkHttp; `java.net.http` (desktop JVM) writes header values as US-ASCII (`?`) and declares it (`sendsNonAsciiLastEventId = false`). |
| M2 | Medium | `OkHttpRules.kt:84`, `AndroidHttpAdapter.kt:235` | A `GET` or `HEAD` whose body is present but empty (`with_body(vec![])`; `checkBody` lets it through as "no body") was `InvalidUrl("the GET method is not supported")` on OkHttp, and silently a **`POST`** on `HttpURLConnection` (`doOutput = true`), a read turned into a write (pre-existing in `android-adapters`, exposed by the shared case). | **Fixed** (`25d7fa9`): both send the method without a body. Contract case `a_get_or_a_head_with_an_empty_body_is_sent_as_itself_without_one`, failed on both. |
| M3 | Medium | `RealtimeAdapterContract.kt`, `OkHttpRealtimeAdapterTest.kt` | The Kotlin "close racing the terminal" counterpart of the Swift `testCloseRacingTheTerminalAnswersThePendingReceiveExactlyOnce` did not exist; nor did a close while the reader is held, a server that never answers a close, an OkHttp `Authenticator` on the upgrade, or any check that OkHttp's threads and connections are released. | **Added** (`e9ca7a1`, `e09ea03`), see "Contract cases added". All pass on OkHttp and on the defaults. |
| M4 | Medium | `okhttp-adapters/README.md:54`, `OkHttpWebSocketAdapter.kt:31`, ADR-060 6, SPEC 11, `site/docs/ports.html`, cookbook | The docs said the app's "interceptors" apply to a WebSocket's upgrade. OkHttp leaves the client's **network** interceptors out of a WebSocket call (`RealCall`, `forWebSocket`) besides giving it `EventListener.NONE`: tracing written as a network interceptor never sees the core's WebSockets. Measured: an application interceptor's header arrives, a network interceptor's does not, the listener hears nothing. | **Fixed** (`bb126b4`): every place says "application interceptors" and names both of OkHttp's limits; a case pins them (an OkHttp that changes them is noticed). |
| M5 | Medium | `crates/undra-cli/src/upgrade.rs:865` | `undra upgrade` moved `dev.undra:runtime`, `android-adapters` and `android-work` but not `okhttp-adapters`, which is compiled against the runtime's embedding API (`SseStreamReader`, `ReadAheadSource`, "may change between releases"): an upgraded app would run the old module against a newer runtime. | **Fixed** (`44d82de`): it moves with the others; unit test failed before. |
| M6 | Medium | `SseAdapters.kt` `SseStreamReader` | Closing a stream whose reader was waiting for room in the binding's buffer cancelled the OkHttp call but never closed the body, so the call kept its connection allocated (in use, socket closed) until a GC and the pool's next cleanup, which OkHttp logs as a leaked connection. Measured: `connectionCount 1, idle 0` after closing, still after `System.gc()`. | **Fixed** (`cd0d699`): the reader thread (the only one that reads the body) closes it however its loop ends; `close()` closes the body of a stream never read. Case "a closed stream leaves no connection in use", failed (timed out) before; the default adapters' suites pass unchanged. |
| M7 | Medium | `.github/workflows/ci.yml` | No CI job compiles or tests `:okhttp-adapters` (nor `:android-adapters`' JVM tests): the `android` job assembles apps that do not depend on it, the `kotlin` job runs `:runtime` through `test-local.sh`. The module's contracts run only on a developer's machine. | **Not fixed**: `ci.yml` is a shared file (`AGENT_WORKFLOW.md`, parallelism rules). Proposed for the integrator: in the `android` job, `actions/setup-node@v4` (24) and `cd runtimes/kotlin/undra-runtime && UNDRA_REQUIRE_TOOLCHAINS=1 ./gradlew --no-daemon :okhttp-adapters:testDebugUnitTest` (about a minute here; the realtime contract already runs on Linux in the `kotlin` job for the defaults). |
| L1 | Low | `OkHttpHttpAdapter.kt` | An application interceptor that throws anything but an `IOException`: OkHttp's `AsyncCall` rethrows it on its dispatcher thread (an Android app crashes, as it does for the app's own calls) and the core's call ends `Cancelled` (`IOException("canceled due to ...")` on a cancelled call). | **Documented** (README). |
| L2 | Low | `OkHttpWebSocketAdapter.kt` | No inbound size limit, and OkHttp offers `permessage-deflate`, so a small frame can inflate into a large message (memory) from a hostile server. The default adapter caps at 64 MiB (1009). | **Documented** (README, beside the limit the piece already named). |
| L3 | Low | `OkHttpSseAdapter.kt:73-79` | THEORY: a cancellation landing between `continuation.isActive` and `resume` drops the `Response` unclosed (the cancel handler has closed the socket; the call holds the connection until GC, as in M6). `resume(value) { response.close() }` closes it but is `@ExperimentalCoroutinesApi` in 1.6.4. | Not fixed (a window of a few instructions). |
| L4 | Low | `SseAdapters.kt` | `SseStreamReader` is a public class rather than a factory returning `SseStream`. Acceptable: its public members are exactly `SseStream`'s and `ReadAheadSource`'s, the opt-in is `RequiresOptIn(ERROR)` (checked: a plain `kotlinc` consumer gets an error at each use), and `test-local.sh` compiles `:runtime` with `-Xexplicit-api=strict`. `Markers.kt` did not list the two among the embedding API. | `Markers.kt` fixed (`d5153ab`); the shape kept. |
| L5 | Low | `RealtimeAdapterTests.kt` (`java.net.http`) | Pre-existing, desktop JVM only: `JdkHttpSseAdapter` sends a non-ASCII `Last-Event-ID` as `?`. | Declared in the contract (M1); not fixed. |
| L6 | Low | `crates/undra-cli/src/upgrade.rs` | Pre-existing: `dev.undra:undra-compose` is not moved by `undra upgrade` either. | Not fixed (out of this piece). |
| L7 | Low | `crates/undra-cli/templates/android/app/build.gradle.kts:183` | `undra init`'s app lists `android-work` as a commented opt-in dependency; it does not mention `okhttp-adapters` (found through the `android-adapters` README, the ports page and the cookbook). Android modules are not published to Maven at all yet (`release.yml`, `packaging/`), so there is no packaging entry to add. | Not fixed (a template line would need `undra upgrade`'s commented-line rule too). |
| L8 | Low | `:runtime:test` under Gradle | Pre-existing and unrelated: `CoreCallTests` ("a continuation that would run inline is resumed off the core thread") fails on an export of `origin/main` `fd7abb4` exactly as on the branch. `RemoteTransportTests` (schema hash after the handshake) failed once in one full Gradle run on the branch and passed alone three times and in every `test-local.sh` run (CI's path); nothing here touches it. | Not fixed. |

## Contract cases added (they run for every adapter that runs the suite)

* `RealtimeAdapterContract` (`:runtime` on the defaults through `test-local.sh`, `:okhttp-adapters` on OkHttp): the core's close racing
  the peer's close frame, 30 rounds at three timings (the pending receive is answered once, with `[]`, `[hello]` or `Closed(4000)`;
  the stream then ends cleanly; `send` sees the core's close); the core's close reaching a server it stopped reading (its code and
  reason); a server that never answers the core's close (`/ws/deaf`: close returns, the client leaves after its grace); a non-ASCII
  `Last-Event-ID` sent as UTF-8 (also in `RealtimeOnDeviceContract`, on Android's `HttpURLConnection` and OkHttp).
* `HttpAdapterContract` (`HttpURLConnection` and OkHttp, JVM and device): a `GET` / `HEAD` with an empty body.
* OkHttp's own: cancelled and timed-out calls leave no call in the dispatcher and no connection in use; a closed event stream and a
  closed WebSocket (held or idle) leave no reader on the dispatcher and no connection in use; a quiet WebSocket outlives the client's
  read and call timeouts; the `Authenticator` answers an upgrade's `401` (`/ws/auth`); application interceptors see the upgrade,
  network interceptors and the event listener do not; `installWithOkHttp` on a core that calls `Http` the moment it is registered.
* Swift: `URLSessionWebSocketAdapter(session:)`'s per-task delegate does not swallow the upgrade's authentication challenge: the
  app's session delegate answers `/ws/auth`'s Basic challenge (macOS 26). The metrics case already showed the delegate sees the task.

## Mutants run

| Mutant | Killed by |
|---|---|
| no `Call.cancel()` on cancellation (`invokeOnCancellation { }`) | 4 Http cases, the new leak case among them |
| the WebSocket listener does not wait for room | the stalled-reader flood case |
| the core's close sends no close frame (only cancels) | 5 cases, the new close-while-held case among them |
| close does not cancel after its grace | the new `/ws/deaf` case (OkHttp would hold the socket for its own 60 s) |
| close neither waits nor cancels, against a server that answers | survives: OkHttp completes the handshake itself; only `/ws/deaf` tells |
| `installWithOkHttp` as it was | the device case of H1 |
| `Last-Event-ID` through the checked header; the empty-body rule as it was; `SseStreamReader` that never closes the body; `undra upgrade` without the module | the cases of M1, M2, M6, M5 |

## What was run

* `:okhttp-adapters:test` 142 results (71 per variant), `:android-adapters:test` 304 (2 skipped); on `emulator-5554` (AVD `undra`,
  API 35) with the host's realtime server: `:okhttp-adapters:connectedDebugAndroidTest` 65 pass, `:android-adapters` 149 pass, 1
  skipped (the network toggle).
* `test-local.sh` with the fixture library: 891 cases, 0 failed (`RealtimeAdapterTests` 27), testkit 32; `-Xexplicit-api=strict`.
* Swift: 885 XCTest cases in `ci/macos` (the app-session challenge case among them).
* `scripts/ci-local.sh --only ci/rust,ci/ts,ci/kotlin,ci/macos,ci/android,ci/contracts,ci/contracts-swift,two-cores/jvm-and-node,site/build`
  on `40d69b2` (the code of the hand-off; only this file and a README line came after): every job green. Also `cargo fmt
  --check`, `clippy -p undra-cli -D warnings`, `undra bindgen --check --docs` on the cookbook, Fieldbook and the playground,
  `node site/scripts/build-all.mjs` (the site unchanged) and `check-links --words`.

## Not verified

* iOS 15 / 16 and macOS 15 for the per-task delegate (`ci/ios-floor` and the hosted `ci/macos` are the proof: watch them).
* OkHttp 5.x as a build (the module cannot be built with this Kotlin; the piece ran its tests once against 5.3.2 by hand) and
  certificate pinning end to end (no TLS server without another artifact); both as ADR-060 says.
* The Android emulator CI job (on hold, not touched) and Linux for `:okhttp-adapters` (M7).
