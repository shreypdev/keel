# SDE: the device bench (branch `wt/device-bench`, 2026-10-01)

Track E1 of the v1.x program (`.10x/specs/2026-10-01-v1x-default-choice-design.md`): the blueprint's section 14 rows
measured **through the generated binding and the platform's mirror, in the playground app itself**, on the iPhone 17 Pro
simulator, an arm64 Android emulator and headless Chromium, with the tooling that makes it a command for the day a phone
is attached; Swift and Kotlin drain numbers for ADR-031; the `undra init` Android template adopting
`ChoreographerFramePacer`. Nothing in the Undra crates or runtimes changed; no ADR (no wire, ABI, runtime-model or
generated-shape change: the playground's schema gained one method).

## What exists

| Piece | Where |
|---|---|
| The command | `scripts/bench-device.sh --device ios\|android\|web [--target ..] [--runs N] [--quick] ...` (`--help` is the manual) |
| Result file and report | `scripts/bench-device-report.mjs` (`finalize`, `assemble`, `render [--check]`, `validate`, `get`), `scripts/bench-device-report.test.mjs` (14 tests, run in CI), `bench/device-targets.json` (the section 14 targets; a test checks them against `docs/blueprint.html`) |
| Results | `bench/results/device/2026-10-01-*.json` (iOS simulator x3, Android emulator x2, Chromium x2) and the device section of `bench/RESULTS.md`, rendered between markers (a test fails when it is not what the files say) |
| Core hook | `Bench.bench_list_update_burst(n)` in `examples/playground/core/src/bench.rs` (+ unit test), bindings regenerated (schema hash `0x04d2adf769c58b9f`) |
| Web | `examples/playground/web/bench.html`, `src/bench/` (`ops.ts`, `stats.ts`, `runner.ts`, `main.ts`, 10 vitest tests), `bench/bench.spec.ts`, `playwright.bench.config.ts`, `vite.bench.config.ts`; `npm run bench` |
| iOS | `ios/PlaygroundApp/Bench/` (`BenchRunner`, `BenchStats`, `DrainRecorder`, `BenchScreen`), `PlaygroundApp.swift` (`-bench full\|cold`), `ios/PlaygroundAppUITests/PlaygroundBenchTests.swift` (skipped unless `TEST_RUNNER_UNDRA_BENCH=1`) |
| Android | `android/app/src/main/kotlin/dev/undra/playground/bench/` (`BenchRunner`, `BenchStats`), `UndraApp.coreLoadNanos`, `app/src/androidTest/.../BenchInstrumentedTest.kt`, a `benchmark` build type in `app/build.gradle.kts` |
| Template | `crates/undra-cli/templates/android/{UndraApp.kt,app/build.gradle.kts}`, test `the_android_shell_installs_the_choreographer_frame_pacer` |
| Docs | `bench/RESULTS.md` (device section, method, findings), `docs/ONBOARDING.md` (two rows), `examples/playground/README.md` + `android/README.md`, ADR-031's consequences (dated entry), this note |

## How a founder runs it on a real device

```sh
# iPhone: USB, trusted, Developer Mode on; UDID from `xcrun xctrace list devices`, team id from Xcode > Settings > Accounts
UNDRA_IOS_TEAM=<team id> scripts/bench-device.sh --device ios --target <udid> --runs 3
# Android phone: USB debugging on; serial from `adb devices`
scripts/bench-device.sh --device android --target <serial> --runs 3
```

A target that is not a simulator/emulator is labelled `kind: "device"`, which is the only thing that turns on the verdict
column (`within` / `over, Nx` against `bench/device-targets.json`). **Those two paths are written and not exercised: no
phone was attached.** What was exercised on this machine: the simulator and emulator paths end to end (three iOS runs,
two Android, two web, each with ten cold launches), `xcresulttool export attachments` as the way a device hands back its
JSON (the script falls back to it when the xcodebuild log has no `UNDRA_BENCH_RESULT` line; it is the same code that ran
on the simulator), and `--help`/error paths. Not exercised: selecting a device by `xctrace`, `-allowProvisioningUpdates`,
`svc power stayon`, the battery notes, the script's own AVD boot (another agent's emulator held the `undra` AVD; see below).

## Decisions

* **A script, not `undra bench --device`.** The command needs a checkout (the playground's hooks, an XCUITest bundle in the
  Xcode project, an instrumented test in the Gradle project, Playwright), so as a CLI subcommand it would be a shipped
  command that works in one place, with help text, a docs page and a stability promise for something that is a repo tool.
  When user projects can generate bench hooks of their own, the subcommand is the right shape and this script is its
  engine; until then `scripts/` is where `bench-record-base.sh` and `wt.sh` live too.
