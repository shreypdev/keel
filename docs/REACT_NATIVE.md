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

   Nothing else: autolinking builds the C++ module into the app's `libappmodules.so` (it opens
   `libundra_core.so` from the APK at start) and links the package's small Android library (Java, no
   dependency), which the default ports use for the Keystore and the network state. Its manifest merges
   `ACCESS_NETWORK_STATE` and a provider that hands it the application context; an app that removes the
   provider (`tools:node="remove"`) calls `UndraPlatform.install(context)` in `Application.onCreate`.

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
});                                                    // every port has a default ("Ports")
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

### Ports

Every standard port has a default, so an app writes no adapter (ADR-038, amendment B). Where React Native has
the API, the default is JavaScript; where it has none (no file system, key-value store, keychain or network state
in its core, and the package takes no community module), the default is the module's own native code, which
answers the core directly, off the JS thread:

| Port | Default | iOS | Android |
|---|---|---|---|
| `Kv` | native (C++, one implementation for both) | files in `<Application Support>/<bundle id>/Undra/kv`, the Swift runtime's layout | files in `<filesDir>/undra/kv`, `android-adapters`' layout |
| `SecureStore` | native | the Keychain (service `dev.undra.securestore`, `AfterFirstUnlockThisDeviceOnly`), the Swift runtime's items | AES-256-GCM under the Android Keystore key `dev.undra.securestore`, sealed files in `<noBackupFilesDir>/undra/secure`, `android-adapters`' layout |
| `Fs` | native (C++) | `<Application Support>/<bundle id>/Undra/fs` | `<filesDir>/undra/fs` |
| `Connectivity` | native | `NWPathMonitor` (Network.framework) | `ConnectivityManager` default-network callback |
| `Http` | `reactNativeHttp()`: React Native's `fetch` | `NSURLSession`, through React Native's networking | OkHttp, through React Native's networking |
| `Lifecycle` | `AppState` | `active`, `inactive`, `background` | `active`, `background` (React Native reports no `inactive` on Android) |
| `Clock`, `Rng`, `Log` | native, in the module (`Log` records also reach your `log` adapter, default the console) | | |
| `Timer` | the core's own timer thread | | |

Because the directories, file layouts, Keychain items and Keystore key are the ones the Swift and Kotlin runtimes
use on the same platform, a value the SwiftUI or Compose shell of an app wrote is read by its React Native shell
and the reverse. What each default does:

* `Kv` and `SecureStore`: one file per key, written to a temporary file, flushed and renamed, so a killed app keeps
  the old value or the new one. They have no error channel: a failing disk, Keychain or Keystore answers the core
  "unavailable" (its call fails and the reason is logged), never "missing" for a value that exists.
* `Fs`: paths are relative to the root; any `..` and any symbolic link on a path is `FsError.Denied`, so the core
  cannot leave the root; `write` creates directories and is atomic; `delete` removes a directory with everything in
  it, never the root.
* `Http`: any status is a response. No connection (airplane mode, a refused connection, a DNS failure) is
  `HttpError.Network`, never "unavailable"; the request's timeout covers the body and is `HttpError.Timeout`; a URL
  that is not `http(s)://host...` is `HttpError.InvalidUrl`. Bodies cross React Native's networking module as base64
  (its limitation). Android blocks `http://` unless the app's network security config allows the host.
* `Connectivity`: the current state as soon as the core is up, then every change, identical consecutive reports
  once. Online means the default network provides internet, as `NWPathMonitor` and `android-adapters` say.

`nativePlatformDefaults()` tells which ports the module answers natively on this device and where they keep their
data.

**Replacing a default.** Pass your own adapter to `loadNative`: a value in `adapters` replaces that port's default
(a JavaScript adapter, as on the web), `null` removes it (the core sees the port as unavailable), and an
implementation in `ports` replaces it too. The playground keeps every default and wraps one, to show how:

```ts
await loadNative({
  expectedSchemaHash: UndraIds.schemaHash,
  adapters: { http: taggedHttp(reactNativeHttp()) },   // a header on every request, then the default
});
```

Replace a native default there, not with `core.registerPort` after the load: the module registers its native ports
before `undra_init` (so start-up work such as the query cache's hydration never races them), and a JavaScript
registration made later does not reach a port the module answers itself.

Your own async ports work as on the web (`core.registerPort` or `ports:`). A port the core calls
**synchronously** (`#[undra::port(sync)]`) that you implement in JavaScript is answered only when the
core calls it from a call you made on the JS thread; from the core's own thread it is unavailable (and
logged once), because the core must never wait for the JS thread. Implement such a port natively, or
make its methods `async`.

**Why the Android half is Java, not Kotlin.** It is a few hundred lines that only call platform APIs, and Java
adds no Kotlin plugin whose version would have to match the app's and no standard library to the APK; it has no
dependency at all. (Reusing `android-adapters` itself was not possible: it is not on a Maven repository an npm
package can reach, and it is built on the Kotlin runtime and coroutines, a second Undra host in an app whose core
the C++ module drives.)

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
* JavaScript-implemented synchronous ports: see "Ports".
* `Clock`, `Rng` and `Timer` are native and cannot be replaced from JavaScript.
* A native default (`Kv`, `SecureStore`, `Fs`, `Connectivity`) is replaced in `loadNative`'s options, not with
  `core.registerPort` afterwards ("Ports").
