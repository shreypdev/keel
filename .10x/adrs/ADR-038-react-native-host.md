# ADR-038: React Native is a fourth host of the C ABI

Status: proposed (2026-10-01, `wt/react-native`, v1.2 bet G1, founder's yes in Amendment A of
`.10x/specs/2026-10-01-v1x-default-choice-design.md`). Touches SPEC 11 (a fourth platform runtime)
and SPEC 17 (a new package, `@undra/react-native`, on top of `@undra/runtime`). **No wire change, no
C ABI change, no wasm ABI change, no schema change, no generated code change**: the React Native host
calls the 19 functions of `undra.h` exactly as the Swift host does, and its JavaScript side reuses the
TypeScript runtime's `UndraCore`, `Mirror`, codecs and the generated TypeScript bindings unchanged.
Constitution R11: a new host is a boundary consumer with its own threading model, so it is decided
here before the code.

## Context

Teams that keep React Native for their UI cannot put Undra under it today:

* **The TypeScript runtime needs WebAssembly or a socket.** `@undra/runtime` reaches a core through
  `wasm-main`, `wasm-worker` (both need `WebAssembly`) or `remote` (a WebSocket to `undra dev`).
  Hermes, React Native's engine, has no `WebAssembly` object, and the remote mode is a dev loop, not a
  way to ship.
* **The native core already exists for both phones.** `undra build --platform ios` produces
  `UndraCore.xcframework` (a static `libundra_core.a` per slice) and `--platform android` produces
  `jniLibs/<abi>/libundra_core.so`, with the C ABI of SPEC 6 exported from both (the Android library
  also carries the JNI shim of SPEC 6.1, which a C caller ignores). A React Native app on those devices
  can link the same library the SwiftUI and Compose apps link.
* **React Native 0.87** (current stable, 2026-10) is New Architecture only: bridgeless, JSI, C++
  TurboModules, Hermes. Native code reaches JavaScript only through `jsi::Runtime` on the JS thread and
  through `CallInvoker::invokeAsync` from any other thread. Two facts of 0.87 matter below and were
  read from its sources: `requestAnimationFrame` in bridgeless mode is implemented as `setTimeout(0)`
  (`ReactCommon/react/runtime/TimerManager.cpp`: "the same as setTimeout(0)", and a zero-delay timer
  is called immediately by `RCTTiming` on iOS and `JavaTimerManager` on Android), and a library can be
  a **pure C++ TurboModule** on both platforms: iOS through `codegenConfig.ios.modules` (a class
  conforming to `RCTModuleProvider`), Android through `react-native.config.js`
  (`cxxModuleCMakeListsPath`, `cxxModuleCMakeListsModuleName`, `cxxModuleHeaderName`; the app runs the
  library's codegen, no Gradle project needed).

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
   `schemaHash`, `schemaJson` and `requestFrame`). The TurboModule exists for discovery, autolinking
   and the `CallInvoker`; the hot path is a direct JSI host function call, no TurboModule method
   dispatch and no codegen types (codegen has no `ArrayBuffer`). The same C++ (`cpp/`) builds on iOS
   (CocoaPods, statically linked against `UndraCore.xcframework`) and Android (the app's CMake through
   autolinking, `dlopen("libundra_core.so")` from the APK). The only platform code is the module
   provider class on iOS and the frame source (decision 6).

2. **The JavaScript side is a `Transport`.** `NativeTransport` implements the TypeScript runtime's
   `Transport` interface (`runtimes/ts/@undra/runtime/src/transport/transport.ts`) over
   `__undraNative`, with `synchronous = true` and `callSync`, like `wasm-main`. `loadNative(options)` is
   `UndraCore.attach(new NativeTransport(..), options)` with React Native defaults for the adapters and
   the mirror's schedule, so `UndraCore`, `Mirror` (ADR-031 rules included), the codecs, the generated
   bindings and `@undra/runtime/react` (`useUndra`, `useSignal`) work unchanged. Nothing in
   `@undra/runtime` changes.

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
   the records to the running one, so nothing is reordered.

5. **Bytes across JSI.** JS to native: a `Uint8Array` is passed as `(ArrayBuffer, byteOffset,
   byteLength)` and the core borrows those bytes for the call, no copy. Native to JS: inbox records as
   above; the `UndraBuf` of `undra_call_sync`, `undra_snapshot` and `undra_stats_json` is wrapped as an
   `ArrayBuffer` whose `MutableBuffer` calls `undra_buf_free` in its destructor (no copy, freed exactly
   once, whenever the JS garbage collector releases it, from any thread, which the ABI allows).
   Handles cross as two 32-bit numbers (`lo`, `hi`), as on the wasm ABI.

