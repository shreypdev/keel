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

* React Native 0.82 or newer (New Architecture, bridgeless, Hermes). The playground uses 0.87.
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
  core down with it, and your app's `loadNative` starts a fresh core.
* **`undra dev`**: in development the app can reach a core served by `undra dev` over the WebSocket
  remote transport instead of the linked one, `UndraCore.load({ mode: 'remote', url })` (Hermes has
  `WebSocket`; from the Android emulator the host is `10.0.2.2`, or `adb reverse tcp:7443 tcp:7443`).
  Reconnecting and Android parity of the dev loop are tracks B1 and B2.
* Metro can serve `@undra/runtime`, `@undra/react-native` and your bindings from their TypeScript
  sources (the playground's `metro.config.js` maps them and serves `./x.ts` for `./x.js` imports).

## Limits

* One Undra core per process (also across languages: not a Swift or Kotlin Undra host next to it)
  until ADR-044's per-core function table lands.
* JavaScript-implemented synchronous ports: see "Adapters".
* `Clock`, `Rng` and `Timer` are native and cannot be replaced from JavaScript.
* Per-call cost is the TypeScript runtime's JavaScript under Hermes, which interprets bytecode: a
  synchronous call is about 0.2 us through JSI and the core, about 6 us through `UndraCore.callSync`,
  and 15 to 20 us as an awaited generated method on the iOS simulator; a 100,000-updates-a-second
  stream of keyed patches costs more than a frame to merge and apply (ADR-038, "Measurements").

## What is tested where

| Layer | How | Command |
|---|---|---|
| The C++ host (inbox, ports, ownership, shutdown), both shims | against the real core on the Mac, ASan + UBSan | `runtimes/rn/@undra/react-native/cpp/test/run.sh` |
| `NativeTransport`, the frame scheduler, the polyfills, `loadNative` | a fake module, on Node | `npm test` in `runtimes/rn/@undra/react-native` |
| The contract scenarios S01..S18 | through `NativeTransport` over a stand-in of the module on the wasm core: 17 pass, S17 (native panic containment) is app-tested | `npm run test:contract` |
| JSI, Hermes, `invokeAsync`, the vsync sources, both builds, native panic containment | the playground app's on-device checks RN01..RN10 and measurements (Bench screen, `UNDRA-RN` log lines) | `examples/playground/rn/README.md` |

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `Property 'TextDecoder' doesn't exist` at start | something imported `@undra/runtime` (your bindings) before `@undra/react-native`: import it first (Install, step 6) |
| `'import.meta' is currently unsupported` from the Hermes compiler | add `@undra/react-native/babel-plugin` (step 3) |
| `the UndraNative TurboModule is not linked into this app` | the package is not a dependency of the app, or `pod install` / the Gradle sync did not run after adding it |
| `cannot load the Undra core libundra_core.so` (Android) | the core's `jniLibs` are not packaged (step 5), or not built for the device's ABI |
| Undefined `_undra_*` symbols when linking (iOS) | the `UndraCore` pod is missing from the Podfile (step 4) |
| `UndraSchemaMismatchError` | the linked core and the generated bindings come from different schemas: run `undra bindgen` and `undra build --platform rn` again |
| `another Undra core is running in this process` | `loadNative` (or `NativeTransport.start`) was called while a core is open; use `UndraCore.shared` |