* **The harness lives in the apps, the launcher outside.** A tap-driven UI test cannot time a 60 ns call, so each app
  has a runner (`BenchRunner`) and the XCUITest / instrumented test / Playwright spec only launch it and carry the JSON
  out (the accessibility labels of `BenchScreen` in pieces of 400 characters; the instrumentation status
  `undra_bench_json`; `window.undraBench`). The playground starts in benchmark mode only with `-bench` (iOS), only from the
  test (Android), only on `bench.html` (web, built apart, never in the site). Every use is public API (R10).
* **One result schema, three runners that must agree** (`undra-device-bench-raw/1`): the op ids live in
  `web/src/bench/ops.ts`, a node test greps every runner for them and for the 1,667 constant, `finalize` refuses a file
  that lacks a row, has a non-finite number, a p99 below its p50 or a missing drain.
* **Every operation is checked before its number is kept**: the sum of the replies, the length of the list and the id at
  the insert position, the hundredth counter, the 1 KB payload, and the drain's change-set counts. A run in which the apply
  did not happen reports nothing. (The web runner's unit test caught its own frame window swallowing the single-entry drains.)
* **Timing**: batches of calls divided by the batch for the handle call (and for every web row: Chromium rounds to 100 us,
  5 us when cross-origin isolated, which `vite preview` turns on for the bench page), one operation at a time on the phones
  for the rest, 2,000 samples after a warm-up; the file records the clock's step and the cost of a pair of reads. The 1 KB row is the
  playground's `Bytes` echo, not a record: stated in the table. On the web the generated call is async, so there are two
  handle-call rows and the blueprint's in-thread target goes on the runtime's `callSync`.
* **The drain (ADR-031)** needed a producer that is not the main thread, so `bench_list_update_burst` was added to the core
  (a schema change, bindings regenerated; contract scenarios 54/54 still pass). iOS and Android: a thread commits the
  1,667-patch burst once per frame and the drain at the next frame is timed by the drain listener, through the real
  `CADisplayLink` / Choreographer; web: the page's own thread (so the burst is main-thread time, said in the row).
  **The unmerged cost is estimated from the entry cost, the runtime is not reverted**: after each frame, 8 calls on the main
  thread each drained alone (iOS, Android), a pair of batches (web), times 1,667, by the median and by the mean entry. The first
  version measured the single-entry cost in a block after the frames; the median moved 2.7x between two iOS runs, so it
  was moved inside the loop (the same minutes for both) and the runs now agree within 4% to 9%.
* **Cold start**: iOS (fresh process each launch via the XCUITest, ten times: the first `UndraCore.load` and the restore of
  the snapshot the full run left, plus 30 in-process shutdown/load/restore cycles); Android (a fresh `am instrument` process each:
  `UndraApp.onCreate` times the load, which includes `System.loadLibrary`; an in-process core cannot be loaded twice, so
  its in-process figure is the restore alone); web (a fresh browser context each: fetch, `WebAssembly.compile`, `UndraCore.load` of
  the compiled module = the blueprint's "after wasm compile"; no restore: TypeScript has no `snapshot()`/`restore()`, and
  the row says "load only"). The 100 KB snapshot is 1,000 to-dos of 80 characters (101,046 bytes).
* **Labels cannot be dropped by accident**: a result that is not from a device carries the not-a-device claim verbatim and
  `validateResult` rejects a file that edits `kind` or the claim; the renderer gives a verdict only to `kind: "device"`; the
  web is `browser`, which never gets one either (the blueprint names no reference machine for Chromium); the "Pending
  hardware" table stays until an `ios:device` and an `android:device` file exist.
* **Isolation on a shared machine.** Another agent was running the `undra` AVD (`emulator-5554`, not read-only), so a second
  instance of it could not start. For this piece an AVD `undra-bench` (a copy of `undra`'s `config.ini`: pixel_7, android-35
  google_apis arm64-v8a, 4 cores, 2 GB) ran on port 5580, and a simulator `undra-bench iPhone 17 Pro` (same device type and
  runtime) was created so the other agents' `iPhone 17 Pro` was never booted, shut down or tested against. The script's
  labels use the device type's name, so the files say "iPhone 17 Pro simulator" either way. Both clones were deleted at the end.
  The script itself never picks an emulator when several run (it asks for `--target`).

## Numbers (details in `bench/RESULTS.md`; none is a device)

iOS simulator / Android emulator / Chromium, p50 over the runs in the files: handle call 294 to 302 ns / 251 to 266 ns /
3.2 to 3.5 us (3.5 to 3.9 us `callSync`); 1 KB 0.46 to 1.1 us / 1.3 to 1.4 us / 4.6 to 5.3 us; keyed insert 8.0 to 8.8 us / 23 us /
20.6 to 22.6 us; 100-signal change-set 26 to 28 us / 19.6 to 20.3 us / 88 to 96 us; cold start 2.2 to 2.7 ms / 1.4 ms / 4.2 to 4.5 ms
(load only). Drain, merged frame vs estimated unmerged: 0.79 to 0.82 ms vs 3.4 to 4.1 ms / 0.17 to 0.18 ms vs 20.9 to 24.7 ms /
4.0 to 4.3 ms vs 11.3 to 12.1 ms.

## Findings for the integrator

1. **The generated binding, not the core, is the handle call**: 300 ns on iOS and 250 to 266 ns on Android on cores faster than
   the target devices', against a 44 ns core-side call; and **44 to 49x over the web's in-thread 80 ns on a desktop
   browser**. A decision is needed on the web row (count only the wasm export, or make the TypeScript path cheaper: a
   `Uint8Array` and `encodeTarget` per call, the mirror flush, the reply parse). Same family of fix as ADR-028 for the core half.
2. **Kotlin's per-patch list copy is the case for ADR-031**: estimated 20.9 to 24.7 ms of main-thread time a frame at
   100,000 patches/s on 10,000 rows without merging (more than a 60 Hz frame), 0.17 ms with. E2 (derived keyed lists) should
   assume this number.
3. **The platform's cold start is ~30x the core's** (2.2 ms iOS load vs 70 to 80 us on the host): adapters, the C ABI init, the
   core thread, the schema check. iOS is at 0.74 to 0.90 of its 3 ms target on a faster core than an A15's: watch it.
4. **TypeScript has no public `snapshot()`/`restore()`** (SPEC 17.1 lists none; contract S15 and S17 go through wasm exports):
   a parity-audit item (C4) and the web crash-recovery row's blocker. An in-process Kotlin core cannot be reloaded (one core
   per process) while Swift's can; also a parity note.
