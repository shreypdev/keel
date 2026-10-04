# SDE - Android platform adapters (wt/android-adapters, 2026-10-01)

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
| `Clock`, `Rng`, `Timer` | the runtime's | none | registered by `install` too, so one call covers all ten |

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
| `cargo test --workspace`, `cargo fmt --check` | `cargo test --workspace`: 2236 passed, 0 failed, 10 ignored; `cargo fmt --check` clean; `cargo clippy -p undra-cli --all-targets -- -D warnings` clean (only the CLI changed: template and `adopt` text) |

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

## 2026-10-01 (later) - crossing main: the merge, the error channels, two review follow-ups

`wt/android-adapters` was cut from `6ab7b8c`; `main` had gained dev-loop (ADR-051), device-bench, parity (ADR-032 amendment A),
schema-json, diagnostics, `cas_update` and the Rust 1.98.1 pin (85 commits), then the checkpoint-6 state commit. The branch now
contains `main` (`git merge-base --is-ancestor main HEAD` holds), so it lands by fast-forward. No wire, ABI, runtime-model or
generated-shape change: no ADR.

### Conflicts, and what came from which side

| File | Resolution |
|---|---|
| `examples/playground/android/.../UndraApp.kt` | **main's skeleton kept as it was**: `start` / `load` / `retry`, `devUrl`, `connection`, `epoch`, `failure`, `reloadWhenReachable`, `onConnection`, the `ChoreographerFramePacer`, `remoteTimeout`, `onError`, the `coreLoadNanos` clock around `UndraCore.load` (it still stops before anything else runs). **From this branch**: the `adapters = mapOf(Http, Kv)` of fakes is gone from `LoadOptions` and `AndroidPlatformDefaults.install(core, this)` runs right after the load; the `DemoServer` is the loopback server (`server.start()` instead of `DemoServer.BASE_URL`); the KDoc. **New, needed by the merge**: `load()` keeps the returned `AndroidPlatform` and closes the previous one when a rebuilt core replaces a lost one (otherwise the old core's connectivity and lifecycle callbacks stay registered and report to a closed core), `connectivity` is main's getter (`ConnectivityEvents(UndraCore.shared)`: the lazy of this branch would hold the first core for ever), and `DemoServer.start()` is idempotent (the reload path calls `load()` again). |
| `crates/undra-cli/templates/android/UndraApp.kt` | the same: main's start/load/retry/reconnect skeleton, plus `install` and the platform handle; the `ChoreographerFramePacer` import both sides had added is one line. |
| `crates/undra-cli/templates/android/app/build.gradle.kts` | both sides added the `android-adapters` dependency; this branch's comment (all six adapters plus the pacer, the two permissions) kept. |
| `examples/playground/android/README.md` | the "How the app is wired" bullets merged (main's `MainActivity`/`DevServer` start, this branch's `install` and loopback server); the smoke-test section is this branch's. |
| `docs/ONBOARDING.md` | this branch's three Android rows, main's Swift count (433); the Kotlin runtime row is 588 now (it said 527). |
| `.10x/decisions/sde/_index.md` | every line kept: main's five (dev-loop, schema-json, device-bench, diagnostics, parity) and this branch's. |
| `site/search-index.json`, `site/llms-full.txt` | not conflicted; regenerated with `node site/scripts/build-all.mjs` after the site edits below. |
| auto-merged, `runtimes/kotlin/.../LoadOptions.kt` | clean: this branch's KDoc sentence on `defaultAdapters` and main's `reconnect` / `onConnectionChange` / `onError` properties do not touch each other. |

Changes the merge needed that no conflict marker showed:

* **Cleartext and permissions.** The playground's debug manifest (dev-loop) carried `INTERNET` and `usesCleartextTraffic`; the main
  manifest now declares `INTERNET` and `ACCESS_NETWORK_STATE` (this branch). The debug `INTERNET` is gone, the comments in
  `app/build.gradle.kts`, `src/debug/AndroidManifest.xml`, `docs/DEV_LOOP.md` and `undra adopt`'s step 5 no longer say that only
  debug builds have it. On API 24+ the playground's network security config (cleartext to the loopback demo server only) takes
  precedence over `usesCleartextTraffic` (review I10); the dev connection is a raw `java.net.Socket`, which the cleartext policy
  does not police, and it was exercised below: the flag is inert for the playground and the template keeps it meaningful.
