# SDE — React Native runtime (wt/react-native, 2026-10-01)

Implements ADR-038 (React Native is a fourth host of the C ABI), v1.2 bet G1. No wire, C ABI, wasm
ABI, schema or generated-code change; nothing in `runtimes/ts/@undra/runtime` changed.

## What changed

**`runtimes/rn/@undra/react-native`** (new package):
* `cpp/UndraApi.h`, `cpp/UndraApiLinked.cpp` (iOS), `cpp/UndraApiAndroid.cpp` (`dlopen`): the C ABI as
  one struct, the only files that name a core symbol (ADR-044 migration point); `cpp/undra.h` (a copy
  a test keeps byte-identical to the Swift runtime's).
* `cpp/UndraHost.{h,cpp}`: the JSI-free host: `start` (ports registered before `undra_init`, native
  Clock/Rng/Log, JS async and sync routing), the inbox (`kind u8, len u32, payload` records, one mutex,
  wake through a posted function), `CallScope` (the JS thread inside a host function), `shutdown`, one
  running host per process, counters.
* `cpp/UndraJsi.{h,cpp}`: `Binding::install` (`__undraNative`'s host functions), the drain (outermost
  only, 1000 rounds, then a re-post), `CoreBuffer` (an `UndraBuf` as an `ArrayBuffer`, freed once),
  `JsAnswerer` (JS sync ports), reload handling (no JSI value held; a stale core stopped on `start`).
* `cpp/UndraTurboModule.{h,cpp}`, `ios/UndraModuleProvider.mm`: the `UndraNative` TurboModule.
* `cpp/UndraFrameSource.{h,cpp}` (AChoreographer, Android), `ios/UndraFrameSource.mm` (CADisplayLink).
* `src/`: `NativeTransport` (`transport.ts`), `loadNative` / `installNative` (`load.ts`),
  `nativeFrameScheduler` (`frame.ts`), `reactNativeAdapters` (`AppState` lifecycle), the
  `UndraNativeModule` type and `portPlan` (`native.ts`), the text codecs and their side-effect-only
  installer (`text-codec.ts`, `polyfills.ts`), the codegen spec (`specs/NativeUndra.ts`).
* `UndraReactNative.podspec`, `android/CMakeLists.txt`, `react-native.config.cjs` (a pure C++
  dependency), `codegenConfig` in `package.json`, `babel-plugin.cjs` (`import.meta` for Hermes).
* Tests: `test/*.test.ts` (36, a fake module), `test/contract/` (the contract scenarios through
  `NativeTransport` over `WasmNative`, a stand-in on the wasm core: 17 pass, S17 app-tested),
  `cpp/test/host_test.cpp` + `run.sh` (13 checks against the real core, per shim, ASan + UBSan).

**`crates/undra-cli`**: `undra build --platform rn` (`builds/rn.rs`): the iOS and Android builds and
`build/ios/UndraCore.podspec` (vendored XCFramework; `-force_load` of the SDK's slice through the pod
setting `UNDRA_CORE_SLICE`, because a clean Xcode build refuses a linker input under
`PODS_XCFRAMEWORKS_BUILD_DIR` that does not exist yet). Tests for the podspec and slice names.

**`contract-tests/ts`**: S03 accepts mode `native` next to `wasm-main`; S12 waits for the GET of a
stale observer and of `refetch()` instead of expecting it before the call resumes (the Swift and
Kotlin columns do the same). TS column 18/18.

**`examples/playground/rn`** (new): a bare React Native 0.87 app (TypeScript, Hermes, New
Architecture): Todos and 10k-list screens over the generated bindings and `@undra/runtime/react`, a
Bench screen with on-device checks RN01..RN10 and the ADR-038 measurements, `UNDRA-RN` log lines;
Metro serves the runtime, the module and the bindings from their sources; Podfile with the `UndraCore`
pod; Gradle `jniLibs` from `build/android`.

**Docs**: ADR-038 (decision, measurements, what is tested where, ADR-044 transitional notes),
`docs/REACT_NATIVE.md`, SPEC §0, §11, §11.1, §11.2 (new), §13, §17.4 (new).

## Proof

Release core (`undra build -C examples/playground --platform rn --release`), release apps:
* iPhone 17 Pro simulator (iOS 26.5): `UNDRA-RN CHECKS 10/10 passed`; Todos seeded through the core
  (`todos=3 visible=3 remaining=2 first.done=true`); 10k list insert and update applied in 7.6 and
  8.2 ms; Bench screen 10/10. Debug core: the same 10/10.
* Android emulator (arm64, API 35, a dedicated AVD `undra-rn`: the `undra` AVD was in use by another
  agent): `CHECKS 10/10`, a to-do typed through the UI, list operations, the stream switch updating
  rows, native vsync source confirmed. A debug build on Metro reloaded mid-benchmark: the core was
  shut down with the runtime and a fresh one started in the same process (`CHECKS 10/10` twice).
Numbers: ADR-038, Consequences.

## Deviations and notes

* `requestAnimationFrame` measured at 60 Hz on both (React Native's timers tick on the display); the
  native vsync source was kept as the contract (ADR-038 decision 6, alternatives).
* Ports are registered before `undra_init` (the Swift host registers after); a second Undra host of
  another language in the same process is unsupported until ADR-044.
* Hermes per-call costs are the TypeScript runtime's JavaScript: `encodeCall` alone is 4.2 us of a
  6 us `callSync` on the simulator; JSI plus the core is 0.23 us. A follow-up for `@undra/runtime`.
* Machine changes: CocoaPods 1.17.0 installed with Homebrew; Android SDK `platforms;android-37.0`,
  `build-tools;37.0.0`, `cmake;3.22.1` installed with `sdkmanager`; AVD `undra-rn` created (a copy of
  `undra`'s configuration).

## 2026-10-01 (later): the merge of `main`, CI for the runtime, the Android reload re-run

Branch `wt/react-native`, after the opus review (`.10x/reviews/2026-10-01-react-native-review.md`) and its fixes
(`1e695e7`). No change to the wire, the C ABI, the schema, or the generated code.

### The merge of `main` (43348eb: schema-json, device-bench, diagnostics, dev-loop, `cas_update`, Rust 1.98.1)

One textual conflict, `.10x/decisions/sde/_index.md` (both sides appended lines): kept every line. `build.rs`,
`cli.rs` and `SPEC.md` merged cleanly (the `--platform rn` arm and the dev-loop edits touch different lines).
`undra bindgen -C examples/playground --docs` wrote nothing ("0 written, 27 unchanged"); `node
site/scripts/build-all.mjs` changed nothing until the `rn` row below.

One cross-branch interaction the merge could not see: main reworded the `undra_schema_json` comment in the Swift
runtime's `undra.h` (65c8528), and the package's own test ("`cpp/undra.h` is byte-identical to the Swift
runtime's") failed. The copy was synced (comment only, no ABI change). That test is what a mirror needs.

Matrix after the merge (the tip of the branch, this machine): `cargo fmt --check` clean; `cargo clippy --workspace
--all-targets -- -D warnings` clean; `cargo test --workspace` 2,333 passed, 0 failed, 11 `#[ignore]`d; `bash contract-tests/run-all.sh` 54/54 (ts, kotlin, swift x 18); in
`runtimes/rn/@undra/react-native`: `cpp/test/run.sh` 14 + 14 checks and the JSI compile check, `npm test` 39/39,
`npm run typecheck` clean, `npm run test:contract` 17 pass + S17 skipped; the playground app's `npm run
typecheck` clean; `scripts/rn-device-checks.sh ios` (iPhone 17 Pro simulator, Release) and `... android --target
emulator-5560` (`undra-rn`, release APK) each `UNDRA-RN CHECKS 10/10 passed`.

### CI

* `.github/workflows/ci.yml`, job `react-native` ("React Native (host + model)"), ubuntu-24.04, Rust 1.98.1, Node
  20: `cargo build -p undra-cli`; the playground core for the host and the web; `npm ci` in the runtime, the
  package and (with `--ignore-scripts`, for the 0.87 headers) the playground app; `cpp/test/run.sh` with
  `CXX=clang++-18` and `UNDRA_RN_REQUIRE_JSI=1` (a missing header set is a failure, not a skip); `npm test`;
  `npm run typecheck`; `npm run test:contract`.
* `.github/workflows/rn-devices.yml`: `workflow_dispatch`, Mondays 05:17 UTC, and pull requests touching
  `runtimes/rn/**`, `examples/playground/rn/**`, the script or the workflow. `ios`: macos-15, the newest stable
  Xcode installed, `undra build --platform rn --release` (which also builds the Android half, hence the
  NDK r27.2 and `cargo-ndk`), `pod install`, `xcodebuild` Release for the simulator, `simctl`, wait for the
  verdict. `android`: ubuntu-24.04, the core and the release APK for x86_64 first, then the emulator (API 34,
  x86_64, KVM) through `reactivecircus/android-emulator-runner`. Both run `scripts/rn-device-checks.sh`, the same
  recipe a developer runs.
* Concurrency: `group: rn-devices-<ref>` and `cancel-in-progress: true` for a pull request only; a
  dispatched or scheduled run has a group of its own (`run_id`) and is never cancelled. There is no `push`
  trigger, so the bench workflow's mistake (a merge's run cancelled by the state commit that follows it,
  runs 36803857331 and 36808385622) cannot happen here, and the guard is in place if a `push` trigger is
  ever added. `ci.yml` has no concurrency group, so its `react-native` job is never cancelled.
* Deviation from the brief: `macos-15`, not `macos-14`: every macOS job of this repo's `ci.yml` is `macos-15`, and the Xcode on
  the image must be recent enough for React Native 0.87 (not looked up; the job selects the newest stable Xcode
  installed, so it does not depend on the image's default).

Proof that the Linux job's steps run on this branch's content: `git clone --local` of the worktree into a
scratch directory, then the job's own `run:` steps read out of `ci.yml` and executed in order (Node 20.20.2, the
workflow's `RUSTFLAGS=-D warnings`), skipping only the `apt-get` step and substituting Apple's `clang++` for
`clang++-18`. The first two runs failed, in the clean clone only, and found two defects that the worktree hid:

1. `npm run typecheck`: `vitest.contract.config.ts` imports `contract-tests/ts/src/reporter.ts`, whose
   `vitest/node` types resolve only if `contract-tests/ts/node_modules` exists. Fixed: `tsconfig.json` maps
   `vitest` to the package's own install.
2. `tsc -p tsconfig.build.json` resolves `@undra/runtime` to `runtimes/ts/@undra/runtime/dist/index.d.ts`, which
   is not committed. Fixed in the job: `npm ci && npm run build` in the runtime first.

The third run passed every step (core builds 21 s, `cpp/test/run.sh` 18 s with both shims at 14 checks and the
JSI check, 39 unit tests, typecheck, contract 17 + 1 skipped). What this does not prove: that a GitHub Linux
runner has `clang-18` + `libclang-rt-18-dev` from the apt step, that ASan starts with the runner's kernel
(`vm.mmap_rnd_bits=28` is set defensively), and that `libstdc++` accepts the C++ (`host_test.cpp` was relying on
libc++ for `std::unique_ptr`, which is now included explicitly). The first run on a runner is the test of
those; `rn-devices.yml` has not run anywhere but as the script it calls, on this Mac. `actionlint` is not
installed: the two workflow files were parsed with Ruby's YAML and checked for structure (every job has
`runs-on`, every step `uses` or `run`, balanced `${{ }}`).

Script fixes found by running `scripts/rn-device-checks.sh` for real: CocoaPods stops without a UTF-8 locale
(the script sets one); Xcode does not track the pod's `-force_load` of the core as a link input, so after a
core rebuild the app is dropped and relinked (its own intermediates, not the Pods').

### Android reload, re-run with the review's fixes

Native code under test: the tip of the branch (`UndraJsi.cpp` with the review's fixes, `UndraHost.cpp` with the
slot wait). Debug build on Metro (`npx react-native start`, `adb reverse tcp:8081`), `undra-rn` AVD (arm64, API
35) on port 5560, reloads by `curl -X POST localhost:8081/reload`, evidence from `adb logcat ReactNativeJS`.
The drivers are throwaway scripts (not committed); the commands are in the sequence below.

* Sequence 1 (one process, pid 3371): launch, an idle reload, two reloads in the middle of the benchmarks, then
  two in quick succession. `loaded=6`, `UNDRA-RN CHECKS 10/10 passed` 6 times, 0 `CHECK ... FAIL`, 0 `UNDRA-RN
  failed|error`, 0 `Fatal signal`/`FATAL EXCEPTION` in logcat, pid unchanged.
* Sequence 2 (pid 3594): three reloads timed 1.7 s after the runtime loaded, inside its self-checks: `loaded=5`,
  the interrupted runs print no verdict, the last one `CHECKS 10/10`, no crash.
* Sequence 3 (pid 3712): 20 reloads in a row, each after the verdict line: `loaded=21`, 21 x `CHECKS 10/10`, none
  other, no crash. `dumpsys meminfo` (Native Heap, KB; Heap Alloc / total PSS): launch 110,987 / 237,536;
  after 5 reloads 98,795 / 220,584; 10: 99,941 / 222,334; 15: 100,049 / 223,352; 20: 99,413 / 222,476.
  Flat within a megabyte of noise; it cannot resolve a 100-byte leak.
* The frame-source note (review I1: a `new std::weak_ptr` per `AChoreographer_postFrameCallback`, never freed if the
  looper quits first): measured with a temporary counter (not committed) in `UndraFrameSource.cpp` that logged,
  when a source was destroyed, whether a callback was pending and the process totals of callbacks posted and run.
  21 reloads in a row (all `pending=0`, `posted == ran`), the 5 reloads of a sequence-1 style run, and 40
  reloads each made while the 10k list's Stream was on (`adb shell input tap`, frames requested 5 to 6 times a
  second): 66 sources destroyed, 0 with a callback pending, `outstanding=0` every time (e.g. `posted=1524
  ran=1524` at the last). So the leak path exists in the code but was not reached once in 66 reloads; it needs
  a frame requested in the instant of the teardown, costs about 100 bytes (a `weak_ptr` and the control block of
  the state it keeps alive), and is development-only. Recorded in `docs/REACT_NATIVE.md` rather than fixed: the
  fix (an id in place of the pointer, a table the source's destructor empties, a host test for the table)
  is bigger than the thing it removes, and no measurement asks for it. If a counter ever reads `pending=1`, that
  is the design.
* Nothing in the sequences is Android specific except `adb`, `logcat` and the tap coordinates (1080 x 2400:
  toast dismiss 996,2208; the 10k list tab 540,2285; the Stream switch 510,542).

### Other review items

I2..I6 stay as notes (unsupported configurations or properties the documentation states). I7 is done: the `rn`
row in `site/docs/cli.html` (search index and `llms-full.txt` rebuilt, links checked); there is still no React
Native page on the site.

### Machine

The `undra-rn` AVD was booted on port 5560 and shut down at the end; Metro on 8081 stopped; the `undra` AVD
(emulator-5554) was not touched. A Node 20.20.2 binary was installed into the scratch directory with `npm i node@20`
for the clean-clone run. The shared scratchpad directory is shared between agents: another agent overwrote a
`env.sh` this work had put there, so its files live in a subdirectory.
