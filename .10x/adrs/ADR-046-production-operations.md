# ADR-046: production operations: symbol files from every release build, a debugger path into Rust, background runs, and panic reports for the app's crash reporter

Status: **Proposed** (2026-10-01, `wt/boundary-adrs`; Amendment B new Track I "production operations",
catalogue M-2, M-5, M-8 and finding 4). Touches SPEC 0 (release artefacts), 5.6/5.1 (what a contained panic
produces), 7 (the web build's debug sections), 8 (one standard port, one standard function, two standard
records), 9 (`Background` flushes persistence), 13, 17 (`runInBackground`, `onPanic`) and ADR-024's standard
table; `undra-cli` (builds, templates, `doctor`), `undra-runtime`, `undra-query`, `undra-ports`,
`undra-bindgen` (the standard table), the three runtimes and two optional platform modules. **No wire change
and no C ABI or wasm ABI change: a background run is an ordinary async call of a standard function, and a
panic report is a standard port call.** The schema of every core changes once (the standard surface grows), so
every core's hash changes once, which is acceptable before publication (ADR-024: "changing the standard
surface already needs an ADR"). Constitution R6, R7, R9, R11, R12.

## Context

The catalogue (`.10x/specs/2026-10-01-competitive-limitations.md`, finding 4): "Running in production is
unplanned. No crash-symbol files are emitted, step-debugging from Swift or Kotlin into the core is
undocumented, and nothing integrates with OS background execution (BGTaskScheduler / WorkManager). Every
production team hits the first. Offline-first teams hit the third. None of the three is in Tracks A–H."
Matrix rows 13 (background execution), 17 (step-debug into shared code) and 33 (crash symbolication): Undra
"no" on all three.

What the code does today:

* **Symbols.** The shim's release profile is `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`,
  `debug = false` (`crates/undra-cli/templates/shim/Cargo.toml.tmpl:48-54`); the web build always runs
  `wasm-opt -Oz … --strip-debug --strip-producers` (`crates/undra-cli/src/builds/web.rs:131-145`; binaryen's
  `--strip-debug` also strips the names section). No platform writes a symbol artefact. The Xcode template's
  Release configuration already asks for `DEBUG_INFORMATION_FORMAT = "dwarf-with-dsym"`
  (`crates/undra-cli/templates/ios/project.pbxproj:194`), but the static library it links has no DWARF, so the
  app's dSYM has no Rust frames. The Android template has no `debugSymbolLevel` (`templates/android/app/build.gradle.kts:16-26`).
  `undra build` has `--release` and nothing about symbols (`crates/undra-cli/src/cli.rs:234-246`).
* **Debugging.** Debug builds use cargo's dev profile (full debug info), and the iOS app links the static core,
  so LLDB could step into Rust, but nothing documents or checks it; on Android the `.so` comes from `cargo ndk`
  unstripped in debug; on the web the module is always the stripped release one.
* **Background.** `Lifecycle` is an event port with `changed(AppState)` (`crates/undra-ports/src/ports.rs:138-144`);
  `undra-query` reacts only to `Active` (`crates/undra-query/src/shared.rs:759-785`); `Background` and
  `Inactive` are ignored by every crate. Swift's Lifecycle is reported by the app (`UndraLifecycle`,
  `Adapters/ConnectivityAdapter.swift:94-129`), Kotlin's is a stub (`adapters/SimplePorts.kt:216-225`), the web
  uses Page Visibility (`adapters/browser.ts:67-97`). No BGTaskScheduler, WorkManager or service-worker sync
  exists anywhere in the repository.
* **Panics.** A contained panic becomes a status 2 reply with `message` and a `Backtrace::force_capture()`
  prefixed with `panicked at file:line:col` (`crates/undra-runtime/src/guard.rs:62-92`, `:127-155`) and a FATAL
  log record, target `undra::panic` (`crates/undra-runtime/src/runtime.rs:759-766`); a detached task's panic is
  logged only (`:1748-1756`). Swift turns status 2 into `UndraCallError.panicked` and reports commands to
  `LoadOptions.onError` (ADR-032); Kotlin has no `onError`; TypeScript's `onError` does not receive traps. An app
  that wants Crashlytics or Sentry to see a core panic has to parse log text.

