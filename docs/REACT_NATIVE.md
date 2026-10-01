# Undra under React Native

`@undra/react-native` puts your Undra core under a React Native app: the same native library the
SwiftUI and Compose apps link (`libundra_core.a` on iOS, `libundra_core.so` on Android), reached from
JavaScript through JSI, with the TypeScript runtime's `UndraCore`, mirror and React hooks on top and
the bindings `undra bindgen` generates for TypeScript, unchanged. The design is ADR-038
(`.10x/adrs/ADR-038-react-native-host.md`); the reference app is `examples/playground/rn`.

```
 React components ── useSignal / useUndra (@undra/runtime/react)
        │
 generated bindings (@your/core) ── UndraCore, Mirror (@undra/runtime)
        │
 NativeTransport (@undra/react-native) ── JSI host functions (globalThis.__undraNative)
        │                                       │ one C++ module, iOS and Android
 ───────┴──────────────── C ABI (undra.h) ──────┴───────────────────────────
                         your Rust core (undra build --platform rn)
```

Calls run the core on the JS thread (a synchronous method answers before the JSI call returns);
async work runs on the core's own thread; everything the core says goes through one inbox that is
handed to JavaScript in batches, in commit order, and the mirror applies what the core produced on
its own once per display frame (ADR-031), exactly as on the web.

## Requirements

* React Native with the New Architecture, bridgeless, on Hermes. **0.87 is the version it is built and
  proven on** (the playground); the package's peer range starts at 0.82, which has the APIs it uses
  (pure C++ TurboModules on both platforms), but nothing below 0.87 has been run.
* iOS: CocoaPods (`brew install cocoapods`), and the deployment target of your core (`[ios]
  deployment_target` in `undra.toml`, 17.0 by default) as the app's `platform :ios`.
