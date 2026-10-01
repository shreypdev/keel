# SDE — Android platform adapters (wt/android-adapters, 2026-10-01)

Closes finding 1 of `.10x/specs/2026-10-01-competitive-limitations.md`: Android shipped without platform adapters
(the Kotlin runtime installed only Clock, Rng, Log and Timer; the playground faked Kv and Http), so persistence, the
offline queue, secure storage, files, connectivity and lifecycle did not work out of the box. They do now: real adapters
for the other six ports, installed by one call, tested on the emulator, and the playground fakes nothing. No wire, C ABI,
runtime-model, threading-model or generated-shape change, so no ADR (R11): the adapters are platform code behind the port
contract of SPEC section 8 and ADR-025, and the code is in `android-adapters` (CLAUDE.md: Android-specific code lives there;
`:runtime` is untouched except KDoc).

## What was built (`runtimes/kotlin/undra-runtime/android-adapters`, package `dev.undra.android`)

| Port | Class | Android API | Parity notes |
|---|---|---|---|
| `Http` | `AndroidHttpAdapter` | `HttpURLConnection`, `Dispatchers.IO` | typed `HttpError` (InvalidUrl, Timeout, Cancelled, Network) like the JVM, Swift and web adapters; `timeoutMs` bounds the whole exchange; cancelling the caller `disconnect()`s the connection from the cancelling thread; redirects followed by the adapter; response capped (64 MiB); request bodies over 256 KiB streamed with a fixed length |
| `Kv` | `AndroidKvAdapter` | files in `filesDir/undra/kv` (wraps `FileKv`) | the file format of the Swift adapter (`u32 len, key, value`), atomic writes |
| `SecureStore` | `AndroidSecureStoreAdapter` | `AndroidKeyStore` AES-256-GCM, files in `noBackupFilesDir/undra/secure` | the web adapter's sealed layout and AAD (`undra.secure:<key>`); failures throw (the port answers unavailable) instead of reading as missing |
| `Fs` | `AndroidFsAdapter` | `filesDir/undra/fs` (wraps `FsAdapter`) | `..` denied outright, symlinks out denied, `delete` recursive and never the root (Swift and web behave so) |
| `Connectivity` | `AndroidConnectivityAdapter` | `ConnectivityManager.registerDefaultNetworkCallback` | initial state at once, then changes, de-duplicated, from a handler thread; online = `NET_CAPABILITY_INTERNET` (like `NWPath.satisfied`) |
| `Lifecycle` | `AndroidLifecycleAdapter` | `Application.ActivityLifecycleCallbacks` | Active / Inactive / Background from started and resumed counts, downgrades debounced 700 ms |
| `Log` | `AndroidLogAdapter` | `android.util.Log` | keeps trace and debug, which Android's `java.util.logging` drops |
| `Clock`, `Rng`, `Timer` | the runtime's | — | registered by `install` too, so one call covers all ten |

`AndroidPlatformDefaults.install(core, context)` (returns an `AndroidPlatform` handle) registers all ten and starts the two event
sources; the playground and the `undra init` Android template call it and **no longer fake anything**. Pure logic is in
`HttpRules` (URL, header, error and redirect rules), `SecureSeal`, `NetworkClassifier` and `ProcessStateMachine`, each testable
without a device.

### Decisions

* **Zero third-party dependencies.** `androidx.security:security-crypto` is deprecated and drags in Tink; Keystore-direct is
  about 80 lines. `ProcessLifecycleOwner` would add `lifecycle-process` to every app for counting activities.
  `HttpURLConnection` on Android is OkHttp already. Test-only dependencies (JUnit 4, AndroidX Test) are not shipped.
* **Install after load.** `install` runs after `UndraCore.load`. The query client's hydration waits up to 5 s for the `Kv`
  adapter to appear (`list_when_available`; "a native host registers them right after `undra_init`"), so there is no race to
  fix and `LoadOptions` did not need to change. Kv is registered first.
* **Online means a route, not a validated one** (`NET_CAPABILITY_INTERNET`, not `VALIDATED`). The query layer replays its queue
  only on an offline-to-online *transition*; a state that never turns online (a Wi-Fi without Google's connectivity check, a
  corporate network) would never drain the queue. `requireValidatedNetwork = true` opts in to the strict reading.
* **Redirects are the adapter's own.** `HttpURLConnection` does not follow http to https and behaves differently across Android
  versions; the rules of `java.net.http` (`Redirect.NORMAL`) are implemented and unit-tested once, with credentials dropped on a
  cross-origin hop.
* **Fs delete is recursive on Android**, like Swift (`removeItem`) and web (`removeEntry(recursive)`). The JVM `FsAdapter` is
  not (it answers `Io("directory not empty")`): an inconsistency in the runtime, see findings.
* **No `terminate` lifecycle event.** `AppState` is Active | Inactive | Background and Android ends a process without telling
  it; the offline queue and the cache are persisted as they change. What the offline queue "drains on" is `Connectivity`
  turning online (and the replay at start-up); `Lifecycle.Active` refetches stale queries.
* **Kv and Fs under `filesDir`, SecureStore under `noBackupFilesDir`**: the first two belong in Auto Backup, a restored
  ciphertext could never be opened on another device.
* Not done, by design: consumer ProGuard rules for `UndraNative` (separate item, still in the README), `WorkManager` drain.

## The playground (`examples/playground/android`)

* `UndraApp` is `UndraCore.load` + `AndroidPlatformDefaults.install` + `configureRemote`. `InMemoryKv.kt` and the in-process
  `Http` port are deleted. **Merge note:** `wt/dev-loop` also edits `UndraApp.kt` (dev-URL wiring); the two edits are in the same
  `onCreate` and need a hand merge.