The probe of ADR-044 shows the symbol half is cheap: a prelinked static Rust object built with
`debug = "line-tables-only"` symbolicates through the app's dSYM (`atos`: `a_undra_report (in t8) (lib.rs:3)`),
and the linked binary is the same size as without debug info.

## Decision

### 1. Every release build writes symbol files

1. The release profile keeps line tables: `debug = "line-tables-only"`, `split-debuginfo = "unpacked"` on Apple
   targets, and **no** `strip` in the profile; `undra build --release` strips the shipped copies itself and
   keeps the unstripped ones. Optimisation and LTO are unchanged, so the code is the same. `--no-symbols` skips
   the symbol outputs; debug builds are unstripped anyway.
2. **Outputs** (namespaced per ADR-044), with a `build/symbols/manifest.json` recording, per artefact, the
   namespace, core version, schema hash and image identity (Mach-O UUID per slice, ELF build id per ABI —
   rustc gets `-C link-arg=-Wl,--build-id` on Android — and the SHA-256 of the shipped wasm):
   * **iOS / macOS:** the prelinked `lib<ns>.a` (ADR-044) keeps its DWARF line tables, so the **app's own dSYM**
     (Xcode Release, `dwarf-with-dsym`, already in the template) contains the Rust frames; Crashlytics and Sentry
     upload that dSYM as they do for Swift. There is no separate core dSYM for a static library.
   * **Android:** `jniLibs/<abi>/lib<ns>.so` is stripped (`llvm-strip --strip-debug --strip-unneeded`, from the
     NDK); `build/symbols/android/<abi>/lib<ns>.so` is the unstripped twin, and
     `build/symbols/android/native-debug-symbols.zip` is the Play Console layout (`<abi>/lib<ns>.so`). The docs
     show Play Console upload and Crashlytics' `unstrippedNativeLibsDir`.
   * **Web:** one `wasm-opt` run produces `build/symbols/web/<ns>.debug.wasm` (optimised, with DWARF line tables
     and the names section, `-g`) and the function map `build/symbols/web/<ns>.wasm.functions.txt`
     (`--print-function-map`); the shipped `build/web/<ns>.wasm` is that module with its debug and name sections
     removed and nothing else changed, so every `wasm-function[i]:0x…` in a production stack trace resolves in
     the debug module and the map.
   * **Host libraries** (JVM desktop, macOS host): `dsymutil` / `objcopy --only-keep-debug` next to the library.
3. **A test proves it** (R9 for symbols): `crates/undra-cli/tests/symbols.rs` builds the playground core in
   release, triggers a known panic and a known crash site, and resolves their addresses with the outputs to the
   right `file:line` on each platform (`atos` with the app dSYM, `llvm-symbolizer` with the Android `.so`, the
   wasm function map). A size check proves the shipped artefacts did not grow.

### 2. A documented, checked debugger path into Rust

1. **Debug builds are the debuggable ones**: `undra build` (no `--release`) for iOS keeps full DWARF in the
   prelinked object; Android debug `.so`s are unstripped and the generated Gradle module sets
   `packaging.jniLibs.keepDebugSymbols += "**/lib<ns>.so"` for the debug variant; the web dev build serves
   `<ns>.debug.wasm`.
2. **Per platform, one page and one `doctor` check each** (`site/docs/debugging.html`, `docs/ONBOARDING.md`):
   * **Xcode:** step into Rust from a Swift call; breakpoints by `breakpoint set -f todos.rs -l 42` (or in the
     editor once the `.rs` file is opened); `undra init` writes an `.lldbinit` that loads the toolchain's Rust
     formatters (`rustlib/etc/lldb_commands`), and `undra doctor` checks the path exists.
   * **Android Studio:** the "Dual (Java + Native)" debugger with the debug `.so`; symbol directory
     `build/symbols/android` for release reproductions; `doctor` checks the NDK's LLDB.
   * **Chrome DevTools:** the C/C++ DevTools Support (DWARF) extension reads `<ns>.debug.wasm`; breakpoints in
     `.rs` files. `doctor` cannot check a browser extension and prints the link.
   * **Host tests:** `rust-lldb` or CodeLLDB on `cargo test`.
