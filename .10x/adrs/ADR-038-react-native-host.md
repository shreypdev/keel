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
