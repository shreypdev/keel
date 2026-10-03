# The Undra playground

The reference app of Undra v1, and the first thing we ship on (constitution R10). One Rust core is the
whole app logic; a React web app, a SwiftUI iOS app and a Jetpack Compose Android app are three
UIs over it. Nothing in `core/` is special: it uses only the public `undra` API, the way your app's
core will.

```
examples/playground/
  undra.toml        the Undra project: core path, names of the generated bindings, platforms
  core/            the Rust core (crate playground-core): todos, counter, 10k list, remote query, lab, bench, stress
  generated/       Swift package, Kotlin module and npm package: written by `undra bindgen`, committed
  web/             React + Vite app         -> build/web/playground_core.wasm
  ios/             SwiftUI app (Xcode)      -> build/ios/PlaygroundCore.xcframework
  android/         Compose app (Gradle)     -> build/android/jniLibs/<abi>/libplayground_core.so
  rn/              React Native app         -> build/ios/PlaygroundCore.podspec + jniLibs (undra build --platform rn)
  build/           what `undra build` writes (not committed)
  .proof/          screenshots and logs of the apps running on Chromium, the iOS simulator and an Android emulator
```

## What the core shows

| Module | Shows | Where the UIs use it |
|---|---|---|
| `todos` | a store with a keyed list, a filter and two values derived from it (`visible`, a `DerivedList` sent as keyed patches; `remaining`, its `count()`); an `async` command with a typed error | Todos tab, contract scenario S19 |
| `counter` | a store whose commands are transactions: one change-set for `count`, `changes` and the computed `parity` | Counter tab |
| `biglist` | 10,000 keyed rows; insert, update, move and remove of one row each cross as a **one-operation patch** (R5) | 10k tab |
| `remote` | a query (`remote_todos`), mutations and optimistic commands over the `Http` port: cached per list, fresh for 30 s, persisted, retried, queued while offline and replayed | Remote tab |
| `lab` | every wire type, sync and async calls, typed errors, panics, cancellation, streams with backpressure | the contract tests |
| `bench` | the hooks of the budget rows (next section) | `bench/` |
| `stress` | high-frequency data: a generator the core runs on its own, paced by the `Timer` port and corrected by the `Clock` port, one transaction per update; a `no_coalesce` signal next to a merged one | Stress tab (web), contract scenario S18 |

The core reads no clock and no random source and starts no thread (R12): identities come from
counters, time from the `Clock` port, delays from `Ctx::sleep`, the network from the `Http` port. That is
why every scenario can be driven with `undra::ports::fakes` in Rust and with an in-memory server on
each platform. The apps have no server either: each supplies its own in-memory `Http` adapter and tells the
core where "the server" is with `configure_remote`. (Android is the exception: it runs on the real platform adapters of
`android-adapters` and talks over a real socket to a small HTTP server inside the app.)

## Commands

All of them use the `undra` CLI (`cargo build -p undra-cli`; the binary is `target/debug/undra`):

```sh
undra bindgen -C examples/playground --docs      # after changing a public type: Swift, Kotlin, TypeScript again
undra bindgen -C examples/playground --docs --check   # CI: fail when generated/ is stale
undra build   -C examples/playground --platform web,host,ios,android [--release]
cargo test -p playground-core                    # the core's own tests (TestRuntime + undra::ports::fakes)
```

Then run an app:

