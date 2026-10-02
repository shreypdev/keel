# SDE — prod-ops: production operations (wt/prod-ops, 2026-10-01)

ADR-046 (Accepted, with the dated amendment at its end). The piece makes a shipped Undra app operable: a panic in
the core reaches the app's crash reporter on every platform with one shape; a background window drains the offline
queue and the cache; the release build keeps symbol files and `undra symbolicate` turns a report into Rust lines;
LLDB stops at a Rust line. The Rust core, the query tasks, bindgen, the docs and the integration were done by the
integrator; the Swift, Kotlin, TypeScript/React Native runtimes and the CLI by one implementer each against the
shared contract (the ADR's brief plus a cross-runtime contract note), integrated here.

## What landed

* **Panic report** (`undra-runtime`, `undra-ports`, `undra-ffi`): every contained panic is the FATAL `undra::panic`
  log record it always was plus one `PanicReport` to the standard sync port `Diagnostics.panicked`, fire and forget
  (`port_call_id 0`, like `Log`), at every containment site (calls, async calls, detached tasks, computeds, observers,
  snapshots, init hooks, the executor's and the timer's wakers, the `undra-ffi` entries). The report encodes by hand
  in the runtime (layout of `undra_ports::PanicReport`; a test of `undra-ports` decodes it with the type).
  Frames come from `undra-ffi`'s `FrameSource` (the platform unwinder, `dladdr`, the image's own Mach-O UUID / ELF
  build id; the only `unsafe`, each block with its `// SAFETY:`), as offsets into the core's image pointing into the
  call instruction; debug builds also name them. Namespace and version are a `CoreIdentity` that
  `export_core!(ns, jni_class = .., version = ..)` submits.
* **The shape** (identical in every language; `UndraPanicReport`, `UndraPanicFrame`, `UndraBackgroundReport`):
  `message, location (file:line:col), operation (Todos.add, explode, task, computed Todos.visible, ...), thread,
  frames [address u64, symbol?, file?, line?], namespace, coreVersion, schemaHash u64, imageId (hex)`. Delivered once per
  report to `LoadOptions.onPanic` on the main thread, in order; a throwing handler is caught and goes to `onError`;
  with no handler the runtime logs one ERROR line, so a panic is never silent.
* **wasm**: the core cannot call out of a panic (panic=abort). Its FATAL record carries `    at <loc>` and `    in <op>`
  trailer lines and the TypeScript host builds the same report from that record plus the trap's stack; `imageId` is the
  SHA-256 of the module, computed only when `onPanic` is set. `wasm-main` and `wasm-worker` both.
* **Background runs**: standard async function `run_background(deadline_ms) -> BackgroundReport {finished, replayed,
  refetched, still_pending}` (id `0x0e5b14ff`), tasks registered per runtime
  (`Runtime::add_background_task`, `undra::background::register`), linked by use. The query client registers replay,
  refetch and flush; each ends by settling. Offline ends at once. `stats_json` has `panic_reports` and a `background`
  section once a task exists. Platforms: iOS `UndraBackground` (BGTaskScheduler; `BackgroundEngine` over four seams),
  Android `UndraWorker`/`UndraWork` in the optional `android-work` module and the Lifecycle adapter's pending callback,
  web the page's own life (`visibilitychange`/`pagehide`/`freeze`, a one-second run, `backgroundRun: false` opts out).
* **Symbols and debugging** (`undra-cli`): `debug = "line-tables-only"` and no strip in the shim profile;
  `build/symbols/manifest.json`; iOS dSYM through the debug map of the prelinked object, Android stripped `.so` + its
  unstripped twin + `native-debug-symbols.zip`, web `<ns>.debug.wasm`, `.wasm.functions.txt`, `.dwarf.wasm` (two
  `wasm-opt` runs); `undra symbolicate`; `--no-symbols`; the generated `.lldbinit`; `undra doctor` checks; generated CI
  uploads `build/symbols/`. The RN host answers Diagnostics natively and queues reports for the JS thread.