* `Http` runs on the JS thread's `fetch`: while the JS thread is busy the core's requests wait for it, and bodies
  are base64 across React Native's networking module. A native `Http` is a follow-up if a measurement asks for one.
* In the background the mirror waits: React Native on Android pauses the JavaScript timers of a backgrounded app,
  and the mirror's drain runs on one, so what the core produced meanwhile (a `Lifecycle` or `Connectivity` report,
  a query that refetched) reaches your signals when the app is back, folded to the last value of each signal unless
  the signal is `#[undra(no_coalesce)]`. The core's own threads run on as long as the system lets the app run. If
  your app needs every transition, keep them in the core (a counter, or a bounded history in a signal) rather than
  relying on the mirror to replay them: React and the other UI frameworks render the last value of a frame anyway.

## What is tested where

| Layer | How | Command | In CI |
|---|---|---|---|
| The C++ host (inbox, ports, ownership, shutdown), both shims; the native default ports through the real core (the playground core's `platform` module, a test platform); the JSI layer compiled against React Native 0.87's headers with `-Werror`; the Apple platform against the iOS SDK (macOS) | against the real core, ASan + UBSan | `runtimes/rn/@undra/react-native/cpp/test/run.sh` | `ci.yml`, job "React Native (host + model)", every push and pull request (Linux, clang 18) |
| The portable `Kv` and `Fs` (layouts against the Swift runtime's own vectors and SHA-256's, `..`, symbolic links, a directory swapped for a link while a read or write walks through it, atomic writes and a writer killed mid-write, deep deletes) | this machine's file system, ASan + UBSan | the same `run.sh` (step 0) | the same job |
| The Android library's pure Java (the seal against an AES-GCM vector from Node, which `android-adapters` opens too; the network classification) | `javac` and the JDK | `runtimes/rn/@undra/react-native/android/test/run.sh` | the same job |
| `NativeTransport`, the frame scheduler, the polyfills, `loadNative`, which ports are native for which options, `reactNativeHttp()` | a fake module and a scripted `fetch`, on Node | `npm test` in `runtimes/rn/@undra/react-native` | the same job |
| Types of the package and its build config | `tsc`, against `@undra/runtime`'s sources and its emitted declarations (`npm run build` in `runtimes/ts/@undra/runtime` first) | `npm run typecheck` | the same job |
| The contract scenarios S01..S18 | through `NativeTransport` over a stand-in of the module on the wasm core: 17 pass, S17 (native panic containment) is app-tested | `npm run test:contract` | the same job |
| JSI, Hermes, `invokeAsync`, the vsync sources, both builds, native panic containment; every default port on the real platform | the playground app's on-device checks RN01..RN16 (RN11..RN16: each default through the core), then the script's RN17..RN20 (a secret not in the app's files, `Kv` across a killed process, `Lifecycle` to the background and back, `Connectivity` in airplane mode on Android): the script prints `UNDRA-RN CHECKS 20/20 passed` (19/19 on the iOS simulator, which has no airplane mode) | `scripts/rn-device-checks.sh ios` (iPhone simulator) and `scripts/rn-device-checks.sh android` (emulator or phone; RN17 needs `su`, which emulator images have) | `rn-devices.yml`: on demand, every Monday, and on pull requests that touch `runtimes/rn/**` or `examples/playground/rn/**` |

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
| The default ports (ADR-038 amendment B), on `wt/rn-adapters`, after its review | the C++ under ASan + UBSan on macOS: the stores test, then the host test through the real core with each shim (a reload's start while an old port worker is mid-job included); the Java on the JDK; `npm test` | 15 + 22 + 22 checks, the Apple platform compiles against the iOS SDK; 4 Java checks; 60 tests, typecheck clean; the contract column unchanged (17, S17 app-tested) |
| The default ports on the devices | iPhone 17 Pro simulator (iOS 26.5) and the `undra-rn` emulator (arm64, API 35), release core, Release app, `scripts/rn-device-checks.sh ios` / `android` | `UNDRA-RN CHECKS 19/19 passed` (iOS: RN01..RN19) and `UNDRA-RN CHECKS 20/20 passed` (Android: RN01..RN20, airplane mode included) |
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
| `the native default ports are off on this device: ...` in the log (Android) | the package's Android library is not in the app (run the Gradle sync after adding the package), or its context provider was removed without `UndraPlatform.install(context)`; until then `Kv`, `SecureStore`, `Fs` and `Connectivity` are unavailable unless you pass adapters |
| `Http` calls fail with `HttpError.Network` mentioning cleartext (Android) | Android blocks `http://` unless the app's network security config allows the host; use `https://` or allow it (the playground allows only its loopback) |
| `UndraSchemaMismatchError` | the linked core and the generated bindings come from different schemas: run `undra bindgen` and `undra build --platform rn` again |
| `another Undra core is running in this process` | `loadNative` (or `NativeTransport.start`) was called while a core is open; use `UndraCore.shared`. The running core is not disturbed. (After a reload, a core whose old runtime is still shutting it down is waited for, up to 5 s, before this is reported.) |
| `UndraTransportError("closed")`, or `UndraCallError.Unavailable` with `transport.reason` `closed`, in a runtime that did not close its core | another React Native instance of the process (or the reloaded runtime) started a core, which stops this one (Limits) |