3. `crates/undra-cli/tests/debugging.rs` (macOS CI): runs LLDB in batch mode against the iOS-simulator debug
   build of the playground, sets a breakpoint by Rust `file:line`, calls the method from Swift and asserts the
   stop. This is the regression test for "steppable" (prelinking included).

### 3. Background runs: the OS grants a window, the core drains in it

1. **A standard function** (ADR-024 rules: in every schema, never generated, implemented by the runtimes'
   own API): `run_background(deadline_ms: u64) -> BackgroundReport` with
   `BackgroundReport { finished: bool, replayed: u32, refetched: u32, still_pending: u32 }`, called like any async
   free function. It runs every registered **background task** concurrently with the deadline (Clock port,
   R12) and returns when all are done or at `deadline − 500 ms`, whichever is first. A host that is about to lose
   its window cancels the call (status 3); work already done is kept (the offline queue persists per item,
   ADR-037). No ABI change: it is an ordinary call with an ordinary reply and cancellation.
2. **Background tasks** are registered through `inventory` (`BackgroundTask { name, run: fn(&Ctx, Deadline) ->
   BoxFuture<'static, BackgroundOutcome> }`); `undra-query` registers "replay the offline queue",
   "refetch observed-or-persisted stale queries" and "flush pending persistence"; app code registers its own with
   `undra::background::register` (or a `#[undra::background]` attribute on an `async fn(ctx, deadline)`, D1's
   call). Each task holds a `WeakCtx` (ADR-034).
3. **The Lifecycle port keeps its shape**; what changes is who listens: on `Background`, `undra-query` flushes
   debounced persistence immediately and records whether background work is pending (the queue is non-empty or a
   persisted query is stale), which the runtimes read (`stats`) to decide whether to schedule a run. Swift and
   Kotlin report Lifecycle by default (`UIApplication`/`NSApplication` notifications; `ProcessLifecycleOwner` in
   C4a), as the gap audit's PO-9 asks.
4. **Platform integration** (runtime packages, not generated):
   * **iOS:** `UndraBackground.register(taskIdentifier:loader:)` at launch registers a `BGProcessingTask`
     (requires network) and a `BGAppRefreshTask` with `BGTaskScheduler`; the handler loads the core if the app
     was launched in the background (`loader` is the generated `Undra<Ns>.load`), calls
     `core.runInBackground(deadline:)`, cancels it from the task's `expirationHandler`, and calls
     `setTaskCompleted(success: report.finished)`. When the app enters the background with pending work, the
     runtime schedules the request and wraps the transition in `beginBackgroundTask` so in-flight mutations
     finish. `undra init` adds `BGTaskSchedulerPermittedIdentifiers` and the background mode to `Info.plist`.
   * **Android:** a new optional module `android-work` (WorkManager is a dependency the base runtime must not
     carry): `UndraWorker : CoroutineWorker` loads the core with the application context, runs
     `runInBackground(deadline = now + 9 min)` (WorkManager stops a worker at 10), returns `success()` or `retry()`;
     `UndraWork.schedule(context)` enqueues unique work with a network constraint when the app goes to the
     background with pending work, or when a mutation is queued offline.
   * **Web:** within the page's life only in v1.x: on `visibilitychange → hidden` and `pagehide`/`freeze` the
     runtime flushes persistence and runs a short background run (deadline 1 s). Background Sync in a service
     worker needs the core to run inside the worker (its own wasm instance and IndexedDB adapters) and is a
     follow-up, stated as such.