* **Standard surface** (ADR-024 amendment): 11 ports (+`Diagnostics`), 12 core types (+3), 1 function; every core's
  schema hash moved once (playground `0xd796bf6e2c6ada46`); bindgen leaves the three types and the function out of
  generated code and spells them in each language's runtime.
* **Testkits**: `CaptureDiagnostics` (Rust fake, Kotlin and TS testkits, the Swift runtime's fake); `PreviewCore`
  records a trap's report.
* **Contract scenarios S29 "panic report" and S30 "background run"** (S27 and S28 are `objects-callbacks`'), on every
  platform; the playground gained `explode_detached`; playground apps show the report in their debug panels.
* **Docs**: SPEC 0, 5.6, 5.11, 6, 7, 8, 9, 17; ADR-024 amendment; ADR-046 Accepted with the amendment (17 numbered
  deviations); `site/docs/production.html` ("Operating in production"); ONBOARDING's suites; `bench/RESULTS.md`
  "Production operations".

## The symbolication proof (R9)

`UNDRA_REQUIRE_TOOLCHAINS=1 cargo test -p undra-cli --test symbols --test debugging` builds the playground core in
release for each platform, makes it panic in a call (`explode`) and in a detached task (`explode_detached`), takes the
report's frame addresses and image id, and resolves them with `undra symbolicate` against the build's symbol files:

* iOS (a booted simulator, Release app): `playground_core::lab::explode` at `lab.rs:222`, its dispatcher at `lab.rs:221`,
  the detached task closure at `lab.rs:236`;
* Android (the `undra` emulator, release `.so` with its unstripped twin): the same three lines;
* macOS host dylib with the dSYM next to it: `lab.rs:222` and `lab.rs:221`;
* web (node, `wasm-opt`ed module with the function map): function level, the dispatcher (`lab.rs:220`) and the detached
  closure (`lab.rs:236`); the report's own `location` has the line.

LLDB in batch mode stops at `breakpoint set -f lab.rs -l 222` in a debug core on the host and in a process of the iOS
simulator (the harness stops itself and `process attach` follows).

## Decisions that are not in the ADR text (the amendment has the full list)

Background tasks are registered, not inventory-submitted; the deadline is the runtime's monotonic clock plus a `Timer`
sleep (no `Clock` port call: it would link the Clock proxy into every core); every query task ends by settling, because
a replay's invalidation refetch had left the persisted entry dirty (S30 step 3 caught it); iOS carries a debug map,
not DWARF (`ld -r` writes `N_OSO` stabs; the app's own dSYM holds the Rust frames); the web build runs `wasm-opt` twice
because binaryen skips DWARF-unsafe passes when it keeps DWARF; the Android release build had ignored ADR-052's home
remap (17 `/Users/<name>` strings in the baseline `.so`): fixed; `.lldbinit` is one `script` line that asks
`rustc --print sysroot` for the formatters.

## Sizes (R9, ADR-052)

Hot path untouched: `sync_alloc`, `commit_alloc`, `derived_alloc` and the budgets test hold at their committed numbers;
a contained panic with its report is 34.6 µs p50 (32.5 µs with the port unbound), an ignored measurement
(`cargo test --release -p undra-ports --test diagnostics -- --ignored --nocapture panic_cost`).

SIZES_TABLE

## Counts, on the merged tree

COUNTS_TABLE

## Open items

* iOS `BackgroundTasks` does not run in the simulator: the registration was run there and the engine is tested with
  fakes; a device run of the BG handler is the debugger recipe in `docs/` ("Operating in production").
* Web Background Sync in a service worker is the follow-up the ADR names (the page's own life only).
* A trap during `undra_init`, before the lazily imported report builder has arrived, gets no `onPanic` report (`load`
  still rejects with the trap).
* `ts-runtime-size` aimed at 16 KB for the hello-world JavaScript; ADR-056 reached 21.2 KB up front. This piece's cost
  against that budget is in the sizes table above.
