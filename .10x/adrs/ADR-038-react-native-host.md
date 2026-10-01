# ADR-038: React Native is a fourth host of the C ABI

Status: proposed (2026-10-01, `wt/react-native`, v1.2 bet G1, founder's yes in Amendment A of
`.10x/specs/2026-10-01-v1x-default-choice-design.md`); implemented on the same branch and proven on
the iPhone 17 Pro simulator and an Android emulator (Consequences). Touches SPEC 11 (a fourth platform
runtime), SPEC 13 (a package and an `undra build` target) and SPEC 17 (a new package,
`@undra/react-native`, on top of `@undra/runtime`). **No wire change, no C ABI change, no wasm ABI
change, no schema change, no generated code change**: the React Native host calls the operations of
`undra.h` exactly as the Swift host does, and its JavaScript side reuses the TypeScript runtime's
`UndraCore`, `Mirror`, codecs and the generated TypeScript bindings unchanged. Constitution R11: a new
host is a boundary consumer with its own threading model, so it is decided here before the code.

**ADR-044 (per-core symbol namespacing, proposed after this record) changes how a host reaches a
core**: one exported `<namespace>_undra_api()` function table (C ABI v2) instead of the global
`undra_*` symbols, and a prelinked iOS image that needs no `-force_load`. Decisions 1, 11 and 13 below
bind to today's v1 symbols and are **transitional until ADR-044's `abi-table` piece lands**; how they
migrate is written in each, and the code is arranged so the migration is the two shim files of
decision 1 plus the pod (ADR-044 §8).

*Landed with `abi-table` (2026-10-01; `.10x/decisions/sde/abi-table.md`):* the transitional notes below are
what is built, with one difference. On iOS the shim does not call the core's header: the module is compiled
once for every core and names none, so each core's pod compiles a class `UndraCoreTable_<namespace>` whose
`+api` returns the table, and the shim looks the class up by name (`objc_getClass`). Android `dlsym`s
`<namespace>_undra_api` in `lib<namespace>.so` as written.

## Context

Teams that keep React Native for their UI cannot put Undra under it today:

* **The TypeScript runtime needs WebAssembly or a socket.** `@undra/runtime` reaches a core through
  `wasm-main`, `wasm-worker` (both need `WebAssembly`) or `remote` (a WebSocket to `undra dev`).
  Hermes, React Native's engine, has no `WebAssembly` object, and the remote mode is a dev loop, not a
  way to ship.
* **The native core already exists for both phones.** `undra build --platform ios` produces
  `UndraCore.xcframework` (a static `libundra_core.a` per slice) and `--platform android` produces
  `jniLibs/<abi>/libundra_core.so`, with the C ABI of SPEC 6 exported from both (the Android library
  also carries the JNI shim of SPEC 6.1, whose `JNI_OnLoad` a `dlopen` does not run). A React Native
  app on those devices can link the same library the SwiftUI and Compose apps link.
