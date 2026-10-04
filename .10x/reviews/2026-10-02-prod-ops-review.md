# `prod-ops` (ADR-046 production operations: panic reports, background runs, symbol artefacts) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/prod-ops` at `ecdd6a2`
(`main` `b800994` merged), then `main` at `f9a37a8` (objects-callbacks, ADR-040/041) merged by the review · **Read:** `CLAUDE.md` (R1, R2, R6,
R7, R9, R12), ADR-046 with its amendment, ADR-052 and its amendments, ADR-024's amendment, `.10x/decisions/sde/prod-ops.md`, SPEC 0, 5.6,
5.11, 6, 7, 8, 9, 17, `site/docs/production.html`, and the diff: `crates/undra-ffi` (`frames.rs`, `guard.rs`, `session.rs`, `table.rs`),
`crates/undra-runtime` (`guard.rs`, `diagnostics.rs`, `background.rs`, `runtime.rs`, `timer.rs`, `ctx.rs`, `persist.rs`), `undra-ports`
(`background.rs`, `records.rs`, `ports.rs`, fakes), `undra-query` (`background.rs`, `queue.rs`, `shared.rs`), the three runtimes' `onPanic`
and background helpers, the CLI's symbol path and generated CI, S29/S30 and the testkit fakes · **Fixes:** five `fix(prod-ops): review fixes`
commits, three `bench(prod-ops)` re-records, one merge of `main`, then this record.

## Verdict