* Android: the NDK the app builds with (React Native 0.87's template asks for r27), and the ABIs of
  `[android] abis` in `reactNativeArchitectures`.

## Install

1. **Build the core** for both phones and write its pod:

   ```sh
   undra build --platform rn --release      # without --release for a debug core (much slower)
   ```

   This is the iOS and Android builds plus `build/ios/UndraCore.podspec`:

   | Under `build/` | Used by |
   |---|---|
   | `ios/UndraCore.xcframework`, `ios/UndraCore.podspec` | the app's Podfile |
   | `android/jniLibs/<abi>/libundra_core.so` | the app's Gradle `jniLibs` |

2. **Add the packages** (`@undra/runtime` and `react-native` are its peers) and your generated bindings:

   ```sh
   npm install @undra/react-native @undra/runtime
   ```

3. **Babel**: Hermes cannot compile `import.meta`, which `@undra/runtime`'s `wasm-worker` mode contains
   (that mode never runs under React Native, but Metro bundles it). Add the package's plugin:

   ```js
   // babel.config.js
   module.exports = {
     presets: ['module:@react-native/babel-preset'],
     plugins: ['@undra/react-native/babel-plugin'],
   };
   ```

   If you consume `@undra/runtime` or your bindings as TypeScript sources (as the playground does),
   also let Babel strip `declare` fields: an override with `['@babel/plugin-transform-typescript',
   { allowDeclareFields: true, allowNamespaces: true }]` for `.ts` files
   (`examples/playground/rn/babel.config.js`).

4. **iOS**: point the Podfile at the core's pod, then `pod install`:

   ```ruby
   platform :ios, '17.0'                       # your core's deployment target
   target 'App' do
     config = use_native_modules!               # links UndraReactNative (autolinking)
     pod 'UndraCore', :path => '../build/ios'   # written by undra build --platform rn
     use_react_native!(...)
   end
   ```

5. **Android**: package the core's libraries:

   ```groovy
   // android/app/build.gradle, inside android { }
   sourceSets {
       main.jniLibs.srcDirs += ["../../build/android/jniLibs"]
   }
   ```

   Nothing else: the module is a pure C++ dependency that React Native's Gradle plugin builds into the
   app's `libappmodules.so`, and it opens `libundra_core.so` from the APK at start.

6. **Import first**: `@undra/react-native` installs `TextDecoder` (Hermes has none) before anything
   loads `@undra/runtime`, so import it at the top of your entry file:

   ```js
   // index.js
   import '@undra/react-native';
   import { AppRegistry } from 'react-native';
   import App from './App';
   ```

## Use

```tsx
import { loadNative } from '@undra/react-native';
import { useSignal } from '@undra/runtime/react';
import { Todos, UndraIds } from '@acme/core';          // your generated bindings

const core = await loadNative({
  expectedSchemaHash: UndraIds.schemaHash,             // a core from another schema is refused
  adapters: { kv: myKv },                              // see "Adapters"
});
const todos = await Todos.create();                    // UndraCore.shared is the native core

function TodoCount() {
  const remaining = useSignal(todos.remaining);
  return <Text>{remaining} left</Text>;
}
```

`loadNative` accepts every option of `UndraCore.attach` (`adapters`, `ports`, `onError`, `onClose`,
`mirror`) plus `devtools`, `logLevel` and `platform`. It resolves with the same core when called again
while it is open. `core.close()` shuts the core down; a JavaScript reload in development shuts it down
and the next `loadNative` starts a fresh one.

### Adapters

| Port | Under React Native |
|---|---|
| `Clock`, `Rng`, `Log` | native, in the module (`Log` records also reach your `log` adapter, default the console) |
| `Timer` | the core's own timer thread |
| `Http` | `fetch` (the runtime's default) |
| `Lifecycle` | `AppState` (`active`, `inactive`, `background`) |
| `Kv`, `SecureStore`, `Fs`, `Connectivity` | **yours to supply**: React Native's core has no storage, file or network-state API. Wrap the library your app already uses (an MMKV or AsyncStorage `KvAdapter`, NetInfo for `Connectivity`); without one the port is unavailable, which the core treats as a normal outcome |

Your own async ports work as on the web (`core.registerPort` or `ports:`). A port the core calls
**synchronously** (`#[undra::port(sync)]`) that you implement in JavaScript is answered only when the
core calls it from a call you made on the JS thread; from the core's own thread it is unavailable (and
logged once), because the core must never wait for the JS thread. Implement such a port natively, or
make its methods `async`.

## Development

* **Fast refresh and reloads** keep working: a reload tears the JS runtime down, the module shuts the
  core down with it, and your app's `loadNative` starts a fresh core (proven on the iOS simulator and
  the Android emulator, reloading in the middle of work too).
* **`undra dev`**: in development the app can reach a core served by `undra dev` over the WebSocket
  remote transport instead of the linked one, `UndraCore.load({ mode: 'remote', url })` (Hermes has
  `WebSocket`; from the Android emulator the host is `10.0.2.2`, or `adb reverse tcp:7443 tcp:7443`).
  Reconnecting and Android parity of the dev loop are tracks B1 and B2.