5. **Typed outcome everywhere**: `runInBackground` returns the report (Swift `async -> BackgroundReport`,
   Kotlin `suspend`, TS `Promise`) and throws only `UndraCallError`-class failures; a background run never
   throws a panic into OS callbacks (R6).

### 4. Panic reports reach the app's crash reporter

1. **One structured report per contained panic.** Every panic the runtime contains — a call, a stream, a
   detached task, an effect, a computed (ADR-019/A3), a port or callback dispatcher, a background task, a
   migration hook (ADR-037) — produces a `PanicReport { message: String, location: String /* file:line:col */,
   operation: String /* "Todos.add", "task", "computed Todos.visible" */, thread: String, frames: Vec<PanicFrame
   { address: u64 /* relative to the image base */, symbol: Option<String>, file: Option<String>, line: Option<u32> }>,
   namespace: String, core_version: String, schema_hash: u64, image_id: String /* Mach-O UUID / ELF build id /
   wasm SHA-256 */ }`. `location` and `message` are always present (the panic location is compiled in);
   `symbol`/`file`/`line` are present when the shipped image still has them (debug builds); `address` is always
   present on native so a server-side symbolicator with the files of decision 1 resolves it. Frames are captured
   with the `backtrace` crate's raw instruction pointers on native (a dependency, checked for iOS, Android and
   wasm32 per CLAUDE.md; on wasm32 the frames come from the trap's JavaScript stack instead).
2. **Delivered through a new standard port**, `#[undra::port(sync)] trait Diagnostics { fn panicked(&self,
   report: PanicReport); }`, called fire-and-forget (`port_call_id 0`, the `Log` path, ADR-026 §4) after the
   FATAL log record; the runtimes implement it by default and forward to **`LoadOptions.onPanic`** (Swift,
   Kotlin, TypeScript; called once per report on the main thread; never throws into the core). The default when
   `onPanic` is not set is to log. The status 2 reply and `UndraCallError.panicked` are unchanged.
3. **Recipes, not dependencies.** The docs give the three-line bridges: Crashlytics (`ExceptionModel` with
   `StackFrame(address:)` on iOS, `recordException` with synthetic frames on Android), Sentry (an event with the
   frames' `instruction_addr` and the image's debug id) and the browser's `reportError`. Undra links no reporter.
4. **wasm**: a panic traps the module; the FATAL `undra::panic` record precedes the trap (SPEC 7). The TS
   runtime turns that record plus the trap's stack into the same `PanicReport` (frames as `wasm-function[i]`
   offsets, resolved with decision 1's map) and calls `onPanic` before ADR-049's recovery runs.

## Alternatives considered

* **A separate dSYM for the core.** Static libraries have no dSYM of their own; the app's dSYM is where crash
  reporters look. A dynamic framework would have one, at the costs ADR-044 lists.
* **Keep the release profile stripped and ship a separate debug build for symbols.** The two builds would not be
  the same code, so addresses would not match. One build, two copies is the only reliable way.
* **Full debug info in release.** Larger symbol files and slower links for variable locations nobody reads in
  a crash report; line tables give `file:line` per frame.
* **A `Lifecycle` event `background_window(deadline_ms)`.** Events are fire-and-forget: the host could not
  learn when the core is done (it must call `setTaskCompleted`) and could not cancel. A call has both.
* **WorkManager inside `android-adapters`** forces the dependency on every Android app; an optional module does
  not.
* **Panic reports as parsed log text.** Fragile, loses addresses, and every app writes the parser. A standard
  record is one more ADR-024 item.
* **Link a crash reporter SDK.** Picks a vendor for every app and adds a dependency to the runtimes.

## Consequences

* Rust frames symbolicate in Crashlytics, Sentry and Play Console; debugging into Rust is documented and
  tested on every platform; offline queues drain while the app is in the background on iOS and Android; core
  panics reach the app's reporter as structured values. Catalogue rows 13, 17 and 33 move to "yes" (web
  background: "part.").
* Every core's schema hash changes once (the standard surface gains `Diagnostics`, `PanicReport`, `PanicFrame`,
  `run_background`, `BackgroundReport`); `undra_bindgen::stdlib` and the runtimes' standard types follow
  (ADR-024).
