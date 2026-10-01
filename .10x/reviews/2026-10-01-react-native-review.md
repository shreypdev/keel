# React Native host (ADR-038, v1.2 bet G1) — adversarial review

**Date:** 2026-10-01 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/react-native` at `4e8179e` (`main` merged at `646d0a0`) · **Read:** `CLAUDE.md` (R2, R6, R12), ADR-038 (with its
ADR-044 transitional notes), ADR-026, ADR-028, ADR-031, ADR-044, `docs/SPEC.md` 6, 11.2, 17.4, `docs/REACT_NATIVE.md`,
`.10x/decisions/{architect,sde}/react-native.md`, the ADR's measurements, `cpp/*` (host, JSI binding, TurboModule, both
shims, both frame sources), `ios/*.mm`, `cpp/test/*`, `src/*.ts`, `test/**`, `crates/undra-cli/src/builds/rn.rs`,
`examples/playground/rn`, the S03/S12 edits, and `crates/undra-ffi` (`native.rs`, `api.rs`, `session.rs`) for what the
host relies on · **Fixes:** `1e695e7`.

## Verdict

The host is sound where it matters most: every buffer has one owner and is freed once, no JSI object is touched off
the JS thread, no lock is held across a call into the core or into JavaScript, nothing unwinds into the core, and a
JS reload tears the core down and starts a fresh one in the same process on both platforms (iOS proven in this review:
three reloads, one in the middle of the benchmarks, four runtimes in one pid, 10/10 each). Two Medium findings were
real and are fixed here, both on the "a second thing touches the one module" paths the design explicitly anticipates:
a second `NativeTransport.start` that failed (the documented "another core is running" case) took the sink away from
the running core, after which every batch was dropped and every await hung (M1); and a runtime whose core was stopped
by a reloaded runtime's `start` kept calling straight into the process's core, which by then belonged to the new
runtime, with colliding call ids and port call ids (M2). One Low was a real race found by a test that fails on the
original code at its first round: on iOS the reloaded runtime's `start` can run while the old runtime's module is
inside `undra_shutdown` on the old JS thread, and got "busy" (L1). Nothing High; nothing High or Medium is open.

**May a React Native team put Undra under their app today?** Yes, as an early adopter, within stated limits: React
Native 0.87 with the New Architecture on Hermes (the only version run); **one** React Native instance per process and no
Swift or Kotlin Undra host beside it (until ADR-044); core-driven update rates around 10,000 keyed patches a second or
below, because at 100,000 a second `@undra/runtime`'s JavaScript under Hermes needs 13 to 16 ms of a 16.7 ms frame
(E4 open; the boundary itself is about 0.2 us a call); the app supplies its own `Kv`, `SecureStore`, `Fs` and
`Connectivity` adapters (G1b open); synchronous ports implemented in JavaScript are reachable only from calls made on
the JS thread. Two operational cautions: nothing in CI builds or runs this package or the app yet, so a team should pin
the Undra version it verified on its own devices; and ADR-038 is still *proposed* and transitional for ADR-044, whose
`abi-table` piece changes the pod (no `-force_load`), the Android library name and `install(namespace)`, a migration
the app will see once.

## Findings

| # | Sev | Where (at `4e8179e`) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `src/transport.ts:122-130`, `:234-238`; `cpp/UndraJsi.cpp:288-291` | `#start` overwrote `native.sink` / `native.portSync` before `native.start`, and on a non-zero code (`0x102` already started, `0x100` busy) `#detach()` set both to `undefined`; `close()` of a transport that never started did the same. If a core was running, its transport's sink was gone: `Binding::drain` then drops every batch ("no transport attached"), so every pending call, stream and port reply of the running app hangs. Reached by the documented path (troubleshooting: "`loadNative` (or `NativeTransport.start`) was called while a core is open"), e.g. an app that attaches a `NativeTransport` itself and then calls `loadNative`. | **Fixed**: the transport installs its own named closures, puts back what was there on a failed start, and `#detach` removes only its own. Test "a second start that fails leaves the running core's sink and sync port in place" (fails on the original). |
| M2 | Medium | `cpp/UndraJsi.cpp:397-401` (`enter`), `:337-339` (takeover), `:496-498` (`snapshot`) | A binding with no running host of its own called the C ABI anyway ("the C ABI answers softly"). That is only true when no core runs. After a reload's takeover (`owner->detach()` in the new runtime's `start`), the old runtime's transport is still "started" and its JavaScript runs until the old JS thread processes the teardown: its `call`s reached the **new** runtime's core and their replies landed in the new runtime's inbox under call ids the new runtime also allocates from 1 (settling the wrong promise); an in-flight `fetch` finishing in the old runtime sent a `PortReply` whose port call id could answer the new core's pending port call of the same number; `callSync`, `restore` mutated, `snapshot` read the new runtime's stores. Also: ADR-038 decision 11 said a second `start` "is refused" while the code stops the first runtime's core (it cannot tell a second React Native instance from a reload). | **Fixed**: every entry that reaches the core answers as the C ABI does with no core, without calling it (`call` 5, void entries ignored, `restore` 6, `callSync`/`snapshot` `undefined`, which `NativeTransport` turns into `UndraTransportError("closed")`). ADR-038 decision 11 and `REACT_NATIVE.md` limits state the two-instance behaviour. TS test for the typed error; the C++ path runs on the device (compiled in `run.sh`, iOS reload proof below). |
| L1 | Low | `cpp/UndraHost.cpp:118-131`, `:169-180` | One core per process was an atomic `g_running` cleared only after `undra_shutdown` returned. On an iOS reload the old runtime's module is destroyed on the old JS thread while the new runtime starts on its own; when the old `Binding` is already expired (so the takeover cannot run) and its destructor is inside `undra_shutdown`, the new `start` got `0x100` busy and `loadNative` rejected. | **Fixed**: the slot is a mutex-guarded pair (holder, stopping); `start` waits up to 5 s for a holder that is shutting down, refuses a running one at once. Test "a start while another host is shutting down waits for it" (20 rounds; the original fails at round 0 with 256). |
| L2 | Low | `cpp/UndraJsi.h:42-45`, `cpp/UndraJsi.cpp:224-231` | `postDrain` was `noexcept` around `invokeAsync`, which allocates: an exception there is `std::terminate` on a core thread (an abort, R6), and `Host::requestDrain`'s `catch` that resets the wake flag was unreachable. | **Fixed** (not `noexcept`; `requestDrain` catches and re-arms). |
| L3 | Low | `cpp/UndraJsi.cpp:246-253` | `postFrame` (`noexcept`) called `invokeAsync` outside any `try`, from the display link's tick (Objective-C++ on the main run loop) or the choreographer's callback (a C callback): a throw terminates. | **Fixed** (caught; the JS scheduler's 100 ms backstop covers a lost frame). |
| L4 | Low | `cpp/UndraJsi.cpp:231-243` | The posted drain task let a JSI exception out (`getProperty`, `ArrayBuffer` creation) into React Native's runtime scheduler, which reports it as a fatal JS error in a release build. `postFrame`'s task already caught. | **Fixed** (caught). |
| L5 | Low | `cpp/UndraJsi.cpp:436-437`, `:497` | `callSync` and `snapshot` built `make_shared<CoreBuffer>` and the `ArrayBuffer` after the core returned the `UndraBuf`: a throw there (out of memory) leaked it. | **Fixed**: `coreArrayBuffer` frees it if the owner cannot be built; once built, its destructor frees it on any later throw. |
| L6 | Low | `cpp/UndraJsi.cpp:403-409` | Records the core queues on the JS thread inside a host function do not wake (the host function drains before it returns); if `body()` threw, they stayed in the inbox until some later drain. | **Fixed** (a throw posts a drain, then propagates). |
| L7 | Low | `src/frame.ts:59` | Each `nativeFrameScheduler` set `native.frame = run`: a second scheduler on the module (a failed second `loadNative`, an app's own attach) took the vsync from the running mirror, which then drained only on its 100 ms backstop (10 Hz). | **Fixed**: the module's frame fans out to every waiting scheduler. Test (fails on the original). |
| L8 | Low | `cpp/test/run.sh` | `UndraJsi.cpp` (the binding, the drain, `enter`, the buffers) was compiled only by the app builds. | **Fixed**: `run.sh` compiles it with `-Wall -Wextra -Werror` against the playground's React Native 0.87 headers when they are installed. |
| L9 | Low | `examples/playground/rn/android/app/build.gradle` | `createBundleReleaseJsAndAssets` tracks only the app's directory; Metro bundles the runtime, the package and the bindings from outside it, so after editing them the release APK shipped the old JavaScript ("UP-TO-DATE"; hit during this review). | **Fixed** (those directories are task inputs; verified the task reruns after an edit). |
| L10 | Low | `docs/REACT_NATIVE.md` | The 100,000-a-second limit was half a sentence under "per-call cost"; "React Native 0.82 or newer" was claimed with only 0.87 run; nothing on two React Native instances; the busy row did not say the running core is unaffected; nothing on a Release simulator build failing to link `x86_64` against the arm64-only core slice (hit during this review). | **Fixed** (a plain limit with the numbers and E4, 0.87 as the proven version, the two-instance limit, three troubleshooting rows). |
| L11 | Low | `.10x/adrs/ADR-038-react-native-host.md` | Decision 11 contradicted the code (see M2); test counts; iOS reload untested. | **Fixed** (decision 11 amended with a dated review note; 14 checks per shim, 39 TS tests; the iOS reload proof). |
| I1 | Info | `cpp/UndraFrameSource.cpp:58` | Each `AChoreographer_postFrameCallback` carries a `new std::weak_ptr<State>` deleted in the callback; a callback still pending when the JS thread's looper quits (a reload) is never run, so one small allocation (and `State`'s control block) leaks per reload. Dev-only, bounded by reloads. | Open (note) |
| I2 | Info | `cpp/UndraHost.cpp` `start` | Ports are registered before `undra_init` (ADR-038 decision 7). If another embedder of another language already runs a core in the process, `undra_init` returns 1 and the cleanup unregisters port ids that were that embedder's. Unsupported configuration (one core per process, documented); ADR-044 removes it. | Open (note) |
| I3 | Info | `cpp/UndraJsi.cpp` `snapshot` | Not refused from inside a JS sync port (inside a core callback), which the header forbids; the core tolerates it (`Runtime::snapshot` reads without the lock it cannot take). | Open (note) |
| I4 | Info | `src/transport.ts` `#deliver` | Records are views into the batch's `ArrayBuffer`; a view the mirror keeps (a queued change-set) keeps the whole batch (its capacity grows by doubling from 4 KB) alive until it is applied. Bounded by the mirror's backlog bounds. | Open (note) |
| I5 | Info | React Native | `RuntimeSchedulerCallInvoker` holds the scheduler weakly; a core thread inside `invokeAsync` at the moment the scheduler is destroyed could hold its last reference. Common to every native module that posts from a background thread; here `undra_shutdown` (in the TurboModule's destructor, on the JS thread) joins the core threads first, so the window is closed whenever the module is destroyed before the scheduler. I did not verify React Native's teardown order beyond the reload proofs (no crash in the four reloads run on the two platforms). | Open (note) |
| I6 | Info | `test/contract/wasm-native.ts`, `contract-tests/run-all.sh` | The React Native contract column runs `NativeTransport` against `WasmNative`, a TypeScript re-implementation of the module's inbox, drain and port rules over the wasm core. It proves the transport and the rules as modelled, not the C++ module (that is `run.sh` plus RN01..RN10), and it is not in `run-all.sh`'s 54. Count it as a "NativeTransport over a stand-in" column, never as a native column next to Swift and Kotlin. | Open (note) |
| I7 | Info | `site/docs/cli.html` | The build-target table of the site has no `rn` row and the site has no React Native page. | Open (integrator) |

## The attacks, one by one

**1. Ownership (ADR-026): holds.** Traced per buffer:

* `undra_call_sync`, `undra_snapshot`: the `UndraBuf` becomes a `CoreBuffer` (`jsi::MutableBuffer`) owned by the
  returned `ArrayBuffer`; its destructor calls `undra_buf_free` once, whenever the collector releases it, on whatever
  thread (allowed; `buf_free` never touches the runtime, and an `EMPTY` buffer has `cap 0` and is ignored). The only gap
  was a throw before the owner existed (L5, fixed). `undra_schema_json`, `undra_stats_json`: copied into a
  `std::string` and freed at once (`takeString`).
* `reply_cb`, `changeset_cb`, `stream_cb`: lent `ptr, len` copied into the inbox under its mutex in `Host::append`; that
  is the one copy, and the host never frees the source (it is the core's, valid only during the callback). Allocation
  failure drops the record and counts it, never throws into the core.
* Port calls: async (a JS port) copies `port, method, port_call_id, args` into a `PortCall` record and returns 1; a
  native sync answer (`Clock`, `Rng`, `Log` with an id) is a `malloc`ed `PortReply` with `cap = 0`, which the core
  `free`s (`native.rs::take_host_reply`), and `Rng`'s failure path frees its own block; a JS sync answer is copied from
  the JS `ArrayBuffer` into a vector and then into a `malloc`ed block (`replyRaw`); the core's own log records (id 0)
  return 1 and allocate nothing; "unavailable" leaves `out_reply` untouched.
* Inbox to JavaScript: `takeInbox` swaps the vector out (no copy) into a `VectorBuffer` that the `ArrayBuffer` owns;
  nothing JavaScript receives aliases core memory after the host function returns. A JS sync port's arguments are
  copied before JavaScript sees them. JavaScript to the core: `(ArrayBuffer, offset, length)` borrowed for the call,
  rooted by the host function's arguments; Hermes keeps `ArrayBuffer` storage outside its moving heap.
* `invokeAsync` closures capture a `weak_ptr<Binding>` and the `alive` flag only; a strong `Binding` exists only on the
  JS thread inside the posted task, so a core thread can never run `~Binding` (whose `detach` shuts the core down). The
  wake function a core thread calls holds the invoker, the flag and the weak pointer. **Runtime destroyed while a core
  thread is inside a callback**: the TurboModule's destructor (JS thread) clears `alive`, then `undra_shutdown`, which
  joins the core threads and waits for running port callbacks (contract 1, 5), so the `Host` the callback uses (`user`)
  outlives it; anything it posted finds `alive` false. **Reload**: the core is shut down before the next one starts
  (destructor, takeover, or now the bounded wait, L1); `undra_init` is never called twice (one slot); `undra_shutdown`
  joins the runtime's threads, so none leak. iOS proven in this review, Android by the implementer.

**2. Threads: holds.** Every `jsi::` use is in `UndraJsi.cpp` / `UndraTurboModule.*` and runs on the JS thread: host
functions, the posted drain and frame tasks, and `JsAnswerer`, which `Host::answerJs` reaches only when the calling
thread's thread-local `CallScope` belongs to this host (that is, the JS thread inside a host function). `UndraHost.*`,
both shims and both frame sources contain none; the frame sources only post. The inbox mutex guards a vector insert or
swap and the counters; the wake, the core and JavaScript are always called outside it, and `Binding::mutex_` is never
held across the core or JavaScript either. Order across threads: one FIFO; `run.sh`'s "one inbox keeps a store's
commit order across the core thread and the JS thread" (40 core-thread `Todos.add` interleaved with JS-thread
`set_filter`, txn ids never go back) and the TS fake's equivalent; only the outermost drain delivers.

**3. Panic containment (R6): holds.** `undra-ffi` refuses to compile with `panic = "abort"` on native targets
(`lib.rs`), and every entry runs under `guarded` (`catch_unwind`), so nothing unwinds into C++; RN04 shows a core panic
as a status-2 reply with the core still answering, on both platforms (rerun here). C++ into the core: every callback is
`noexcept` and catches what can throw (allocation, the JS sync port); a `noexcept` violation would terminate, not
unwind into Rust, and L2/L3 removed the two places where one could. JavaScript exceptions from the sink, a sync port or
the frame callback are caught in C++.

**4. Ports: holds.** `Rng` is `arc4random_buf` (iOS: kernel-seeded ChaCha; Android's bionic and glibc 2.36+:
`getrandom`-backed), bounded at 16 MiB; `Clock.now_ms` is `system_clock`, `monotonic_ns` `steady_clock`. A JS sync port
called from a core thread is answered "unavailable", logged once per port, counted, and documented (`REACT_NATIVE.md`,
SPEC 11.2); the generated proxies treat it as a typed outcome. Async ports go to JavaScript. Every non-event port is
registered before `undra_init`; the ABI version and the schema hash are checked in `NativeTransport.start` before
`start` reaches `undra_init` (`UndraSchemaMismatchError`; RN10).

**5. Frame source: holds.** `CADisplayLink` on the main run loop and `AChoreographer` on the JS thread's looper, armed
per request, paused when nothing waits; a request after the tick's exchange resumes it. iOS: the link and its target
form the usual cycle, broken by `invalidate`, which the source's destructor dispatches to the main queue (ARC; the
callback's captures are released there). Android: a pending callback finds the source's state expired and only frees
its pointer (I1 for the reload case). Backgrounded: no frames come; the scheduler uses a zero-delay timer while
`AppState` is not active and the 100 ms backstop covers a frame requested just before. L7 fixed the module's single
`frame` slot.

**6. Packaging: holds.** The module's pod (`UndraReactNative.podspec`) compiles `cpp/` and `ios/` (Android's halves
excluded), links no core and uses no `-force_load`; the CLI's `UndraCore.podspec` force-loads the SDK's slice, marked
transitional for ADR-044. Android: `android/CMakeLists.txt` is added to `libappmodules.so` by autolinking
(`react-native.config.cjs`: `cxxModuleCMakeListsPath`, module name, header name); `dlopen("libundra_core.so",
RTLD_NOW | RTLD_LOCAL)`, never `dlclose`d (the core's threads live in it), errors carry `dlerror()` and the fix; the
built `libundra_core.so` and `libappmodules.so` have 16 KB (`0x4000`) aligned `LOAD` segments (checked with
`llvm-readelf`). `codegenConfig` registers `UndraModuleProvider` for iOS. `babel-plugin.cjs` replaces `import.meta` with
`{ url: undefined }` because Hermes's compiler rejects it and Metro bundles the runtime's `wasm-worker` mode even though
it never runs. `undra build --platform rn` writes `build/ios/UndraCore.xcframework` + `UndraCore.podspec` and
`build/android/jniLibs/<abi>/`, documented in SPEC 11.2 and `REACT_NATIVE.md` (the site lacks it, I7).

**7. Tests.** `cpp/test/run.sh` under ASan + UBSan: 14 checks per shim (13 + L1's), plus the JSI compile check; the TS
unit tests are 39 (36 + 3 regression tests, each failing on the original); the contract column 17 pass + S17 skipped as
app-tested. The stand-in is faithful to the module's *rules* (inbox records, drain-before-return, wake on another
"thread", the port routing, sync ports only inside a host function) but is a model (I6). The shared edits do not
weaken the TS column: S03 still asserts an in-process synchronous transport (`wasm-main` or `native`); S12 still asserts
exactly two and then three GETs, and only drops "before `create` resumes", which no SPEC text requires and the Swift and
Kotlin columns never asserted.

**8. Measurements and honesty: honest, now plain.** Methods are stated per row (`examples/playground/rn/src/bench.ts`:
medians of rounds, synthetic change-sets fed to `mirror.enqueue` and one `flush` for the per-frame rows, release core,
Hermes bytecode, three runs, a shared machine). Rerun here on the iOS simulator (release): JSI floor 22 ns, raw
`callSync` 0.24 us, `encodeCall` 4.1 us, `await` a generated method 15.7 us, the 1,667-patch drain 5.4 ms and parse
8.2 ms: inside or at the edge of the ADR's ranges. The 100,000-a-second statement is computed from those synthetic rows
(no 100,000-a-second firehose was run through the core); `REACT_NATIVE.md` now says plainly that it does not fit a
frame on React Native today and that E4 is open (L10).

## Gaps (recorded, not fixed)

* **G1b, default adapters.** No `Kv`, `SecureStore`, `Fs` or `Connectivity` adapter under React Native (the app
  supplies one; the playground uses an in-memory `Kv`). The integrator's plan (route the standard ports to the existing
  Swift and Kotlin adapters through the module) is the right one; it needs the module to register those ports natively
  and answer them on the platform side, so it touches decision 7 and wants an ADR amendment first.
* **No CI.** Nothing in `.github/workflows` builds or runs `runtimes/rn` or the app. Cheap and device-free, add now:
  a job (ubuntu-24.04 is enough: glibc 2.39 has `arc4random_buf`) that builds the playground core for `host` and `web`,
  then in `runtimes/rn/@undra/react-native` runs `npm ci`, `npm run typecheck`, `npm test`, `npm run test:contract` and
  `cpp/test/run.sh` (the JSI compile check needs `npm ci` in `examples/playground/rn` too). The app itself: a macOS job
  (CocoaPods, `undra build --platform rn --release`, `xcodebuild` Release for the simulator with
  `ONLY_ACTIVE_ARCH=YES ARCHS=arm64`, `simctl install/launch`, wait for `UNDRA-RN CHECKS 10/10 passed` in the simulator
  log) and an Android emulator job (`assembleRelease`, `adb install`, the same line in `logcat -s ReactNativeJS`).
* **iOS reload: closed in this review.** A debug build on Metro, reloads through `POST /reload`: three reloads, one
  after the end-to-end benchmark had started, four runtimes in the same process (pid unchanged), `CHECKS 10/10` each.
  Android's reload was not rerun after the fixes (the implementer's proof predates them; the changed C++ is the same
  code iOS ran).

## Runs after the fixes

`cpp/test/run.sh` 14 + 14 checks and the JSI compile check; `npm test` 39/39; `npm run typecheck` clean; `npm run
test:contract` 17 pass, S17 skipped (app-tested); `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
warnings` clean; `cargo test --workspace` 2,238 passed, 0 failed; `contract-tests/run-all.sh` 54/54. On devices with the
fixed native code: the iPhone 17 Pro simulator (debug on Metro, four runtimes across three reloads, and release) and
the Android emulator `undra-rn` (release, fresh bundle): `UNDRA-RN CHECKS 10/10 passed` every time.