5. **Noise is real**: the iOS 1 KB row was 458 ns in one run and 1.04 to 1.13 us in two others on an otherwise identical
   build; web numbers moved 10 to 15% between a load-4 and a load-3 pair. The files carry every run and the host's load.
6. Playwright's `devices["Desktop Chrome"]` descriptor spoofs a Windows user agent; the bench config does not use it.
7. In this environment `simctl io screenshot` could not overwrite the existing `.proof/ios-*.png` (EPERM; the files carry an
   extended attribute), so `ios/smoke.sh` failed at its first screenshot until they were removed (not touched here: remove
   `.proof/ios-*.png` first if it happens to you; `git checkout` restores them afterwards).

## Verification (this tree)

`cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace` **2,238 passed,
0 failed, 10 ignored**; `UNDRA_TEST_ANDROID=1 cargo test -p undra-cli --test platforms` 4 passed (and, in an earlier pass over the same
template, `UNDRA_TEST_ANDROID_APP=1` built the generated project's debug APK through the composite build with
`android-adapters`: 11.9 MB); `node --test scripts/bench-device-report.test.mjs` 14 passed; web `npm test` 112 passed (10 new),
`npm run typecheck` clean, `npm run smoke` 4 passed; iOS `smoke.sh` PASS (5 tour tests, the bench test skipped as designed, no
fault or crash lines); Android: `:app:assembleDebug` and all four tabs launched on the emulator with no crash in logcat;
contract scenarios 18 x 3 pass (54/54) on the regenerated bindings. The three device runs: web x2, iOS x3, Android x2, every one
from a clean tree at a stated commit, agreeing as in the reproducibility lines of RESULTS.md.

## What the integrator owns

* Merge; then `.10x/status.md` / `handoff.md` (this branch touched neither): the E1 line of the v1.x queue is done except the real-device rows,
  which are one command each (above) and need a phone.
* The web handle-call decision (finding 1) and, if wanted, a hello-world size measurement on release targets for the other
  section 14 rows (still host proxies).
* `site/` (the Kotlin API page's `UndraApp.kt` example, `llms-full.txt`, the search index) still shows the pre-pacer template
  snippet; I did not touch the site's generated files.
* The CI step added to `ci.yml` (`node --test scripts/bench-device-report.test.mjs`) has not run on GitHub.