* Metro can serve `@undra/runtime`, `@undra/react-native` and your bindings from their TypeScript
  sources (the playground's `metro.config.js` maps them and serves `./x.ts` for `./x.js` imports).

## Limits

* **High-rate updates do not fit a frame on React Native today.** At 100,000 keyed-patch updates a
  second (1,667 per 60 Hz frame) the mirror needs 13 to 16 ms of every 16.7 ms frame on the iPhone 17
  Pro simulator (parsing the change-sets as they arrive, 7.8 to 9.4 ms, plus the frame's drain, 5.3 to
  6.4 ms; more on the Android emulator), against 0.3 to 0.5 ms on V8: nothing is left for React. At
  10,000 a second the work scales down to roughly a tenth (scaled from those rows, not measured on
  its own; the 10,000-a-second firehose was measured to drain once per frame). The cost is `@undra/runtime`'s JavaScript under Hermes,
  not the boundary; it is open work for that package (Amendment D, E4), and until it lands, keep
  core-driven update rates near 10,000 a second or below on React Native (ADR-038, "Measurements").
* Per-call cost is the same JavaScript: a synchronous call is about 0.2 us through JSI and the core,
  about 6 us through `UndraCore.callSync` (4 us of it building the payload), and 15 to 20 us as an
  awaited generated method on the iOS simulator.
* One Undra core per process (also across languages: not a Swift or Kotlin Undra host next to it)
  until ADR-044's per-core function table lands. That includes two React Native instances in one
  process (a brownfield app with two `ReactHost`s): the module cannot tell a second instance from a
  reloaded one, so the second `loadNative` stops the first instance's core, whose calls then fail
  with `UndraTransportError("closed")` (`UndraCallError.Unavailable` through the generated bindings, whose
  `transport.reason` is `closed`); they never reach the other instance's core.
* JavaScript-implemented synchronous ports: see "Adapters".
* `Clock`, `Rng` and `Timer` are native and cannot be replaced from JavaScript.

## What is tested where

| Layer | How | Command | In CI |
|---|---|---|---|
| The C++ host (inbox, ports, ownership, shutdown), both shims; the JSI layer compiled against React Native 0.87's headers with `-Werror` | against the real core, ASan + UBSan | `runtimes/rn/@undra/react-native/cpp/test/run.sh` | `ci.yml`, job "React Native (host + model)", every push and pull request (Linux, clang 18) |
| `NativeTransport`, the frame scheduler, the polyfills, `loadNative` | a fake module, on Node | `npm test` in `runtimes/rn/@undra/react-native` | the same job |
| Types of the package and its build config | `tsc`, against `@undra/runtime`'s sources and its emitted declarations (`npm run build` in `runtimes/ts/@undra/runtime` first) | `npm run typecheck` | the same job |
| The contract scenarios S01..S18 | through `NativeTransport` over a stand-in of the module on the wasm core: 17 pass, S17 (native panic containment) is app-tested | `npm run test:contract` | the same job |
| JSI, Hermes, `invokeAsync`, the vsync sources, both builds, native panic containment | the playground app's on-device checks RN01..RN10 (and the Bench screen's measurements): the app logs `UNDRA-RN CHECKS 10/10 passed` | `scripts/rn-device-checks.sh ios` (iPhone simulator) and `scripts/rn-device-checks.sh android` (emulator or phone) | `rn-devices.yml`: on demand, every Monday, and on pull requests that touch `runtimes/rn/**` or `examples/playground/rn/**` |

The contract column proves `NativeTransport` and the module's rules as modelled by the stand-in, not the
C++ module: that is the C++ host test plus RN01..RN10 on a device. Count it as "`NativeTransport` over a
stand-in", never as a native column next to Swift and Kotlin.

## What is verified

Everything below was run on 2026-10-01 at `wt/react-native` with `main` merged in (the schema JSON, device
bench, diagnostics, dev loop and parity pieces: the typed failure model, `snapshot()` and `restore()`), on a Mac (Apple clang, Xcode 26.6) shared with other agents' builds.

