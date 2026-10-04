# SDE - prod-ops: production operations (wt/prod-ops, 2026-10-01)

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

| Artefact | Before (`main`, merged tree) | With this piece | Gate |
|---|---|---|---|
| Hello-world web core, wasm, gzipped | 117,081 | **119,227** (+2,146: the standard surface in every schema, `run_background`, the wasm FATAL record's location and operation; `--no-symbols` reproduces the old build) | 120,000 |
| Hello-world JavaScript runtime up front, gzipped (ts-size-e4's measure) | 21,336 | **21,756** (+420: trap-report loader 130, `runInBackground` 112, the page window 80, `pagehide`/`freeze` 80, the stats fields 76, the Diagnostics registration 56; the report builder, 853 bytes, and the `Diagnostics` port are lazy; `lazy_gzipped` 22,623 to 23,916) | **22,000** (was 21,500; ADR-052 amended) |
| Android `.so` (playground core, arm64-v8a / x86_64), `--no-symbols` vs symbols | 2,355,912 / 2,515,840 | 2,355,848 / 2,515,776 (smaller: the NDK's `llvm-strip` beats the linker's) | the test fails if it grows |
| iOS app, linked and stripped | 2,190,456 | 2,190,456 (byte-identical) | same |
| Host dylib | 2,227,072 | 2,227,032 | same |
| Web core of the symbols test project (playground, wasm raw / gzipped) | 879,095 / 353,111 | 875,686 / 351,605 | same |
| Symbol files (not shipped): the web `.dwarf.wasm`, the Android unstripped twin, the dSYM | see `build/symbols/manifest.json` | n/a | n/a |

## Counts, on the merged tree

| Suite | Result |
|---|---|
| `cargo test --workspace --no-fail-fast` | **3,163 pass, 0 failed, 18 ignored** (the two ignored schema tests below pass when run) |
| `cargo test -p undra-cli --test schema_retention --test schema_docs -- --ignored` | 2 pass |
| `cargo test -p undra-bench --test budgets --release`; `sync_alloc`, `commit_alloc`, `derived_alloc` | 6 pass; 5 + 2 + 1 pass |
| `cargo fmt --check`; clippy `-D warnings` host and `-p undra-ffi --target wasm32-unknown-unknown`; `cargo doc -D warnings` | clean |
| `cargo test -p undra-cli --test symbols --test debugging` (`UNDRA_REQUIRE_TOOLCHAINS=1`: iOS simulator, `undra` AVD, host, node) | 5 + 3 pass |
| wasm ABI harness (`crates/undra-ffi/tests/wasm/run.sh`) | 22 + 36 pass |
| TypeScript runtime / testkit | 1,646 (57 files) / 37 pass, typecheck clean |
| React Native: `npm test`; contract column; C++ host under ASan/UBSan | 100 pass; 19 pass, S17 and S29 skipped (app-tested), S30 pass; exit 0 |
| Kotlin runtime, brew Kotlin 2.4.20 and CI's 2.0.21 (fresh build dirs) | 784 cases, 0 failed, 2 skipped, both; testkit 32 |
| Android adapters, JVM (debug + release) / instrumented on the `undra` AVD / `android-work` JVM + instrumented | 302 results, 0 failed / 149 run, 0 failed (1 skipped) / 16 + 11 pass |
| Swift runtime | 753 pass |
| Contract grid (`contract-tests/run-all.sh`) | **80 cells pass** (TypeScript 28, Kotlin 26, Swift 26; S29 and S30 on all three), 0 failed or skipped |
| Interop (`crates/undra-transport/interop/run.sh`) | OK |
| `undra bindgen --check --docs` (playground, cookbook, fieldbook, two-cores a and b); iOS 15 sample without `--docs` | up to date |
| Site `build-all`, `check-links --words` | clean; landing prose 342 of 350 words |

## The merges with `main`

`main` moved twice under the branch. The first merge (ports-v2's opt-in ports, ios-floor, testkit docs) needed the
standard schema and every golden regenerated and the opt-in ports' schema hash re-blessed (the opt-in golden carries the
Diagnostics port and the three records); a duplicate `optionString` in the TS codecs was the only compile error. The
second (ts-size-e4 = ADR-056, ns-storage = ADR-044 A) rewrote the TypeScript runtime's structure under this piece's
code: `private _field` instead of `#field`, the lazy default ports, the events in `events.ts` and `browser-events.ts`. The
TypeScript implementer ported ADR-046 onto it (the Diagnostics port in the lazy `ports` chunk, the trap-report builder
lazy, `up-front.test.ts` to keep `core.ts` from statically reaching them) and the integrator re-measured: 21,756 up
front, over main's 21,500 gate, so the budget is 22,000 (ADR-052 amended, with the ablation). In Kotlin and Swift
`namespace` sits beside `onPanic` in the load options (`onPanic` last). Deviations 18 and 19 of the ADR-046 amendment
are the consequences (remote pages fetch the `ports` chunk at load; S29 is a skip, with reason, in the React Native column).

## Open items

* iOS `BackgroundTasks` does not run in the simulator: the registration was run there and the engine is tested with
  fakes; a device run of the BG handler is the debugger recipe in `docs/` ("Operating in production").
* Web Background Sync in a service worker is the follow-up the ADR names (the page's own life only).
* A trap during `undra_init`, before the lazily imported report builder has arrived, gets no `onPanic` report (`load`
  still rejects with the trap).
* ts-size-e4 (ADR-056) reached 21.2 KB up front for the hello-world JavaScript; this piece's cost
  against that budget is in the sizes table above.