* Release builds take longer (symbol extraction, a second wasm-opt output) and write more files; shipped
  artefacts do not grow (tested).
* Two optional platform modules: `android-work` (WorkManager) and the iOS `UndraBackground` helper (in the Swift
  runtime, BackgroundTasks framework, iOS only).

## Risks

* **Background windows are short and unreliable** (the OS decides). The design never assumes a window: it
  persists progress per item and reports what is still pending.
* **iOS background launch** loads the core without UI; every adapter must be safe without a window scene (the
  Swift adapters are). Covered by a test that runs the BG handler in the simulator
  (`_simulateLaunchForTaskWithIdentifier` in the debugger, documented by Apple).
* **The `backtrace` dependency** must pass the dependency rule; if it does not, frames fall back to std's
  `Backtrace` text (symbols when present, no addresses) and the report says so.

## Implementation brief

1. `crates/undra-cli`: the shim's release profile (decision 1.1); `builds/ios.rs` (keep DWARF in the prelinked
   object), `builds/android.rs` (strip the shipped copy, keep the twin, the Play zip, `--build-id`),
   `builds/web.rs` (`-g` debug module, function map, stripped shipped module), `builds/host.rs`; `manifest.json`;
   `--no-symbols`; templates (Gradle `keepDebugSymbols` for debug, `.lldbinit`, `Info.plist` keys, the background
   registration in the iOS bootstrap and the Android `Application`); `doctor` checks; tests `symbols.rs` and
   `debugging.rs`.
2. `crates/undra-ports`: `Diagnostics` port, `PanicReport`, `PanicFrame`, `BackgroundReport`, the
   `run_background` function and the `BackgroundTask` registry types; fakes (`CaptureDiagnostics`).
3. `crates/undra-runtime`: build a `PanicReport` at every containment site (`guard.rs`, `runtime.rs`
   `note_panic`, task, stream, effect, computed, dispatcher, background task), the `Diagnostics` call after the
   FATAL record, frames via `backtrace` on native; `run_background` (spawn registered tasks with the deadline,
   return at the deadline margin, cancellation).
4. `crates/undra-query`: the three background tasks; `Background` flushes persistence and updates the pending
   flag in `stats_json`.
5. `crates/undra-bindgen`: the stdlib table gains the new items (filtered from app bindings).
6. Runtimes: `LoadOptions.onPanic` and the default `Diagnostics` adapter (Swift, Kotlin, TS); `runInBackground`;
   Swift `UndraBackground` (BackgroundTasks) and default Lifecycle reporting from `UIApplication` notifications;
   Kotlin `android-work` (`UndraWorker`, `UndraWork`); TS page-lifecycle flush and the trap-to-report path.
7. Contract scenarios (provisional numbers): **S27 "panic report"** (a panicking method: the caller gets status 2
   and `onPanic` receives message, `file:line`, operation, namespace and schema hash; a panicking detached task
   reports too) and **S28 "background run"** (a mutation queued offline; connectivity returns; `runInBackground`
   replays it and reports `finished`; a run cancelled at its deadline leaves the item queued, intact).
8. Bench: no runtime row changes (the panic path is cold); the release-size budget rows (`bench/RESULTS.md`
   size table) must not move; a build-time row for `undra build --release` with symbols.
9. Docs: SPEC 0, 5, 7, 8, 9, 13, 17; `site/docs/debugging.html`, `site/docs/crash-reporting.html`,
   `site/docs/background.html`; the cookbook's offline-first page (background drains).

## Dependencies

ADR-044 (namespaced artefacts and the prelinked iOS object, which decision 1 keeps DWARF in), ADR-034
(`WeakCtx` for background tasks), ADR-037 (per-item queue persistence makes cancelled runs safe), C4a (Android
Lifecycle adapter), ADR-049 (web recovery runs after the panic report). D3 (build-system integration) shares
the build plumbing and should land the template changes with it.