| What | Where | Result |
|---|---|---|
| The C++ host, both shims, ASan + UBSan | macOS; also a clean `git clone` (Node 20) running the CI job's steps | 14 + 14 checks, `UndraJsi.cpp` compiles against 0.87's headers |
| `NativeTransport` and friends | Node 24 and Node 20 | 41 tests; typecheck clean |
| Contract scenarios through `NativeTransport` | Node 24 and Node 20 | 17 pass, S17 skipped (app-tested); S15 uses the public `core.snapshot()` and `core.restore()` |
| The on-device checks, iOS | iPhone 17 Pro simulator (iOS 26.5), release core, Release app, `scripts/rn-device-checks.sh ios` | `CHECKS 10/10 passed` |
| The on-device checks, Android | `undra-rn` emulator (arm64, API 35), release core, release APK, `scripts/rn-device-checks.sh android` | `CHECKS 10/10 passed` |
| JavaScript reload, iOS | debug build on Metro, `POST /reload`: three reloads, one in the middle of the benchmarks (review, before the merge) | four runtimes in one process, 10/10 each |
| JavaScript reload, Android, with the review's fixes | debug build on Metro, `POST /reload`, one process throughout: an idle reload, two in the middle of the benchmarks and two more in quick succession (6 runtimes); three reloads timed to land inside the self-checks (5 runtimes); then 20 in a row (21 runtimes) | every run that reached its end passed 10/10 (6, 2 and 21 runs; the interrupted ones print no verdict), no `CHECK ... FAIL`, no native crash in `logcat` |
| Android frame source, per reload | the same reloads with a counter on the choreographer callbacks (not committed) | 0 callbacks pending when the source was destroyed in 66 reloads (26 plain, 40 with the 10k list's Stream running), `posted == ran` every time; the native heap read from `dumpsys meminfo` is flat within the noise (see below) |
| `rn-devices.yml` and the CI job on a GitHub runner | not yet: a runner has not run them | the job's steps were run in a clean clone on the Mac (Linux-only parts, `apt` and `clang++-18`, were not) |

On Android a callback posted with `AChoreographer_postFrameCallback` that is still pending when a reload
quits the JS thread's looper never runs, and the one small allocation it carries (a `std::weak_ptr`, and
the frame source's state it keeps alive, about 100 bytes) is not freed. That needs a frame requested in
the instant of the teardown; it did not happen once in the 66 reloads above (the counter saw no pending
callback each time the source was destroyed), and the native heap (`dumpsys meminfo`, Heap Alloc) was
112.6 MB at launch and 100.6, 101.6, 100.1 and 100.6 MB after 5, 10, 15 and 20 reloads (a second run: 111.0, then 98.8, 99.9, 100.0, 99.4): the noise of a
React Native reload (about a megabyte) is ten thousand times larger than that. It is a development-only cost, bounded by the
number of reloads. The fix, if it is ever seen, is to pass an id instead of a pointer and keep the
pending states in a table that the source's destructor empties.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `Property 'TextDecoder' doesn't exist` at start | something imported `@undra/runtime` (your bindings) before `@undra/react-native`: import it first (Install, step 6) |
| `'import.meta' is currently unsupported` from the Hermes compiler | add `@undra/react-native/babel-plugin` (step 3) |
| `the UndraNative TurboModule is not linked into this app` | the package is not a dependency of the app, or `pod install` / the Gradle sync did not run after adding it |
| `cannot load the Undra core libundra_core.so` (Android) | the core's `jniLibs` are not packaged (step 5), or not built for the device's ABI |
| Undefined `_undra_*` symbols when linking (iOS) | the `UndraCore` pod is missing from the Podfile (step 4) |
| `found architecture 'arm64', required architecture 'x86_64'` for `libundra_core.a`, then undefined `_undra_*` symbols, in a Release build for the simulator | the Release configuration builds every simulator architecture and the core's simulator slice has only `[ios] simulator_archs` (`arm64` by default): build for the active architecture (`ONLY_ACTIVE_ARCH=YES ARCHS=arm64`, as Xcode's Run does) or add `x86_64` to `simulator_archs` |
| `UndraSchemaMismatchError` | the linked core and the generated bindings come from different schemas: run `undra bindgen` and `undra build --platform rn` again |
| `another Undra core is running in this process` | `loadNative` (or `NativeTransport.start`) was called while a core is open; use `UndraCore.shared`. The running core is not disturbed. (After a reload, a core whose old runtime is still shutting it down is waited for, up to 5 s, before this is reported.) |
| `UndraTransportError("closed")`, or `UndraCallError.Unavailable` with `transport.reason` `closed`, in a runtime that did not close its core | another React Native instance of the process (or the reloaded runtime) started a core, which stops this one (Limits) |