* web: `undra build -C examples/playground --platform web`, then `cd web && npm ci && npm run dev`
  (`npm test` runs the fake server's tests, `npm run smoke` builds and drives the app in headless Chromium).
* iOS: `undra build -C examples/playground --platform ios`, then open `ios/PlaygroundApp.xcodeproj`,
  or run `ios/smoke.sh` to build, launch every tab on the simulator, screenshot it and run the XCUITest tour.
* Android: `undra build -C examples/playground --platform android --release`, then `cd android && ./gradlew
  :app:installDebug` (`android/README.md` has the `adb` commands), or `android/smoke.sh` to build, install, tour the
  tabs and drive the offline story. `--release` is what you package: a debug core is 42 MB per ABI.

The web and iOS apps have no server: each supplies an in-memory `Http` adapter ("a server in a few lines of Swift
or TypeScript") that answers for `https://playground.undra.test`. The Android app uses the real adapters of
`android-adapters` and a small HTTP server inside the app on the device's loopback interface (`android/smoke.sh` drives
it). On every platform an Offline switch on the Remote tab makes the server fail every request and tells the core through
the `Connectivity` port, so you can watch the offline queue hold an optimistic add and replay it; on Android the queue
also survives the process being killed, and real airplane mode does the same.

`undra dev -C examples/playground` serves the core over a WebSocket: start an app against it and edit `core/` to see
the change without rebuilding the app (`docs/DEV_LOOP.md` is the whole story; `undra dev` prints these with the
real port):

| App | Against `undra dev` |
|---|---|
| web | `?undra=ws://127.0.0.1:7443` in the page URL (or `VITE_UNDRA_DEV_URL`) |
| iOS simulator | `SIMCTL_CHILD_UNDRA_DEV_URL=ws://127.0.0.1:7443 xcrun simctl launch booted dev.undra.playground`, or `UNDRA_DEV_URL` in the scheme |
| Android emulator | `adb shell am start -n dev.undra.playground/.MainActivity --es undra_dev_url ws://10.0.2.2:7443` |
| Android USB device | `undra dev --android` (it runs `adb reverse tcp:7443 tcp:7443`), then `--es undra_dev_url ws://127.0.0.1:7443` |

Edit a Rust function, save, and each app reconnects by itself and is on the rebuilt core with the state it had (ADR-053:
`undra dev` snapshots the old core and restores it into the new one, so the counter keeps its value and the 10,000-row
list its rows). A thin bar at the top shows what the connection is doing, green connected, amber reconnecting, red over,
and for four seconds what the dev server says about a reload: "Reloaded, state kept". A dropped connection (the laptop
slept, adb restarted) resumes the same objects with their state too; `.proof/dev-loop/` has screenshots of the Android
app doing both. When the state cannot be carried (a schema change, a snapshot over 16 MiB, `--no-keep-state`) the apps
load the new core and start over, and the bar says why. The Remote tab's query handle comes back too (ADR-059): the
snapshot keeps what it is made of and the tab observes it again after the reconnect, with no code in the app, and
pull-to-refresh works. `remote_todos` is `persist`ed, so the tab shows the data its last fetch stored and refetches by its
30 s window; a query without `persist` (the ticker, the feed) shows its loading state once and then the data the rebuilt
code fetched (docs/DEV_LOOP.md).

## The stress screen

The **Stress** tab (web, `?screen=stress`) pushes the runtime and shows what it did, measured in your browser.
`Stress.start(mode, per_second)` (`core/src/stress.rs`) makes the core generate updates by itself: a task sleeps
10 ms on the `Timer` port and commits `rate x elapsed` updates per tick (the elapsed time from the `Clock` port,
the remainder carried), each its own transaction, so the platform receives one change-set per update. `value`
(firehose) is merged by the mirror once per frame; `progress` (progress) is `#[undra(no_coalesce)]` and applied
every time. `generated` and `running` are signals the UI reads; `stop()` ends the generator; `burst(mode, n)` commits
`n` updates at once (contract scenario S18).

The web screen (`web/src/views/StressView.tsx`, math in `web/src/stress-stats.ts`) offers firehose or progress at
1k, 10k, 50k or 100k updates a second and reports generated, received and applied updates a second (and the merge
ratio), drains a second with p50 and p99 duration, nanoseconds per change-set, dropped frames and the JS heap
(Chrome only). `?screen=stress&rate=100000&mode=firehose&autostart=1` starts it on load (`rate` takes `1..=1000000`
or a count of thousands such as `100k`; a rate the chips do not offer gets a chip of its own); embedded (`embed=1`) it
posts the same numbers to the landing page as the `undra-stats` message. `docs/HIGH_FREQUENCY.md` explains the
numbers; the iOS and Android screens are the device phase.

## Benchmark hooks

The blueprint's section 14 budget rows are measured against one store, `Bench`
(`core/src/bench.rs`), with 128 counters (`s000` .. `s127`, signal ids `1..=128`) and a 10,000-row keyed list
(signal `0`):

| Budget row | Call |
|---|---|
| Handle method call, primitive arguments and return | `Bench.bench_add(a, b)` |
| 1 KB record, round trip | `Bench.bench_echo_bytes(data)` with 1,024 bytes |
| Change-set with 100 dirty signals, applied on the main thread | `Bench.bench_touch_signals(100)` |
| Keyed patch on a 10,000-item list, one insert | `Bench.bench_list_insert(i)`, then `bench_list_reset()` to start over |
| Core cold start with a snapshot restore | `BigList` or `Bench` (about 250 KB of state each) through `snapshot` / `restore`; the device bench restores 1,000 to-dos of 80 characters (100 KB) |
| ADR-031 drain: 1,667 one-update keyed patches in one frame | `Bench.bench_list_update_burst(1667)`: one transaction, so one change-set, per update |

### The device bench

The three apps run these hooks themselves, through the generated bindings and the platform's mirror, when asked to:
`scripts/bench-device.sh --device ios|android|web` builds the core and the app for the target, drives it with the
harness its smoke test uses and writes `bench/results/device/<date>-<target>.json` and the device tables of
`bench/RESULTS.md`. The app side is `web/src/bench/` (a separate page, `bench.html`, run by `npm run bench`),
`ios/PlaygroundApp/Bench/` (the app starts in benchmark mode with `-bench full`; `PlaygroundBenchTests` is the
XCUITest, skipped unless `TEST_RUNNER_UNDRA_BENCH=1`) and `android/app/src/main/kotlin/.../bench/` (an instrumented
test, `BenchInstrumentedTest`, over a `benchmark` build type: release, not debuggable, signed with the debug key).
`bench/RESULTS.md` ("Device numbers") says how every row is timed and what the labels mean.

## Contract tests

`contract-tests/` runs the nineteen scenarios of SPEC section 14 against this core from the TypeScript
(wasm), Kotlin (JNI) and Swift (C ABI) runtimes; see `contract-tests/scenarios.md`.
