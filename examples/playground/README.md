# The Undra playground

The reference app of Undra v1, and the first thing we ship on (constitution R10). One Rust core is the
whole app logic; a React web app, a SwiftUI iOS app and a Jetpack Compose Android app are three
UIs over it. Nothing in `core/` is special: it uses only the public `undra` API, the way your app's
core will.

```
examples/playground/
  undra.toml        the Undra project: core path, names of the generated bindings, platforms
  core/            the Rust core (crate playground-core): todos, counter, 10k list, remote query, lab, bench
  generated/       Swift package, Kotlin module and npm package: written by `undra bindgen`, committed
  web/             React + Vite app         -> build/web/undra_core.wasm
  ios/             SwiftUI app (Xcode)      -> build/ios/UndraCore.xcframework
  android/         Compose app (Gradle)     -> build/android/jniLibs/<abi>/libundra_core.so
  build/           what `undra build` writes (not committed)
  .proof/          screenshots and logs of the apps running on Chromium, the iOS simulator and an Android emulator
```

## What the core shows

| Module | Shows | Where the UIs use it |
|---|---|---|
| `todos` | a store with a keyed list, a filter and two computed values (`visible`, `remaining`); an `async` command with a typed error | Todos tab |
| `counter` | a store whose commands are transactions: one change-set for `count`, `changes` and the computed `parity` | Counter tab |
| `biglist` | 10,000 keyed rows; insert, update, move and remove of one row each cross as a **one-operation patch** (R5) | 10k tab |
| `remote` | a query (`remote_todos`), mutations and optimistic commands over the `Http` port: cached per list, fresh for 30 s, persisted, retried, queued while offline and replayed | Remote tab |
| `lab` | every wire type, sync and async calls, typed errors, panics, cancellation, streams with backpressure | the contract tests |
| `bench` | the hooks of the budget rows (next section) | `bench/` |

The core reads no clock and no random source and starts no thread (R12): identities come from
counters, time from the `Clock` port, delays from `Ctx::sleep`, the network from the `Http` port. That is
why every scenario can be driven with `undra::ports::fakes` in Rust and with an in-memory server on
each platform. The apps have no server either: each supplies its own in-memory `Http` adapter and tells the
core where "the server" is with `configure_remote`.

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
* iOS: `undra build -C examples/playground --platform ios`, then open `ios/PlaygroundApp.xcodeproj`
  (`UNDRA_LINK_CORE=1` in the environment of Xcode, see the project settings), or run `ios/smoke.sh` to
  build, launch every tab on the simulator, screenshot it and run the XCUITest tour.
* Android: `undra build -C examples/playground --platform android --release`, then `cd android && ./gradlew
  :app:installDebug` (`android/README.md` has the `adb` commands). `--release` is what you package: a debug
  core is 42 MB per ABI.

The apps have no server: each supplies an in-memory `Http` adapter ("a server in a few lines of Swift, Kotlin
or TypeScript") that answers for `https://playground.undra.test`, and an Offline switch on the Remote tab makes
it fail every request and tells the core through the `Connectivity` port, so you can watch the offline queue
hold an optimistic add and replay it.

`undra dev -C examples/playground` serves the core over a WebSocket: start an app against it (web:
`?undra=ws://127.0.0.1:7443`; iOS: the `UNDRA_DEV_URL` environment variable) and edit `core/` to see the
change without rebuilding the app.

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
| Core cold start with a snapshot restore | `BigList` or `Bench` (about 250 KB of state each) through `snapshot` / `restore` |

## Contract tests

`contract-tests/` runs the seventeen scenarios of SPEC section 14 against this core from the TypeScript
(wasm), Kotlin (JNI) and Swift (C ABI) runtimes; see `contract-tests/scenarios.md`.