**Sound with fixes; merge.** One High, fixed: merged with `main` (objects-callbacks), the hello-world wasm measured **121,164** bytes gzipped,
1,164 over its 120,000 gate (H1); after two changes that drop no behaviour it is **119,654**. Four Medium, fixed with tests that fail without the
fix: the ELF build-id reader assumed the image's first segment sits at virtual address 0 and that the program headers are mapped wherever
`e_phoff` says, so a core linked into a fixed-address executable read far outside its image in the panic path, and the Mach-O walk was bounded
by a command count, not by `sizeofcmds` (M1, R2: two `// SAFETY:` comments were true only for the images `undra build` makes); a panicking
background task constructor and a panicking `pending` probe were contained but never reported (M2, R6); D1: `runInBackground` and the
`Diagnostics` registration were up front although a hello page never runs either (M3); and the contract grid's own run caught a race: a wasm
trap right after `load` carried an empty `imageId`, because the module was hashed in the background (M4). The `unsafe` is otherwise right: the stack is walked from
the panic hook, on the panicking thread, before unwinding; `dladdr` only looks an address up; the reader is stateless and thread-safe; and the
parsers now run under Miri. The symbolication proof holds when re-run here (iOS simulator, the `undra` AVD, host, node), and `--no-symbols`
changes no code in the linked iOS app. Low findings are open items, not blockers: debug-build frames pair addresses with names by aligning two
stacks from the bottom, which out-of-image and inlined frames shift (L1); `Deadline::remaining()` never decreases on wasm, where the runtime's
clock is the manual one (L2); the generated CI uploads the unstripped Android twin as a workflow artefact, public for a public repository (L3);
and the iOS app built with and without `--no-symbols` is the same code and size but not byte-identical as recorded, only because the test builds
the two in different directories (L4).

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| H1 | High | the merged tree; `crates/undra-runtime/src/guard.rs` (`guarded`), `crates/undra-ports/src/background.rs` | With objects-callbacks merged the hello wasm was 121,164 gzipped against a 120,000 budget (`main` 119,055; this piece +2,109). Named builds and twiggy, then the script's own gzipped deltas: the `async fn run_background` linked its future, its reply encoding and its dispatcher into every core (-745 when removed), the wasm hook's operation naming (-78), and the most, the reporting paths of a *caught* panic, which a wasm core can never run (with `panic = "abort"`, `catch_unwind` never returns `Err`). | **Fixed** (`e0176e9`): `guarded` is `Ok(f())` on wasm (the hook still logs the FATAL record before the trap; -1,137, part of it `main`'s own dead paths), and `run_background`'s dispatcher is written by hand with the very declaration the macro produced (schema and hash unchanged, `tests/schema.rs`): a runtime with no task answers an idle report synchronously (`Runtime::background_call`), the asynchronous run is reachable only through `add_background_task` (-378). `tests/background_call.rs`: idle answers at once, a bad body is a bad request, `call_sync` refuses it by its declaration, a core with a task runs it asynchronously. **119,654** (+599 on `main`), budget unchanged. |
| M1 | Medium | `crates/undra-ffi/src/frames.rs` (`elf_build_id`, `macho_uuid`) | R2. `elf_build_id` read the note at `base + p_vaddr`, "relative to the load bias (the base of a shared object, whose first segment is at virtual address 0)": true for what `undra build` links, false for a fixed-address executable (a C embedding built `-no-pie`, the README's "Embedding the C ABI"), where `dli_fbase` is 0x400000 and the read lands at `base + 0x400xxx`, in the panic path. The program headers were read wherever `e_phoff` points, which no loader guarantees is mapped (glibc copies a table outside the segments). `macho_uuid` walked `ncmds.min(512)` commands with no bound but each `cmdsize`. | **Fixed** (`9462ed0`): Mach-O commands are read only inside `sizeofcmds` (dyld refuses an image whose commands leave the first segment); ELF program headers only when inside the first page, notes only inside a `PT_LOAD` segment, at the load bias (`base` minus the vaddr of the segment at file offset 0). `visit` cannot overflow (an unwind out of the `extern "C"` callback would abort). Four synthetic-image tests (a shared object, a fixed-address executable, headers pointing off the page or off every segment, a Mach-O UUID past `sizeofcmds`): three fail on the old code, all pass, and pass under Miri. |
| M2 | Medium | `crates/undra-runtime/src/background.rs` (`Registry::pending`, `run`) | R6, one report per contained panic. A task whose `run` panicked before returning a future, and a panicking `pending` probe (asked by every run and every `stats_json`), were guarded and dropped silently: no FATAL record, no `Diagnostics` report, not counted. | **Fixed** (`9462ed0`): both go through `log_panic` (operations `task` and `background pending <name>`). `tests/panic_reports.rs` fails on the old code (only the running task's panic was reported). |
| M3 | Medium | `runtimes/ts/@undra/runtime/src/core.ts` | D1 (below): two items in the first chunk that a hello page never runs. | **Fixed** (`47b0d2d`): `runInBackground` is the `background.js` chunk (281 bytes), the `Diagnostics` registration is `serveDiagnostics` in the `ports` chunk; `up-front.test.ts` guards `background.ts`. The React Native abort test waited one tick for a call that now loads a chunk first (`c7d8169`). |
| M4 | Medium | `runtimes/ts/@undra/runtime/src/panic-report.ts` (`start`), `core.ts` (`_loadPanics`) | The grid's TypeScript S29 failed on the merged tree: `imageId` was `""`. The module was hashed in the background after `load`; one call later the playground's debug module trapped first on a busy machine. A crash at startup is the one most worth symbolicating, and the report had no build id. | **Fixed** (`47153a6`): `PanicReporter.ready` settles when the hash is known; `_start`, which already waited for the report builder while the module compiles, waits for both (only for an app with `onPanic`). A test with a 50 ms digest fails without the fix; S29 passes. +4 bytes up front. |
| L1 | Low | `crates/undra-runtime/src/diagnostics.rs` (`name_frames`) | In a **debug** build each named frame gets an address by aligning the backtrace text with the walked addresses from the bottom. The walk keeps only frames inside the core's image; the text has every frame, out-of-image ones (`_pthread_start`, `thread_start`, `start`) at the bottom and the host's own in between, and one line per inlined symbol. The pairs drift by that many frames, so a debug report's `address` need not belong to its `symbol`. Release builds (what ships and what the symbolication proof uses) carry addresses only and are unaffected. | **Open**: pair by instruction address (the text does not print it in the short style), or leave `address` 0 on named frames in debug builds. Checked with a standalone probe of the same two captures on a spawned thread (macOS): 29 text frames, 21 walked, 20 in the image, the text's bottom frame out of it. |
| L2 | Low | `crates/undra-runtime/src/background.rs` (`Deadline`) | On wasm the runtime's clock is the manual one, which nothing advances in production, so `Deadline::remaining()` stays at the full budget and `expired()` is never true there; the window is still enforced by the run's own `Timer` sleep, which drops the tasks. An app's own task that budgets its work by `remaining()` is misled on the web. | **Open**: document it on `Deadline`, or read the host's clock for the window on wasm. |
| L3 | Low | `crates/undra-cli/templates/ci/android.yml` | The generated workflow uploads `build/symbols/` (with the **unstripped** `.so`) as a GitHub Actions artefact on every push; for a public repository that artefact can be downloaded by any signed-in user for 90 days. Only symbol artefacts are uploaded (never the shipped binary), as the brief requires. | **Open**: a comment and a short `retention-days`, or upload to the crash reporter instead. |
| L4 | Low | ADR-046 amendment item 14, `bench/RESULTS.md`, `.10x/decisions/sde/prod-ops.md` | "The iOS linked app byte-identical" with and without `--no-symbols`: same size, but the test's two projects live in different directories, so 66 bytes of panic-location paths differ and shift the rest (see surface 4); the test itself asserts size only. | **Open** (wording): "the same code and size; byte-identical when built from the same path". |

## Decision D1, the JavaScript up front

The rule (R9): growth is allowed only for behaviour a hello app gets at load. Item by item (the amendment's ablation, gzipped, overlapping):

| Item | Bytes | Decision |
|---|---|---|
| `pagehide` and `freeze` listeners | 80 | up front: registered at load; a chunk fetch at those events is unreliable |
| the page's background window (its `stats()` read at every hide) | 80 | up front: runs at every hide of every page |
| `panicReports` and `background` stats fields | 76 | up front: specified `stats()` surface on every platform; the window reads `background.pending` |
| the trap-report loader | 130 | up front: the builder and the module's SHA-256 must be there before a trap, and with `recovery` the report is built synchronously before the restart; the builder stays lazy |
| `runInBackground` | 112 | **lazy** (`background.js`): a hello core has no task, `background.pending` is 0, nothing calls it; the window fetches it at the first hide with work pending (hidden precedes `pagehide` and `freeze`), and the core flushes persistence on `Lifecycle.Background` itself |
| the `Diagnostics` registration | 56 | **lazy** (`ports` chunk): native cores only, whose ports chunk is fetched before the transport starts anyway |

On the branch alone: 21,756 -> **21,666**. On the merged tree: **22,005** against `main`'s 21,672 (objects-callbacks' record): this piece is
**+333**, the four kept items and M4's 4 bytes. The gate is **22,100** (the record rounded up to the next hundred; 22,000 would already fail).
Both pieces' bytes are in ADR-052's prod-ops review note: `main` before either 21,336; objects-callbacks +336; prod-ops +333.

## The attack, per surface

**1. R2, the new `unsafe` (`frames.rs`).** *The unwinder:* `capture` is called from the panic hook (`guard::hook` -> `capture_frames`) on the
panicking thread, before unwinding starts, so every frame it walks is live; the fallback (`report_from`, for a payload that bypassed the hook,
`resume_unwind`) walks after unwinding, at the catch site: also live, only shallower. `visit` reads the context it is handed and writes through
`arg`, which points at a `Walk` on `capture`'s stack for exactly the call; nothing in it can panic (an unwind out of an `extern "C"` function
aborts): `ip` is checked non-zero before `ip - 1`, and the offset is now `checked_sub`. *`dladdr` on a non-image address:* returns 0, so the frame
is left out; `dli_fbase` null is checked. *Image headers:* M1 (fixed); a non-Mach-O image on Android is the ELF path; a 32-bit Mach-O
(`arm64_32`) or a big-endian one gets an empty id; wasm does not compile `frames.rs`. *Two threads at once:* the source is stateless
(`OnceLock` for the base and the id), `_Unwind_Backtrace` and `dladdr` are thread-safe, the hook's recording is thread-local, and
`emit_report`'s re-entrancy flag is thread-local: two panicking threads each report once (`tests/abi.rs` reports from the caller's and the core
thread). *Re-entrancy:* a panic inside the hook would abort (std's "panicked while processing panic"); the hook calls no user code (the payload is
downcast, not formatted) and nothing in the report builder indexes or unwraps; the *delivery* is outside the hook, guarded, and a report made
while one is delivered is dropped, not recursed (`a_report_made_while_one_is_delivered_is_dropped_not_recursed`, a host whose
`Diagnostics.panicked` panics; `undra-ports`' `a_report_that_panics_is_not_reported_again`, a Rust binding that panics). *Stack:* the hook
symbolizes on the panicking thread's stack; a panic close to the guard page can overflow there, and a stack overflow is not a panic at all: on
native, Rust's handler prints and aborts (the OS crash reporter sees it; the release symbols resolve its frames); on wasm it is a trap with no FATAL
record, and the TypeScript report carries the trap's own text. *Miri:* the four image-reader tests pass under Miri (`cargo +nightly miri test -p
undra-ffi --lib`, permissive provenance); the unwinder and `dladdr` are FFI Miri cannot run. *ASan:* no Linux-shaped test links here; the C++ host
under ASan/UBSan is below.

**2. R6 and ordering.** One report per contained panic, at each site, on the Rust side: calls, async calls, detached tasks (`undra-ports`
`tests/diagnostics.rs`), the computed (`computed_isolation.rs`, new: the location comes from the hook through `caught_elsewhere`), init hooks,
event subscribers, background start/probe/run, a store's restore, a reporter that panics (`undra-runtime` `tests/panic_reports.rs`, new), the
`undra-ffi` entries and the async core thread (`tests/abi.rs`). The executor's `Host::schedule`, the timer thread's and the `Lifeline`'s wakers
use the same `report_current` and are not separately tested (a `TestRuntime` has a manual clock and no global runtime): read, not run. The
FATAL record and the report never block: `port_call_id 0`, the outcome ignored, nothing registered as pending, and an answer with id 0 is
dropped before it is counted (`tests/abi.rs` asserts the id). A throwing `onPanic` goes to `onError` in Kotlin and TypeScript (S29 step 4); Swift's
handler cannot throw. On the main thread and in order: Swift and Kotlin hop with a serial main queue; the TypeScript and React Native paths run
on the JavaScript thread. The wasm report has the native report's shape field by field (`panic-report.test.ts`, new: the same nine fields with
the same types, frames with the same four).

**3. Background runs.** The deadline is the runtime's monotonic clock plus a `Timer` sleep (R12; the query tests drive it with the manual clock);
a task that outlives the window is dropped (cancelled) and `stillPending` counts its work; a host that cancels the call drops the run
(`a_host_that_cancels_the_call_cancels_the_run_and_loses_nothing`). A replay in flight belongs to the client, not the run: the item is popped only
when answered and persisted per item, so a kill mid-run re-sends it with the same `Idempotency-Key` (S30 step 4 asserts one POST), and an
unreadable queue is read again, never overwritten (`an_unreadable_queue_is_read_again_by_a_run_and_replayed`). iOS: `BackgroundEngine`'s fakes
exercise expiry twice (`testExpirationCancelsTheRunAndCompletesOnceAsAFailure`, `testTheGraceExpiringCancelsTheDrainAndEndsTheTaskOnce`);
`BGTaskScheduler` itself does not run in a simulator. `android-work`: the 11 instrumented tests drive the worker over a **fake** core with
WorkManager's test driver (unique work, network constraint, retry, a stopped worker cancelling the run): they prove the WorkManager wiring,
not a real drain on a device. The web window: on hide, the core flushes debounced persistence at once (S30 step 1); a tab closed mid-flush loses
at most the write in flight (IndexedDB), the queue being persisted per item. L2 is the one wasm gap.

**4. Symbolication.** Re-run on the merged tree with `UNDRA_REQUIRE_TOOLCHAINS=1` (the booted iPhone 17 Pro simulator, the `undra` AVD,
the host, node): `symbols` 5 of 5 (iOS Release app: `lab.rs:222`, `:221` and the detached closure `:236`; Android with the unstripped twin; host
with the dSYM; web to the function) and `debugging` 3 of 3 (LLDB stops at `lab.rs` on the host and in a simulator process). `--no-symbols`: the
two linked, stripped iOS apps are the same size (2,289,752) but **not** byte for byte: `cmp` differs at 1.8 M positions, all from one cause, the
test builds the two projects in different directories (`symbols`, `symbols-plain`) and `__cstring` carries the 11 panic-location paths of the
playground's sources (66 bytes), which shifts everything after it; every other section has the same size. So the claim holds in substance
(the symbol work changes no code) and is literally true only for one project path; ADR-046's "byte-identical" should say so (L4). Home paths
(`grep -a` for `/Users/<name>`): none in the iOS app, its dSYM, the host dSYM, `<ns>.debug.wasm`, `.dwarf.wasm`, the function map, the
Android twins or `native-debug-symbols.zip`; the panic locations are remapped to `~/...`. The one place it remains is the xcframework's static
library: its debug map's `N_OSO` entries name the prelinked object by absolute path in the builder's `target/` (what `dsymutil` reads when the
app links; stripped from the app). CI: only `build/symbols/` and the dSYM are uploaded, never a shipped binary (L3).

**5. Schema and compatibility.** The standard surface's own hash is unchanged by the merge (`0x543d_0961_0867_e387`); the playground's moved
once more with objects-callbacks to **`0xcc36d9fa84455aef`**. `undra bindgen --check --docs` is up to date for the playground, cookbook,
fieldbook and two-cores a and b, and `--check` for the iOS 15 sample. A core built before this branch: every runtime compares the Hello's
schema hash before it registers its adapters (Swift `connect`, Kotlin, TypeScript `_start`), so it is refused with `UndraSchemaMismatchError`;
an old core never calls the `Diagnostics` port (it does not know it), and registering a port id a core does not know only fills a table.
Read, not run against an old binary.

## Counts (the merged tree)

| Suite | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -D warnings`; clippy `-p undra-ffi` (and runtime, ports, query) `--target wasm32-unknown-unknown`; `cargo doc -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` (`UNDRA_REQUIRE_TOOLCHAINS=1`, so it ran the symbol, debugger and build-system tests on the simulator and the AVD) | **3,229 pass, 0 failed, 18 ignored** |
| `schema_retention`, `schema_docs` `--ignored` | 2 pass |
| `undra-cli --test symbols` / `--test debugging` (inside the run above) | 5 / 3 pass |
| `undra-bench` budgets (release); `sync_alloc`, `commit_alloc`, `derived_alloc` (release) | 6 pass (1 ignored); 5 + 2 + 1 pass |
| wasm ABI harness; interop | 22 + 36 pass; `interop OK` |
| Miri, the image readers (`cargo +nightly miri test -p undra-ffi --lib`) | 4 pass |
| Swift runtime (`swift test`) | 772 pass |
| Kotlin runtime, brew Kotlin 2.4.20 and CI's 2.0.21, fresh build dirs | 810 cases, 0 failed, 2 skipped, both; testkit 32, both |
| `android-adapters` (debug + release) and `android-work`, JVM | 318 results, 0 failed, 2 skipped |
| Instrumented on the `undra` AVD: `android-adapters` / `android-work` | 149, 0 failed (5 skipped) / 11 pass |
| TypeScript runtime (61 files) / testkit / React Native unit, all typechecked | **1,681** / 37 / 104 pass |
| React Native contract column; C++ host under ASan/UBSan (`cpp/test/run.sh`) | 21 pass, S17 and S29 skipped (app-tested); exit 0, 113 checks, none failed |
| Contract grid (`contract-tests/run-all.sh`, Swift `.build` removed first) | **86 of 86**: TypeScript 30, Kotlin 28, Swift 28 (the full run had TypeScript S29 failing, M4; the TypeScript column re-run after the fix: 30 of 30) |
| `undra bindgen --check --docs` (playground, cookbook, fieldbook, two-cores a and b); `--check` the iOS 15 sample | up to date |
| `scripts/wasm-size.sh --record` | wasm 119,654 of 120,000; JS 22,005 of 22,100 |
| Playground web: vitest; build; Playwright smoke | 127 pass; built; 6 of 6 |
| Playground Android `assembleDebug` | BUILD SUCCESSFUL |
| Site `build-all` (idempotent on a second run), `check-links --words` | clean; landing prose 342 of 350 |

The TypeScript runtime's `fuzz.test.ts` ("5000 random byte arrays in every decoder") timed out at its 5-second default in two of four full
runs while the Rust, Swift and Kotlin suites were compiling beside it (3.2 s alone); it passes in every quiet run. Not this piece's code.

## Sizes

| Artefact | `main` (`f9a37a8`) | merged, as the piece landed | after the review | Gate |
|---|---|---|---|---|
| Hello web core, wasm gzipped | 119,055 | 121,164 (over) | **119,654** | 120,000 |
| Hello JavaScript up front, gzipped | 21,672 | 22,001 (after D1) | **22,005** (M4: +4) | **22,100** |

A contained panic with its report: **47.3 µs** p50, 39.6 µs with the port unbound (`panic_cost`, release, p50 of 400), measured while
two other agents were building; the record's 34.6 / 32.5 µs was a quiet machine. The report is about 8 µs of it here (2 µs in the record); no row is budgeted
(the hot path is untouched: the budgets and allocation gates pass).

## Open items

* L1, L2, L3, L4 above.
* `report_current` sites without a test of their own: the executor's `Host::schedule`, the timer thread's waker, the `Lifeline`'s wakers.
* A trap during `undra_init` before the lazily imported report builder arrives gets no `onPanic` report (the decision record's own open item).
* `docs/ONBOARDING.md`'s per-suite counts are the piece's pre-merge numbers except the contract grid's; the integrator refreshes them.