* **React Native 0.87** (current stable, 2026-10) is New Architecture only: bridgeless, JSI, C++
  TurboModules, Hermes. Native code reaches JavaScript only through `jsi::Runtime` on the JS thread and
  through `CallInvoker::invokeAsync` from any other thread. Read from its sources and measured on the
  devices of this record:
  * a library can be a **pure C++ TurboModule** on both platforms: iOS through
    `codegenConfig.ios.modules` (a class conforming to `RCTModuleProvider`), Android through
    `react-native.config.js` (`cxxModuleCMakeListsPath`, `cxxModuleCMakeListsModuleName`,
    `cxxModuleHeaderName`; the app runs the library's codegen, the library needs no Gradle project);
  * `requestAnimationFrame` in bridgeless mode is `setTimeout(0)` by definition
    (`ReactCommon/react/runtime/TimerManager.cpp`: "the same as setTimeout(0) ... may change in the
    future"). What makes it a frame today is that the timer registries tick on the display:
    `RCTTiming` on a display link on iOS, `JavaTimerManager` on `Choreographer` on Android, so a zero
    delay waits for the next vsync. Measured median interval of 60 chained callbacks: 16.67 ms (iOS
    simulator), 16.6 to 17.0 ms (Android emulator);
  * Hermes has `TextEncoder` but **no `TextDecoder`**, which `@undra/runtime` creates when its modules
    load, and its compiler rejects **`import.meta`**, which the runtime's `wasm-worker` transport uses
    (never reached under React Native, but bundled, since Metro bundles everything the entry reaches).

The host contract (SPEC 6, `undra.h`, ADR-026) is what any host must honour: callbacks run on any
thread, concurrently, possibly under the core lock; they must not unwind or call back into the core
(except the lock-free entries); `out_reply` is `malloc`ed and the core `free`s it; registrations and
`undra_shutdown` wait for running callbacks; every `UndraBuf` the core returns is freed exactly once
with `undra_buf_free`.

## Decision

1. **One C++ module, two platforms.** `@undra/react-native` is a pure C++ TurboModule named
   `UndraNative` with one method, `install()`, which installs a plain JS object
   (`globalThis.__undraNative`) whose properties are JSI host functions, one per C ABI entry the host
   needs (`start`, `call`, `callSync`, `cancel`, `streamCredit`, `observe`, `release`, `portReply`,
   `event`, `timerFired`, `snapshot`, `restore`, `statsJson`, `shutdown`, plus `abiVersion`,
   `schemaHash`, `schemaJson`, `hostCounters` and `requestFrame`). The TurboModule exists for
   discovery, autolinking and the `CallInvoker`; the hot path is a direct JSI host function call, no
   TurboModule method dispatch and no codegen types (codegen has no `ArrayBuffer`). The same C++
   (`cpp/`) builds on iOS (CocoaPods) and Android (the app's CMake, through autolinking). **Every
   reference to a core symbol is in one shim per platform** that fills one struct of function pointers
   once per process: `cpp/UndraApiLinked.cpp` (iOS: the statically linked core) and
   `cpp/UndraApiAndroid.cpp` (Android: `dlopen("libundra_core.so")` from the APK and `dlsym`); the shim
   also reads the ABI version and the schema hash once and refuses a core of another ABI version.
   *Transitional (ADR-044):* the shims will call `<namespace>_undra_api()` (iOS: the core's header;
   Android: `dlsym` on `lib<namespace>.so`), check the table's `abi_version` and copy its pointers into
   the same struct; `install(namespace)` will install one host object per core
   (`__undraNative[namespace]`), each with its own inbox. Nothing else in the module changes.

2. **The JavaScript side is a `Transport`.** `NativeTransport` implements the TypeScript runtime's
   `Transport` interface (`runtimes/ts/@undra/runtime/src/transport/transport.ts`) over
   `__undraNative`, with `synchronous = true` and `callSync`, like `wasm-main`. `loadNative(options)` is
   `UndraCore.attach(new NativeTransport(..), options)` with React Native defaults for the adapters and
   the mirror's schedule, so `UndraCore`, `Mirror` (ADR-031 rules included), the codecs, the generated
   bindings and `@undra/runtime/react` (`useUndra`, `useSignal`) work unchanged. Nothing in
   `@undra/runtime` changes. What Hermes lacks is supplied by the package, not the runtime: a UTF-8
   `TextDecoder` (and `TextEncoder`) installed on `globalThis` only where missing, from a module that
   the package's entry imports for its effect alone, first (so a bundler that makes imports lazy still
   runs it before anything loads `@undra/runtime`), and a Babel plugin
   (`@undra/react-native/babel-plugin`) that replaces `import.meta` with `{ url: undefined }`.

3. **Which thread runs the core: the same as Swift and Kotlin.** A synchronous entry (`call`,
   `callSync`, `observe`, `release`, `event`, `restore`, `cancel`) runs on the JS thread and holds the
   core lock for its duration (SPEC 5.1); async tasks, timers and port completions run on the
   `undra-core` thread and the blocking pool. The JS thread is never blocked waiting for another
   thread, and no other thread ever runs JavaScript.

4. **One inbox, one copy, commit order kept.** Every callback (`reply_cb`, `changeset_cb`,
   `stream_cb`, an async `port_cb`, a `Log` record) appends one record (`kind u8, len u32, payload`) to a
   single byte buffer under a mutex and returns; this is the one copy the contract requires (core
   memory is valid only during the callback) and the only work done under the core lock. The inbox is
   drained on the JS thread: **(a)** before a host function that entered the core returns, so the reply
   of a synchronous method and the change-sets of a call, and the initial change-set of `observe`, are
   in the mirror before control returns to JavaScript (that is what `synchronous = true` promises
   `UndraCore`); **(b)** from `CallInvoker::invokeAsync` when a record arrives from another thread, at
   most one invocation pending at a time. A drain swaps the buffer out under the mutex and hands it to
   JavaScript as one `ArrayBuffer` that **owns** it (a `jsi::MutableBuffer`, no second copy); JS reads
   the records as views. One FIFO for every thread is what keeps the core's per-store commit order
   (ADR-020) across threads: a change-set committed by a timer on the core thread and one committed by
   a JS call arrive in the order the core delivered them. Only the outermost drain on the JS thread
   delivers; one that would nest (a port implementation calling the core from inside a drain) leaves
   the records to the running one, so nothing is reordered. The module holds no JSI value: the sink,
   the sync-port function and the frame callback are read from `__undraNative` when needed, so nothing
   outlives the runtime it came from.

5. **Bytes across JSI.** JS to native: a `Uint8Array` is passed as `(ArrayBuffer, byteOffset,
   byteLength)` and the core borrows those bytes for the call, no copy. Native to JS: inbox records as
   above; the `UndraBuf` of `undra_call_sync`, `undra_snapshot` and `undra_stats_json` is wrapped as an
   `ArrayBuffer` whose `MutableBuffer` calls `undra_buf_free` in its destructor (no copy, freed exactly
   once, whenever the JS garbage collector releases it, from any thread, which the ABI allows).
   Handles cross as two 32-bit numbers (`lo`, `hi`), as on the wasm ABI.

6. **Frames.** ADR-031 drains what the core produced on its own at most once per display frame. The
   module provides the frame: `requestFrame()` arms a vsync source (`CADisplayLink` on the main run
   loop on iOS, `AChoreographer` on the JS thread's looper on Android, paused while nothing waits), and
   on the next vsync it posts one `invokeAsync` that runs the waiting drains. `@undra/react-native`'s
   scheduler uses it while `AppState` is `active`, with the browser schedule's 100 ms timer as a
   backstop, and a zero-delay timer while the app is not active (no frames are produced then); without
   a source it falls back to a 16 ms timer. It is passed as the mirror's `schedule`
   (`AttachOptions.mirror.schedule`, already public). `requestAnimationFrame` would land on the same
   vsync today (Context), but only because React Native's timers tick on the display, which its own
   source says may change; the vsync source is the contract Swift (`CADisplayLink`) and Kotlin
   (`Choreographer`) already rely on, at the cost of about 150 lines. Replies, `callSync` and `observe`
   drain immediately, as on every platform (SPEC 11.1).

7. **Ports.** The host registers a `port_cb` for every non-event port of the schema **before**
   `undra_init` (so start-up hooks never race a late registration, SPEC 6), except `Timer`, which keeps
   the core's native timer thread. `Clock`, `Rng` and `Log` are answered **in C++** on whatever thread
   asks (`system_clock`, `steady_clock`, `arc4random_buf`), synchronously, with a `malloc`ed reply;
   `Log` records are also queued for JavaScript (`handler.log`, the app's `LogAdapter`). An async
   method (`Http`, `Kv`, `SecureStore`, `Fs`, an app's async port) is queued and answered with `1`;
   JavaScript runs the registered `PortImpl` and answers with `undra_port_reply`, also "unavailable"
   (status 2) when no implementation is registered. A **synchronous** method implemented in
   JavaScript can only be run on the JS thread: when the core calls it from inside a host function on
   the JS thread it is called there and then (re-entrant JavaScript, the reply copied into `malloc`ed
   memory); from any other thread it is answered "unavailable" and a warning is logged once per port,
   because blocking the core on the JS thread can deadlock against a JS thread that waits for the core
   lock. Standard sync ports are native, so this limits only app-defined sync ports with a JS
   implementation; the generated proxies already treat "unavailable" as a typed outcome (SPEC 5.7).

8. **The schema-hash gate** is checked before `undra_init`, as the Swift host does:
   `undra_abi_version() == 1` and `undra_schema_hash() == expectedSchemaHash`, else
   `UndraSchemaMismatchError` / `UndraTransportError("handshake")` and the core is never started.

9. **Panic containment and no unwinding (R6).** The core contains its own panics (status 2,
   unchanged). On the C++ side every callback is `noexcept` and catches everything inside (host
   contract 3); every host function converts a C++ failure into a `jsi::JSError` thrown to JavaScript,
   never through the core; a JavaScript exception thrown by the inbox sink or a sync port is caught in
   C++, reported, and the port answered "unavailable". Nothing calls `abort` or `std::terminate` on
   purpose; allocation failure in a callback drops the record and counts it.

10. **Cancellation and streams** are the runtime's: `AbortSignal` sends `Cancel` (`undra_cancel`),
    streams grant credit with `undra_stream_credit` and close with `undra_cancel`, as in every mode.

11. **Lifecycle and reload.** Today there is one core per process (`undra_init` is process-wide), so
    one `UndraCore` per process; a second `start` in the process is refused before it touches the ABI,
    and an Undra host of another language in the same process is not supported. `close()` calls
    `undra_shutdown` from the JS thread (never from a callback; a call from inside a sync port throws).
    A JS reload in development destroys the JS runtime while the process lives on: the TurboModule's
    destructor shuts the core down (so nothing is posted to a dead runtime; pending `invokeAsync`
    closures hold a weak reference and do nothing), and the next `install()` starts a fresh one; a core
    left by a runtime whose module was not destroyed yet is stopped by the next `start`, and one whose
    module is inside `undra_shutdown` on the old JS thread is waited for (at most 5 s) rather than
    reported busy. *Review (2026-10-01):* a runtime whose core was stopped that way never reaches the C
    ABI again: its host functions answer as the ABI does with no core (status 5, ignored, unavailable;
    `callSync` and `snapshot` become `UndraTransportError("closed")`), because the process's core is
    now another runtime's, with its own call ids and port call ids. The module cannot tell a reload
    from a second React Native instance in the process, so a second instance's `loadNative` stops the
    first's core: two instances are unsupported until ADR-044 (`docs/REACT_NATIVE.md`, limits).
    *Transitional (ADR-044):* with the table, the limit becomes one `UndraCore` per core namespace, and
    two cores (or two Undra versions) share a process.

12. **The dev loop is B1/B2's.** A React Native app in development can already use the WebSocket
    remote transport to `undra dev` (`UndraCore.load({ mode: "remote", url })`; Hermes has
    `WebSocket`; the Android emulator reaches the host at `10.0.2.2`). This ADR changes nothing there;
    auto-reconnect and Android dev parity are tracks B1 and B2.

13. **Packaging.** `runtimes/rn/@undra/react-native`: TypeScript sources, `cpp/` (the module, the two
    shims, and a copy of `undra.h` that a test keeps byte-identical to the Swift one), `ios/` (the
    module provider and the frame source), `android/CMakeLists.txt` (a pure C++ dependency: no Gradle
    project), `UndraReactNative.podspec`, `react-native.config.cjs`, a `codegenConfig` and the Babel
    plugin. It lives under `runtimes/rn/`, not `runtimes/ts/`, because `runtimes/` is organised by host
    and this host brings its own native toolchain (CocoaPods, CMake, NDK) and CI job; `runtimes/ts`
    stays Node-only. The core binaries come from `undra build --platform rn`: the iOS and Android
    builds, plus `build/ios/UndraCore.podspec` (the static XCFramework as a vendored pod); the app's
    Podfile points at that pod and its Gradle file packages `build/android/jniLibs`. The pod force-loads
    the XCFramework's slice for the SDK (a pod build setting picks the slice), so a debug core keeps its
    registrations and a clean Xcode build finds the input. *Transitional (ADR-044):* the core is
    prelinked into one object per slice whose entry point pulls every registration; the pod drops
    `-force_load` and vendors `<Namespace>Core.xcframework`, and Android packages `lib<namespace>.so`.
    `@undra/react-native` itself relies on neither. It has no runtime dependency; `react-native` and
    `@undra/runtime` are peers.

## Alternatives considered

* **A WebAssembly polyfill under Hermes.** Rejected: Hermes has no `WebAssembly`; a JavaScript wasm
  interpreter would run the core two orders of magnitude slower than native and still have no threads,
  so async tasks would starve the UI.
* **A TurboModule with a JSON (or base64) bridge per call.** Rejected: per-call serialisation on top of
  the wire format the runtime already speaks, a string allocation per payload, and codegen's types
  have no `ArrayBuffer`, so bytes would be copied at least twice each way. The wire bytes are the API.
* **Two platform TurboModules (Objective-C++ and Kotlin/JNI).** Rejected: two implementations of the
  same callback discipline and inbox, and on Android a JNI crossing per call on top of JSI. One C++
  module is one thing to review for ADR-026 ownership.
* **Expo Modules API.** Considered: Expo modules are Swift and Kotlin over JSI with typed-array support
  and good autolinking, but they pull `expo-modules-core` into every app, still need CocoaPods on iOS,
  and would again be two native implementations. A thin Expo wrapper around this C++ module stays
  possible later.
* **`requestAnimationFrame` as the frame.** Considered after measuring (it fires at the display's 60
  Hz on both platforms today); not chosen, because React Native defines it as a zero-delay timer and
  only its timer implementation makes it a frame (decision 6).
* **Blocking the core on the JS thread for JS sync ports.** Rejected: a JS thread inside
  `undra_call_sync` waits for the core lock while the core thread, holding it, would wait for the JS
  thread: a deadlock that a timeout only converts into a random failure.
* **Registering the ports after `undra_init`** (what the Swift host does). Rejected for the reason SPEC
  6 gives: a start-up hook's first call to a late port is answered "unavailable"; registering first
  costs nothing here, since every port the schema names is known before `start`.

## Consequences

* The wire, the C ABI, the wasm ABI, the schema and the generated bindings are unchanged; the
  TypeScript runtime gains no code. One new transport (`NativeTransport`), one new package, one new
  `undra build` target (`rn`). Two TypeScript contract scenarios lost an assumption of `wasm-main`
  (S03 accepts the `native` mode; S12 waits for a request a core thread makes, as the Swift and Kotlin
  columns do); the TypeScript column is still 18/18.
* **What is tested where.** Node cannot load the module, so three layers are tested three ways:
  * the C++ host (`cpp/UndraHost`, both shims) against the real playground core on the Mac, under
    AddressSanitizer and UndefinedBehaviorSanitizer (`cpp/test/run.sh`, 14 checks per shim): the inbox,
    the wake rule, commit order across threads, the ports, `malloc`ed replies, every `UndraBuf` freed
    once, shutdown with a call in flight, a second core refused, start after shutdown, a start while
    another host is shutting down on another thread (a reload); `UndraJsi.cpp` is compiled against
    React Native's headers there too (it runs only on a device);
  * the JavaScript (`NativeTransport`, the scheduler, the polyfills, `loadNative`) with a fake module
    (39 tests), and **the contract scenarios S01 to S18 through `NativeTransport`** over `WasmNative`,
    a stand-in of the module with its inbox, drain and port rules over the playground's wasm core:
    17 pass; S17 is app-tested, because native cores contain panics and a wasm stand-in traps;
  * what only a device can show (JSI, Hermes, `invokeAsync`, the vsync sources, both builds, native
    panic containment) by the playground's React Native app: ten on-device checks (RN01 to RN10: sync
    and async calls, typed errors, panic containment, cancellation, a stream with backpressure,
    read-your-writes, a keyed patch on 10,000 rows, the native Clock and Timer, the schema gate) pass on
    the iPhone 17 Pro simulator and on the Android emulator; on Android, a JavaScript reload in the
    middle of the benchmarks (a debug build on Metro) tore the core down with the runtime and the next
    `loadNative` started a fresh one in the same process (10/10 again). On iOS (review, 2026-10-01: a
    debug build on Metro, reloads through Metro's `/reload`) three reloads, one in the middle of the
    benchmarks, gave four runtimes in the same process (one pid), 10/10 each. After the review's fixes and
    the merge of `main` (2026-10-01): the same on Android, with 32 runtimes in three runs and 66 more
    with a counter on the frame source (`.10x/decisions/sde/react-native.md`).
  * CI: the first two layers and the typecheck run on every push and pull request (`ci.yml`, "React
    Native (host + model)", Linux); the device layer is `scripts/rn-device-checks.sh`, run by
    `.github/workflows/rn-devices.yml` on demand, weekly and on pull requests that touch the package or
    the app (a macOS job on the iPhone simulator, a Linux job on an x86_64 emulator).
* **Measurements** (release core, release app with Hermes bytecode; Apple M-series Mac, iPhone 17 Pro
  simulator on iOS 26.5 and an arm64 Android 15 emulator with 2 GB and a software GPU, on a machine
  shared with other agents' builds and emulators; medians, the range over three runs each, the Android
  emulator's widest because of that load):

  | Row | iOS simulator | Android emulator |
  |---|---|---|
  | JSI host function, the floor | 17 to 23 ns | 18 to 32 ns |
  | Sync call, prebuilt payload straight to `__undraNative.callSync` (JSI, the core, the reply `ArrayBuffer`) | 0.19 to 0.23 us | 0.36 to 0.71 us |
  | Sync call through `UndraCore.callSync` (`Bench.bench_add`) | 5.7 to 8.0 us | 6.2 to 18 us |
  | of which `encodeCall` (a new `UndraWriter` per call, in Hermes) | 4.1 to 4.2 us | 4.6 to 12 us |
  | `await bench.benchAdd(1, 2)` (the generated method: call, inbox reply, promise) | 15 to 20 us | 17 to 44 us |
  | 1 KB record round trip, `callSync`, decoded | 8.9 to 12.4 us | 9.9 to 23 us |
  | 1 KB record round trip, `await bench.benchEchoBytes` | 18 to 27 us | 20 to 43 us |
  | 1,667 one-op keyed patches per frame on 10,000 rows: the one drain (mirror) | 5.3 to 6.4 ms | 5.4 to 12 ms |
  | the same 1,667 change-sets parsed on arrival (`mirror.enqueue`) | 7.8 to 9.4 ms | 8.2 to 19.5 ms |
  | 1,667 `BigList.updateAt` calls through JSI in one turn, then their one drain | 39 to 40 ms (23 to 24 us a call), drain 6.2 to 6.5 ms | 41 to 100 ms (25 to 60 us a call), drain 7.5 to 15 ms |
  | Drain interval under a 10,000/s firehose the core generates (Stress) | 16.66 ms, 123 drains in 2.1 s, 83x merged | 16.7 to 17.1 ms, 114 to 120 drains, 85 to 90x merged |
  | `requestAnimationFrame` interval | 16.67 ms | 16.6 to 17.0 ms |

  The boundary itself is cheap: about 0.2 us for a synchronous call through JSI and the core on the
  simulator, against 50 ns for the C ABI alone on the Mac (ADR-028). What a React Native app pays per
  call is the TypeScript runtime's JavaScript under Hermes, which interprets bytecode: building the
  call payload alone is 4 us (a fresh `UndraWriter` and a `bigint` handle per call). The mirror's
  per-frame work at 100,000 patches a second is 13 to 16 ms on the simulator against 0.3 to 0.5 ms on
  V8 (ADR-031), so on React Native that rate is past the frame budget; 10,000 a second is not. Both are
  costs of `@undra/runtime`'s JavaScript under an interpreter, a follow-up for that package (a reused
  writer in `callSync`, a cheaper change-set parse; the binding call path is Amendment D's E4), not of
  this host.
* Limits, documented in `docs/REACT_NATIVE.md`: one core per process until ADR-044; JS-implemented
  synchronous ports are reachable only from calls made on the JS thread; `Clock`, `Rng` and `Timer`
  are native and not overridable from JavaScript; there is no default `Kv`/`SecureStore`/`Fs` adapter in
  React Native's core (the app supplies one, as the playground does) and no default `Connectivity`
  source; the app adds the package's Babel plugin and imports the package before the bindings.

## Amendment B (2026-10-01): the standard ports come with the package (G1b, `wt/rn-adapters`)

Status: proposed, written before the code (R11: it adds native port callbacks and threads to the host of decision 7
and a Java half to the Android package of decision 13). **No wire, C ABI, wasm ABI, schema or generated-code
change** for `@undra/react-native`'s users; the playground core gains a module (below), which changes the playground's
schema and its checked-in bindings, not any public shape. This amendment removes the limit "there is no default
`Kv`/`SecureStore`/`Fs` adapter in React Native's core (the app supplies one) and no default `Connectivity` source":
`loadNative` now gives every standard port of SPEC 8 a platform implementation, as `Adapters.platformDefault` (Swift),
`AndroidPlatformDefaults.install` (Kotlin) and `browserAdapters()` (web) do, and an app overrides any of them the way
it already overrides `http` or `lifecycle`.

### What React Native 0.87 offers, and what it does not

Read from `react-native@0.87.1` (the playground's `node_modules`): JavaScript has `fetch` (`whatwg-fetch` over the
`Networking` module: `NSURLSession` on iOS, OkHttp on Android; binary bodies cross as base64; `URL` is a lenient
regex shim that never throws) and `AppState`. It has **no** file system, no key-value store, no keychain or keystore
and no network state (`NetInfo` left core years ago): `react-native-fs`, MMKV, AsyncStorage, `react-native-keychain`
and `@react-native-community/netinfo` are community modules, and `expo-secure-store` needs `expo-modules-core`. The
package takes no runtime dependency (decision 13), so four ports cannot be written in TypeScript at all.

### The options

* **(a) Reuse the platform runtimes' adapters natively.** iOS: the Swift `UndraRuntime` adapters inside the pod.
  Android: depend on `android-adapters` and call its `PortImpl`s. Rejected:
  * neither is distributable to an npm package: the Swift runtime is a SwiftPM package with no podspec (a pod
    cannot depend on it; React Native's `spm_dependency` helper is experimental and rewrites the app's Xcode
    project), and `dev.undra:android-adapters` / `dev.undra:runtime` are on no Maven repository (Maven Central is
    an open follow-up), while an npm package cannot reference a Gradle project outside itself;
  * both are adapters *of their runtime*: Swift's take an `UndraCore` (`makePortImpl(core:)`), Kotlin's are
    `suspend` `PortImpl`s over `kotlinx-coroutines` and the Kotlin `UndraCore`. Using them puts a second Undra
    host's classes (the whole Swift runtime, or `:runtime` plus coroutines) into an app whose core is driven from
    C++, for a few hundred lines that are actually needed;
  * Swift in the pod costs a Swift target next to the C++ (module maps, an `@objc` facade, since Swift cannot be
    called from C++ without turning on Swift/C++ interop for React Native's headers, and Swift 6 strict concurrency
    across that facade), and a second language to review against ADR-026's ownership rules. The Swift `FsAdapter`
    also does not refuse symbolic links (SPEC 8 says it must), which reuse would inherit.
* **(b) Everything in TypeScript over React Native's APIs.** Rejected: impossible without community modules for
  `Kv`, `SecureStore`, `Fs` and `Connectivity` (above).
* **(c) Hybrid (chosen).** TypeScript where React Native has the API (`Http` over `fetch`, `Lifecycle` over
  `AppState`); native where it has none, with **one portable C++ implementation of `Kv` and `Fs` for both
  platforms**, and per platform only what C++ cannot reach: the Keychain and `NWPathMonitor` (Objective-C++ over
  the C APIs of Security and Network.framework, no Swift), the Keystore and `ConnectivityManager` (a small Java
  library, no Kotlin, no dependency). Sharing with the platform runtimes is by **format and test**, not by linking:
  the same directories, entry layout, file names, Keychain items, Keystore alias and sealed layout as the Swift
  adapters on iOS and `android-adapters` on Android, so a value one shell wrote is read by the other.

### Decisions

B1. **Which port is implemented where** (the tests are in B10):

| Port | Implemented in | iOS | Android |
|---|---|---|---|
| `Kv` | C++ (`cpp/UndraStores.cpp`), shared | files in `<Application Support>/<bundle id>/Undra/kv`, named `<fnv1a64>-<fnv1a32>` of the key (the Swift `KvAdapter`'s directory and names) | files in `<filesDir>/undra/kv`, named by the SHA-256 of the key (`android-adapters`' `AndroidKvAdapter`) |
| `SecureStore` | iOS: Objective-C++ (`ios/UndraPlatformApple.mm`); Android: the C++ store over values sealed in Java | Keychain generic passwords, service `dev.undra.securestore`, account = key, `AfterFirstUnlockThisDeviceOnly` (the Swift `SecureStoreAdapter`'s items) | AES-256-GCM under the `AndroidKeyStore` key `dev.undra.securestore`, `format, iv, ciphertext+tag` with `undra.secure:<key>` as AAD, files in `<noBackupFilesDir>/undra/secure` (`AndroidSecureStoreAdapter`'s alias, layout and directory) |
| `Fs` | C++, shared | `<Application Support>/<bundle id>/Undra/fs` | `<filesDir>/undra/fs` |
| `Connectivity` | native event source | `nw_path_monitor` (Network.framework's C API) on its own queue | `ConnectivityManager.registerDefaultNetworkCallback` on a handler thread (Java), into C++ over JNI |
| `Http` | TypeScript (`reactNativeHttp()`) | React Native's `fetch` (`NSURLSession`) | React Native's `fetch` (OkHttp) |
| `Lifecycle` | TypeScript (`appStateLifecycle()`, unchanged) | `AppState` | `AppState` (React Native reports no `inactive` on Android) |
| `Clock`, `Rng`, `Log` | C++, unchanged (decision 7) | | |
| `Timer` | the core's own timer thread, unchanged | | |

B2. **Native defaults never cross into JavaScript.** For `Kv`, `SecureStore` and `Fs` the host registers a native
`port_cb` (before `undra_init`, as decision 7 requires) that copies the arguments into a job, hands it to that
port's own serial worker thread and returns 1; the worker does the I/O, encodes the reply and calls
`undra_port_reply` (allowed from any thread, host contract 4). The JS thread is never involved: these ports work
while JavaScript is busy, and a query's persistence costs no JSI crossing. One worker per port, serial: a slow
Keystore call never holds up `Kv`, and two writes of one key from the core land in order. `Connectivity` reports
are sent with `undra_event` from the monitor's own thread (a host thread, never a callback), deduplicated (an
identical consecutive state is reported once), the current state first.

B3. **Lifetime: nothing outlives its core.** `Host::shutdown` runs `undra_shutdown`, then stops the event source
(`nw_path_monitor_cancel` followed by a barrier on its queue; Java's `unregisterNetworkCallback` and a joined
handler thread), then stops and joins the workers (a job already running finishes, its `undra_port_reply` reaches
no runtime and is ignored; queued jobs are dropped), and only then releases the process's core slot (decision 11).
So a reply or an event of a stopped core can never reach the next one, whose port call ids restart from 1.

B4. **`Fs` rules (SPEC 8), stricter where SPEC allows.** Paths are `/`-separated and relative to the root; a
leading `/`, empty and `.` components are ignored; **any** `..` component is `Denied` (as Swift and the web do;
Kotlin normalises first); a NUL byte is `Io`. Every component is opened with `openat(..., O_NOFOLLOW)` from the
root's descriptor, so **any** symbolic link on the path is `Denied`, whether it points in or out (the core cannot
create links; a link inside the root is not a supported layout); `delete` removes a link without following it, and
a directory with everything in it (`unlinkat` walk, links never followed); the root itself is `Denied` for
`delete` and `Io` for `read`/`write` (it is a directory). `write` creates missing parents and is atomic (a
temporary file in the same directory, `fsync`, `renameat`); `list` returns the names of one directory sorted by
byte order, without this adapter's temporary files. Errors: `ENOENT`/`ENOTDIR` are `NotFound`, `EACCES`/`EPERM`/`ELOOP`
are `Denied`, anything else `Io` with `strerror`.

B5. **`Kv` and `SecureStore` files.** One file per key holding `u32 key length, key, value` (both runtimes' layout);
a write goes to a temporary file in the same directory, is `fsync`ed and renamed over the entry, mode `0600`, so a
killed process leaves the old or the new value; `get` checks the stored key (a name collision is never another
key's value); `list` reads only each file's key, skips temporary and dot files and anything that is not an entry,
filters by prefix and sorts by byte order (code-point order; the runtimes sort UTF-16 or Swift strings, identical
for keys in the Basic Multilingual Plane). SHA-256 and FNV-1a are implemented in the C++ (checked against published
vectors) because neither platform offers one to C++ that both share.

B6. **Errors (ADR-032, ADR-025).** `Fs` failures are its typed `FsError` (status 1). `Kv` and `SecureStore` have no
error channel: a failing disk, Keychain or Keystore answers "unavailable" (status 2) and logs one record (target
`undra::react-native`), never "missing" for a value that exists but cannot be read. `Http` failures are its typed
`HttpError`: no connection, a refused connection or a DNS failure (`TypeError: Network request failed`) is
`Network`, never `Unavailable`; the request's `timeoutMs` (covering the body) is `Timeout`; a cancellation is
`Cancelled`; a URL that is not `http(s)://host...` is `InvalidUrl` (checked by the adapter, since React Native's
`URL` accepts anything).

B7. **Choosing native or JavaScript, per port.** `loadNative` asks the module which defaults the platform has
(`native.platformDefaults()`) and passes the native ones to `start`: a port is native when the app did not override
it (`adapters.kv === undefined` and no `ports[<id>]`); a value in `adapters` or `ports` is registered as a
JavaScript port exactly as today; `null` removes it (unavailable). `adapters.connectivity` likewise replaces or
removes the native source. `core.registerPort` after `loadNative` cannot replace a port the module answers
natively (the registration is the module's, made before `undra_init`): an app that replaces a default passes it to
`loadNative` (documented). `Http` and `Lifecycle` are ordinary JavaScript adapters and replaceable either way.

B8. **Android packaging.** The package gains an Android library (`android/build.gradle`, Java, namespace
`dev.undra.reactnative`): `UndraPlatform` (the app's directories, the Keystore seal, the default-network callback),
the context captured by a `ContentProvider` (`UndraContextProvider`, the same start-up hook AndroidX Startup uses;
an app that removes it calls `UndraPlatform.install(context)`), and an empty `UndraReactNativePackage`, because
autolinking links a library with a Gradle project only when it has a `ReactPackage`. The C++ module stays the C++
TurboModule of decision 1, still autolinked through `cxxModuleCMakeListsPath` (React Native's Gradle plugin
registers C++ modules of libraries with a Gradle project too); the library applies `com.facebook.react` so the
codegen of `codegenConfig` runs for it, as for any library. The C++ reaches Java through JNI only: the class is
loaded through the JS thread's context class loader in `install()`, methods are looked up once, the native
callback of the network monitor is bound with `RegisterNatives` (nothing depends on exported `Java_` symbols, which
a static library inside `libappmodules.so` would drop), and the `SecureStore` worker attaches itself to the VM once.
The manifest declares `ACCESS_NETWORK_STATE` (and `INTERNET`); consumer R8 rules keep the classes JNI names. iOS
gains `ios/UndraPlatformApple.mm` and the `Security` and `Network` frameworks in the podspec; no Swift.

B9. **One instance per process, paths per process.** The defaults belong to the process's one running host
(decision 11). Two cores in one process (ADR-044) would share the directories and the Keychain service; when the
`abi-table` piece lands, a core's namespace becomes a subdirectory and a service suffix, except for the default
namespace, whose layout stays this one (the native shells' layout). *Transitional (ADR-044).*

B10. **Tests, per layer** (the deterministic story of the other runtimes: Rust fakes for core logic, the real
platform on the device):

| Port | C++ host test (`cpp/test/run.sh`, real core, ASan + UBSan) | `npm test` (fake module, mocked React Native) | Device (`scripts/rn-device-checks.sh`, iPhone 17 Pro simulator and the `undra-rn` emulator) |
|---|---|---|---|
| `Kv` | entry format and both namings against fixed vectors; atomic write; collisions; list; through the core (`kv_put`/`kv_get` of the playground core) | native when not overridden, JavaScript when overridden, absent with `null` | a round trip through the core; a value written before the app was killed and read after the relaunch |
| `SecureStore` | the store over a test sealer; unavailable on a sealer failure | as `Kv` | a round trip through the core; the value's marker is **not** found in the app's files while a `Kv` marker written next to it **is** (the scan works) |
| `Fs` | `..`, symbolic links in and out, the root, NUL, recursive delete, atomic write, sorted list, typed errors; through the core | as `Kv` | write, read, list, delete inside the root; `../` refused with `FsError.Denied` through the core |
| `Http` | (TypeScript) | `reactNativeHttp()` over a mocked `fetch`: status, headers, body, `InvalidUrl`, `Timeout`, `Cancelled`, `Network` | a GET through the core to a loopback server the script runs (`adb reverse` on Android), with an app override that adds a header (the sample of overriding); a refused port is `HttpError.Network` |
| `Connectivity` | the event encoding, deduplication, stop before the slot is released (scripted source) | the source is native unless overridden | the core received a report; on Android the core sees `online = false` in airplane mode and `true` after it |
| `Lifecycle` | (TypeScript) | `appStateLifecycle()` (unchanged tests) | the core sees `background` after Home (Android) or another app (iOS) and `active` after the relaunch |

The Java's pure parts (the seal with a software key against a vector computed with Node's AES-GCM, the network
classification) are unit-tested with `javac` and the JDK (`android/test/run.sh`). The core's side of every check
is a new playground module, `platform` (`kv_*`, `secret_*`, `file_*`, `http_get`, and a `Device` store that shows
the last `Connectivity` and `Lifecycle` reports the core received since it started, through an `InitHook`), tested
with `undra::ports::fakes` like the rest of the playground core, and usable by the other playground apps.

### Consequences

* An app writes no adapter: `loadNative({ expectedSchemaHash })` gives the core all ten ports. The playground's React
  Native app drops its in-memory `Kv` and keeps one override (an `Http` adapter that adds a header) as the sample.
* `@undra/react-native` is no longer a pure C++ dependency on Android: it has a Gradle library (Java, no
  dependencies) beside the C++ module. On iOS it stays one pod, now linking Security and Network.
* The module grows by four threads per running core at most (three port workers, started on first use, and one
  monitor queue or handler thread), all stopped before the core's slot is released.
* Limits that remain (documented in `docs/REACT_NATIVE.md`): `registerPort` after `loadNative` does not replace a
  native default; `Http` runs on the JS thread's `fetch` (bodies cross React Native's bridge as base64; a native
  `Http` is a follow-up if a measurement asks for one); Android reports no `inactive`; the iOS simulator has no
  airplane mode, so the `Connectivity` flip is checked on Android only.