6. **Frames.** ADR-031 drains what the core produced on its own at most once per display frame, and
   React Native's `requestAnimationFrame` is not a frame in 0.87 (context). The module therefore
   provides the frame: `requestFrame()` arms a vsync source (`CADisplayLink` on the main run loop on
   iOS, `AChoreographer` on the JS thread's looper on Android, paused while nothing waits), and on the
   next vsync it posts one `invokeAsync` that runs the waiting drains. `@undra/react-native`'s
   `scheduleFrame` uses it while `AppState` is `active`, with the browser schedule's 100 ms timer as a
   backstop, and a zero-delay timer while the app is not active (no frames are produced then). It is
   passed as the mirror's `schedule` (`AttachOptions.mirror.schedule`, already public). Replies,
   `callSync` and `observe` drain immediately, as on every platform (SPEC 11.1).

7. **Ports.** The host registers a `port_cb` for every non-event port of the schema **before**
   `undra_init` (so start-up hooks never race a late registration, SPEC 6), except `Timer`, which keeps
   the core's native timer thread. `Clock`, `Rng` and `Log` are answered **in C++** on whatever thread
   asks (`clock_gettime`, `arc4random_buf`), synchronously, with a `malloc`ed reply; `Log` records are
   also queued for JavaScript (`handler.log`, the app's `LogAdapter`). An async method (`Http`, `Kv`,
   `SecureStore`, `Fs`, an app's async port) is queued and answered with `1`; JavaScript runs the
   registered `PortImpl` and answers with `undra_port_reply`, also "unavailable" (status 2) when no
   implementation is registered. A **synchronous** method implemented in JavaScript can only be run on
   the JS thread: when the core calls it from inside a host function on the JS thread it is called
   there and then (re-entrant JavaScript, the reply copied into `malloc`ed memory); from any other
   thread it is answered "unavailable" and a warning is logged once per port, because blocking the core
   on the JS thread can deadlock against a JS thread that waits for the core lock. Standard sync ports
   are native, so this limits only app-defined sync ports with a JS implementation; the generated
   proxies already treat "unavailable" as a typed outcome (SPEC 5.7).

8. **The schema-hash gate** is checked before `undra_init`, as the Swift host does:
   `undra_abi_version() == 1` and `undra_schema_hash() == expectedSchemaHash`, else
   `UndraSchemaMismatchError` / `UndraTransportError("handshake")` and the core is never started.

9. **Panic containment and no unwinding (R6).** The core contains its own panics (status 2,
   unchanged). On the C++ side every callback is `noexcept` and catches everything inside (host
   contract 3); every host function converts a C++ failure into a `jsi::JSError` thrown to JavaScript,
   never through the core; a JavaScript exception thrown by the inbox sink or a sync port is caught in
   C++, reported, and the port answered "unavailable". Nothing calls `abort` or `std::terminate` on
   purpose; allocation failure in a callback drops the record and logs.

10. **Cancellation and streams** are the runtime's: `AbortSignal` sends `Cancel` (`undra_cancel`),
    streams grant credit with `undra_stream_credit` and close with `undra_cancel`, as in every mode.

11. **Lifecycle and reload.** There is one core per process (`undra_init` is process-wide), so one
    `UndraCore` per process. `close()` calls `undra_shutdown` from the JS thread (never from a callback;
    a call from inside a sync port throws). A JS reload in development destroys the JS runtime while the
    process lives on: the TurboModule's destructor shuts the core down (so nothing is posted to a dead
    runtime; pending `invokeAsync` closures hold a weak reference and do nothing) and the next
    `install()` starts a fresh one. JSI values that belonged to a destroyed runtime are released
    without being touched (a small one-off leak per dev reload, never in a shipped app).

12. **The dev loop is B1/B2's.** A React Native app in development can already use the WebSocket
    remote transport to `undra dev` (`UndraCore.load({ mode: "remote", url })`; Hermes has
    `WebSocket`; the Android emulator reaches the host at `10.0.2.2`). This ADR changes nothing there;
    auto-reconnect and Android dev parity are tracks B1 and B2.

13. **Packaging.** `runtimes/rn/@undra/react-native`: TypeScript sources, `cpp/` (the module and a
    copy of `undra.h` that a test keeps byte-identical to the Swift one), `ios/` (the module provider
    and the frame source), `android/CMakeLists.txt` (a pure C++ dependency: no Gradle project),
    `UndraReactNative.podspec`, `react-native.config.js` and a `codegenConfig`. It lives under
    `runtimes/rn/`, not `runtimes/ts/`, because `runtimes/` is organised by host and this host brings
    its own native toolchain (CocoaPods, CMake, NDK) and CI job; `runtimes/ts` stays Node-only. The
    core binaries come from `undra build --platform rn`: the iOS and Android builds, plus
    `build/ios/UndraCore.podspec` (the static XCFramework as a vendored pod, linked with `-force_load`
    so a debug core keeps its registrations); the app's Podfile points at that pod and its Gradle file
    packages `build/android/jniLibs`. `@undra/react-native` has no runtime dependency; `react-native` and
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
* **`requestAnimationFrame` as the frame.** Rejected for 0.87 (context): it is a zero-delay timer, so
  drains would follow every macrotask and the "once per frame" bound of ADR-031 would not hold under a
  firehose.
* **Blocking the core on the JS thread for JS sync ports.** Rejected: a JS thread inside
  `undra_call_sync` waits for the core lock while the core thread, holding it, would wait for the JS
  thread: a deadlock that a timeout only converts into a random failure.

## Consequences

* The wire, the C ABI, the wasm ABI, the schema and the generated bindings are unchanged; the
  TypeScript runtime gains no code. One new transport (`NativeTransport`) and one new package.
* The contract scenarios S01 to S18 run against the RN transport's JavaScript in Node over a
  WebAssembly stand-in of the native module (the same inbox, drain and sync-port rules; the real module
  cannot load in Node), and the C++ module's ownership rules run against the real native core on the
  host; what only a device can show (JSI, Hermes, `invokeAsync`, the vsync source, the two builds) is
  shown by the playground's React Native app on the iOS simulator and the Android emulator.
* Measurements (filled in by the implementation, `docs/REACT_NATIVE.md` has the method): a synchronous
  call round trip through JSI, a 1 KB record round trip, and 1,667 keyed patches per frame on a 10,000
  row list, on the iOS simulator and the Android emulator.
* Limits, documented in `docs/REACT_NATIVE.md`: one core per process; JS-implemented synchronous ports
  are reachable only from calls made on the JS thread; `Clock`, `Rng` and `Timer` are native and not
  overridable from JavaScript; there is no default `Kv`/`SecureStore`/`Fs` adapter in React Native's
  core (the app supplies one, as the playground does) and no default `Connectivity` source.