* **Site docs that described the old Android story**: `docs/ports.html` (default-adapter table, the `UndraApp.kt` snippet with
  `InMemoryKv`, the "a port registered later can be missed" note, which the query layer's five-second wait for `Kv` makes untrue
  for Android's `install`), `api-kotlin.html` and `queries.html` (OkHttp, `EncryptedFile`, `ProcessLifecycleOwner`). The
  android-adapters README no longer says `Mode.REMOTE` fails on Android (dev-loop made it work) and gains a section on remote mode
  and late installs.

### Error channels (parity's `UndraCallError` against the adapters): what I decided

The adapters raise errors on two channels, and `UndraCallError` (the failure of a *call into the core*) is on neither, except
through `onError`:

1. **Typed port errors stay typed.** `HttpError` (`Network`, `Timeout`, `Cancelled`, `InvalidUrl`) and `FsError` (`NotFound`,
   `Denied`, `Io`) leave the adapter as `UndraPortException` (a typed reply, status ERROR); the core turns them into the app's own
   typed error (`RemoteError.Http` in the playground). A device that is offline is `HttpError.Network`, as on Swift, the web, the
   JVM and in the contract scenarios; a refused or out-of-root path is `FsError.Denied`; a missing `INTERNET` permission is
   `HttpError.Network` with the permission hint. **`UndraCallError.Unavailable` is not used for any of them**: it means the core
   cannot be reached (closed, or the `undra dev` connection lost), which `docs/ERRORS.md` now says in one sentence.
2. **Untyped failures** (`SecureStoreException` from the Keystore or a tampered file, an `IOException` from `Kv`, a rejected timer)
   are the port registry's: the core is answered `PortError::Unavailable`, the failure is logged and handed to `onError`, where
   parity's `asCallError` classifies anything that is not Undra's as `Malformed` with the exception's text. I kept it. The five
   classes are closed (ADR-032), a Keystore failure is a platform failure, and `Malformed`'s message ("an unexpected failure:
   dev.undra.android.SecureStoreException: the Android Keystore key 'x' is unavailable: ...") names it. What would be better, a
   port-failure case, is an ADR-032 amendment and is not this piece's. No adapter needed to change.
3. **Events to a core that cannot be reached.** `core.event` is the raw API and throws `UndraTransportException` (`CONNECTION_LOST`
   while a remote core reconnects); both event adapters catch, log at warning and drop it. Nothing escapes into the system
   callback thread (R6 holds). See the open items for what is lost.

ADR-049's `StorageError` is not adopted (that is `persistence-v2`).

### Review follow-ups

* **F1 done.** `FsAdapter.delete` (JVM, and so Android) is recursive: a populated directory goes, a symbolic link is removed and
  never followed, the root is `Denied`, `..` stays `Denied`. It is walked with `Files.walkFileTree` (iterative, does not follow
  links), so a tree of any depth cannot overflow the stack and nothing outside the root is reachable from the walk. The Android
  wrapper's own walk is deleted (it only does `confined`: NUL and `..`). SPEC section 8 states the Fs semantics every adapter
  shares (root deletion is `Denied` on Swift, Android and the JVM, `Io` on the web, which has no handle for it). Tests: two new
  cases in `FileAdapterTests` (tree, empty directory, the seven spellings of the root, links inside and out, a directory holding a
  link); the review's three Android Fs tests still pass on the JVM and the device, now through the base class.
* **F2 done.** `FileKv.list` reads `4 + key` bytes of each entry file (a 4-byte length, then the key; a length that does not fit
  the file, a short file or a key that is not UTF-8 is "not one of ours", as before). Measured on the Mac's JVM, warm, a scratch
  program next to the old loop: **100 entries of 2 MB: 19-37 ms before, 1.6-3.3 ms after**; **1,000 entries of 10 KB: 23-35 ms
  before, 24-49 ms after, no change** (the `open` dominates and the machine was loaded; the review measured the same on the
  emulator, 22-34 ms). So the win is for big values, not for many small ones. The new case also lists a sparse 3 GiB entry file
  (skipped off APFS, ext4, tmpfs, overlay, xfs, btrfs and zfs), which the whole-file read could not hold.
* Still open from the review: F3 (`FsAdapter.io` should map `InvalidPathException`; only the Android wrapper does), F4 (sweep
  `*.tmp`), F5 (Http on its own dispatcher slice when coroutines is bumped), F6 (the `installed` registry retains cores), I3, I4,
  I11.

### Verification on the merged tree (`main` = `e518653`, merged in three steps: `c186665`, `18cf0b8`, `e518653`)

| Check | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` (with `tsc` on `PATH` and `UNDRA_REQUIRE_TOOLCHAINS=1`) | 2,351 passed, 0 failed, 11 ignored after the last merge (2,349 before the React Native merge, = main's then; the two more are its CLI tests) |
| `runtimes/kotlin/undra-runtime/scripts/test-local.sh`, brew's kotlinc 2.4.20 | 588 cases in 36 suites, 0 failed, 2 skipped (JNI smoke; no native library): main's 585 + the three new |
| the same under CI's compiler, kotlinc 2.0.21 first on `PATH` (own build dir, `UNDRA_FORCE=1`, the stdlib of that distribution) | 588 cases, 0 failed, 2 skipped; and with the real JNI library (`cargo build -p undra-ffi --features jni`, `UNDRA_NATIVE_LIB_DIR=target/debug UNDRA_NATIVE_NAME=undra_ffi`, as CI runs it) 588 cases, 0 failed, 1 skipped. The android-adapters module and the runtime's main sources are compiled by Gradle's Kotlin 2.0.21 in `:android-adapters:test`, so both Kotlin versions have built them |
| `./gradlew :android-adapters:test` | 131 tests per variant (debug and release), 130 pass, 1 skipped (PATCH on the desktop JVM) |
| `./gradlew :android-adapters:connectedAndroidTest` on `emulator-5554` (`undra` AVD, Android 15) | 113 tests, 112 pass, 0 failed, 1 skipped (the gated network toggle); the Gradle log says `Starting 113 tests on undra(AVD) - 15` and ends `undra(AVD) - 15 Tests 110/113 completed. (1 skipped) (0 failed)`, `Finished 114 tests on undra(AVD) - 15`; per class: Fs 14, Http 41, Kv 13, SecureStore 10 + 9 on the real Keystore, Connectivity 6, Lifecycle 7, `install` 5, Http on device 5, locations 3 |
| `examples/playground/android`: `./gradlew :app:assembleDebug` | `BUILD SUCCESSFUL`, 15 MB debug APK, on a core built by `undra build --platform android --release` |
| `smoke.sh` (`SKIP_CORE=1`) on a **private AVD** (`undra-aamerge`, `emulator-5562`), because it uninstalls the app and switches airplane mode | `SMOKE PASSED`: four tabs alive, `AndroidPlatformDefaults: registered Kv, SecureStore, Fs, Http, Clock, Rng, Log and Timer; reporting Connectivity and Lifecycle`, one file in `files/undra/kv` after the fetch and two after queueing, the cached list shown from `Kv` offline with the time of the fetch before the kill, 0 POSTs while offline, the replay with the original Idempotency-Key, 0 crash markers. `.proof/android-adapters-smoke.log` is the new log; the PNGs are the earlier run's. |
| launch of the merged playground, in process and against `undra dev` (`undra dev --addr 127.0.0.1:7471 --no-watch`, `--es undra_dev_url ws://10.0.2.2:7471`) | in process: Remote tab `Success`, the three inbox items through the real `Http` adapter and the loopback server. Remote: the **green dev bar** "Dev server: ws://10.0.2.2:7471" over the same screen (`.proof/android-adapters-merged-devbar.png`); `undra dev`'s log shows the core's port calls going out to the device and the commits coming back. Killing the dev server: `Reconnecting(attempt=1..5, ECONNREFUSED)`; starting it again: `Connected`, then `Closed(SESSION_LOST)`, `Connecting`, `Connected` and a **second** `AndroidPlatformDefaults` line (the first platform closed), the same pid, no crash, the Remote tab back on `Success` |
| `bash contract-tests/run-all.sh` | 54/54 (18 scenarios x ts, kotlin, swift), run again after the React Native merge changed two TS scenarios |
| `undra bindgen -C examples/playground --check --docs`; `node site/scripts/check-links.mjs` | up to date (hash `0x04d2adf769c58b9f`); 20 pages OK |

### Open items found while merging

* `./gradlew :runtime:test` did not compile under Gradle's Kotlin 2.0.21 (`CallErrorTests.kt:321`, a `throw`-only lambda): main fixed it
  in `7c8d2d8` while this merge was in progress; the merge carries the fix.
* A remote core loses `Connectivity` and `Lifecycle` reports made while its connection is down, and nothing reports them again
  when it returns: after airplane mode in a dev session the laptop's core can believe the device is still offline until the next
  change. A re-report on `Connected` (`AndroidPlatform.resync()`, called from `onConnectionChange`) would close it; dev-mode only.
* The Lifecycle adapter counts activities from its install: installed after the first activity started (the playground's Retry,
  a reload after `SESSION_LOST`) a dialog over the app reads as `Background` instead of `Inactive` until the next activity change.
  Benign, documented in the README; a process-wide tracker registered once would fix it.
* In remote mode the dev server's core hydrates its query cache when it starts, before any device is connected, so the `Kv` port is
  unavailable for the five seconds the query layer waits and the cache starts empty (`WARN undra::query: the Kv port never became
  available`). Already so before this merge (the in-memory `Kv` was registered after the core started too).