* `remote/DemoServer.kt` is now a small HTTP server on the loopback interface (a real socket and a real `HttpURLConnection`),
  reachable only while the device has a network and the Offline switch is off, like a host on the internet: otherwise it resets the
  connection, which the core sees as `HttpError.Network`. The Offline switch still *simulates* an outage (it also sends a
  `Connectivity` event through `ConnectivityEvents`); airplane mode needs no switch because the real adapter reports it.
* `res/xml/network_security_config.xml` allows cleartext to 127.0.0.1 and localhost only. Manifest: INTERNET and
  ACCESS_NETWORK_STATE explicitly (the library's manifest declares them too, so they merge in for any app).
* `examples/playground/android/smoke.sh` (new; the earlier Android "smoke" was an uncommitted ad-hoc script, see
  `.proof/android-smoke.log`) tours the four tabs and drives the offline story, see Evidence.
* Template (`crates/undra-cli/templates/android`) and `undra adopt`'s instructions depend on `android-adapters`, call `install`
  and declare both permissions.

## Verification

| Check | Result |
|---|---|
| `./gradlew :android-adapters:test` (JVM unit tests) | 128 tests per variant (debug and release unit-test tasks both run), 127 pass, 1 skipped (PATCH cannot be sent by the desktop JVM's `HttpURLConnection`; it runs on the device) |
| `./gradlew :android-adapters:connectedAndroidTest` on the `undra` AVD (API 35, arm64) | 110 tests: 109 pass, 0 fail, 1 skipped (the network-toggle test, off unless asked): Http 41, Kv 13, Fs 12, SecureStore 10 (shared with the JVM run) + 8 on the real Keystore and process death, Connectivity 6, Lifecycle 7, `install` 5, Http on device 5, locations 3 |
| the same plus `-Pandroid.testInstrumentationRunnerArguments.undra.networkToggle=true` (Wi-Fi and data switched off and on) on a private AVD | 6 of 6 `ConnectivityOnDeviceTest`, none skipped |
| playground `./gradlew :app:assembleDebug`, `smoke.sh` on an emulator | `BUILD SUCCESSFUL`; `SMOKE PASSED` (four tabs alive, no crash marker; log below) |
| `runtimes/kotlin/undra-runtime/scripts/test-local.sh` | 500 cases, 0 failed, 2 skipped (the JNI smoke needs a native library; unchanged) |
| `bash contract-tests/run-all.sh` | 54/54 (18 scenarios x ts, kotlin, swift), unchanged: the Kotlin runner keeps its JVM Kv/Http |
| `cargo test -p undra-cli`; `UNDRA_TEST_ANDROID=1 UNDRA_TEST_ANDROID_APP=1 cargo test -p undra-cli --test platforms android_builds` (template app builds with `android-adapters`, 11.9 MB debug APK) | pass |
| `cargo test --workspace`, `cargo fmt --check` | RESULT_CARGO |

### Evidence of the offline story (`examples/playground/.proof/android-adapters-smoke.log`, screenshots `android-adapters-*.png`)

A queued mutation survives a killed process and is replayed after real airplane mode ends:

1. fetch: `inbox` fetched, one file in `files/undra/kv` (the persisted query);
2. Offline switch on, "Queued while offline" added: shown as `queued`, two files in `files/undra/kv` (cache and queue); the
   server logs `POST /lists/inbox/todos dropped ... (Idempotency-Key K)`;
3. `am force-stop`; real airplane mode on (`cmd connectivity airplane-mode enable`); relaunch: the list is shown from `Kv` with
   the time of the fetch *before the kill*, no POST is sent while the Connectivity adapter says offline, Refresh fails with
   `network: network error: Connection reset` (the real Http adapter);
4. airplane mode off: the adapter reports online, the core replays the queue and the server logs
   `POST /lists/inbox/todos -> 201 (Idempotency-Key K)`, the same K as before the kill; the list shows four rows.

## Findings for the integrator

* **`FsAdapter.delete` (JVM) is non-recursive; Swift and web are recursive.** Android follows Swift and web. Align the JVM adapter
  (and say so in SPEC section 8, which does not specify it).
* **`FileKv.list` reads every entry file whole** to get its key (Swift reads only the key). The query client's hydration lists
  `undra.query.cache.` at every start, so a large persisted cache costs I/O proportional to its size at launch on a background
  thread. A header-only read is a few lines; not done here (runtime untouched).
* `AppState` numbering: SPEC section 8's trait comment lists `Active | Background | Inactive`, the type `Active, Inactive, Background`;
  the runtime follows the type (unchanged, noted in `StandardRecords.kt`).
* The core's default log level hides its DEBUG records (`queued mutation ... for replay`), so logcat shows only what the demo
  server and the adapters log. Evidence in the smoke therefore comes from the server's request log.
* `ActivityScenario.moveToState(STARTED/CREATED)` launches another activity of the test app on top, so from the app's point of
  view it never leaves the foreground; the device tests take the app to the background with Home. `Inactive` is covered by the
  state machine's unit tests only.
* `android.useAndroidX=true` is now in `runtimes/kotlin/undra-runtime/gradle.properties` (needed by the AndroidX Test
  dependencies of the instrumented tests; the library has no AndroidX dependency).

## Emulators

`connectedAndroidTest` ran on the shared `undra` AVD (`emulator-5554`, `ANDROID_SERIAL` pinned so other attached devices are not
used). The two tests that change the whole device (switching the network off, airplane mode in `smoke.sh`) ran on a private AVD,
`undra-adapters` on port 5560, created and removed by this piece, so no other agent's session was disturbed.
