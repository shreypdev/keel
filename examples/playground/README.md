# The Keel playground

The reference app of Keel v1, and the first thing we ship on (constitution R10). One Rust core is the
whole app logic; a React web app, a SwiftUI iOS app and a Jetpack Compose Android app are three
UIs over it. Nothing in `core/` is special: it uses only the public `keel` API, the way your app's
core will.

```
examples/playground/
  keel.toml        the Keel project: core path, names of the generated bindings, platforms
  core/            the Rust core (crate playground-core): todos, counter, 10k list, remote query, lab, bench
  generated/       Swift package, Kotlin module and npm package: written by `keel bindgen`, committed
  web/             React + Vite app         -> build/web/keel_core.wasm
  ios/             SwiftUI app (Xcode)      -> build/ios/KeelCore.xcframework
  android/         Compose app (Gradle)     -> build/android/jniLibs/<abi>/libkeel_core.so
  build/           what `keel build` writes (not committed)
  .proof/          screenshots and logs of the apps running on Chromium, the iOS simulator and an emulator
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
why every scenario can be driven with `keel::ports::fakes` in Rust and with an in-memory server on
each platform. The apps have no server either: each supplies its own in-memory `Http` adapter and tells the
core where "the server" is with `configure_remote`.

## Commands

All of them use the `keel` CLI (`cargo build -p keel-cli`; the binary is `target/debug/keel`):

```sh
keel bindgen -C examples/playground --docs      # after changing a public type: Swift, Kotlin, TypeScript again
keel bindgen -C examples/playground --docs --check   # CI: fail when generated/ is stale
keel build   -C examples/playground --platform web,host,ios,android [--release]
cargo test -p playground-core                    # the core's own tests (TestRuntime + keel::ports::fakes)
```

Then run an app (each directory has its own README section in the files it ships):

* web: `cd web && npm install && npm run dev`
* iOS: open `ios/PlaygroundApp.xcodeproj` (first `keel build --platform ios`)
* Android: `cd android && ./gradlew :app:installDebug` (first `keel build --platform android`)

`keel dev -C examples/playground` serves the core over a WebSocket: start an app against it (web:
`?keel=ws://127.0.0.1:7443`; iOS: the `KEEL_DEV_URL` environment variable) and edit `core/` to see the
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
