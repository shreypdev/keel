# Device bench (E1) — adversarial review

**Date:** 2026-10-01 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/device-bench` at `edcb9e0` (12 commits on `main` at `549b1f2`) · **Read:** `CLAUDE.md` (R9, R10, R12), the SDE record
`.10x/decisions/sde/device-bench.md`, `bench/RESULTS.md` (device section, method, findings), `bench/device-targets.json`,
`bench/results/device/*.json`, `scripts/bench-device.sh`, `scripts/bench-device-report.mjs` + test, the three runners
(`ios/PlaygroundApp/Bench/*`, `PlaygroundBenchTests.swift`; `android/.../bench/BenchRunner.kt`, `BenchInstrumentedTest.kt`,
`UndraApp.kt`; `web/src/bench/*`, `bench/bench.spec.ts`, the two bench configs), `core/src/bench.rs`, the regenerated
bindings, the three runtimes' call and drain paths, ADR-031's appended entry, the `undra init` Android template and its
test, `ci.yml` / `bench.yml` · **Fixes:** `80a4058`, and wording corrections in the commit that adds this file.

## Verdict

The iOS-simulator and Android-emulator rows may be published as labelled: every timed region is the generated store
call, the crossing and the apply to the mirror on the main thread, nothing else (no logging, no checks, no resets
inside the clock; warm-ups are discarded), each operation's effect on the mirror is verified (per operation on the
phones, cumulatively per batch on the web), the renderer gives a verdict only to `kind: "device"`, and no sentence
claims a device target; two harness costs sat inside the cold-start clocks (the iOS Kv-directory removal, the Android
`runOnMainSync` hop), both of which can only make a row slower, and both are now outside. The web rows may be published
only with the disclosure added here: they measure the runtime **as Vite's default build target lowers it** (every ES2022
`#private` member becomes a `WeakMap` entry), which is a large share of every web row (the same page built at `es2022`:
handle call 1.8 us against 5.6 to 6.0, change-set 31 us against 173 us, keyed insert 22.5 against 36 us, at a higher
load), so "the web's 1 KB row is over
its target" and finding 1's cause list ("a `Uint8Array` per call, the target encoding, the mirror's flush") described the
build, not the code. Before this review the tooling could also label an emulator as a device (an `adb connect`ed
emulator has no `emulator-` serial) and render verdicts for it; `finalize` now refuses a device label the runner's own
facts contradict. The absolute numbers are a quiet-host snapshot (load 2 to 10): my reruns at load 19 to 94 came out
1.2x to 2.0x slower with the same shape.

## Findings

| # | Sev | Where (at `edcb9e0`) | Finding | Status |
|---|---|---|---|---|
| H1 | High | `scripts/bench-device.sh:250-251`; `scripts/bench-device-report.mjs:145-156` | Android `kind` came from the serial alone (`emulator-*` → emulator, anything else → **device**). An emulator reached by `adb connect` (`127.0.0.1:5555`, Genymotion, docker/cloud emulators) is a "device", and `kind: device` is the one switch that turns the verdict column on: an emulator's rows would read "within". `validateResult` checked only the claim string, never the runner's own `device.is_virtual`, nor that a web result is never a device. | **Fixed**: `validateResult` refuses `kind: device` unless `device.is_virtual === false` (absent counts as virtual), and a kind outside its platform's set (ios: device/simulator, android: device/emulator, web: browser); the script also reads `ro.kernel.qemu`, `ro.boot.qemu`, `ro.hardware` (ranchu/goldfish/vbox86); the Kotlin heuristic gained vbox86. Tests: edited-kind file, TCP emulator, silent runner, web device |
| H2 | High | `examples/playground/web/vite.bench.config.ts:15` (no `build.target`); `scripts/bench-device.sh:337`; `bench/RESULTS.md:769, 774`; ADR-031 entry | The bench page inherits Vite's default target (es2020/safari14), which lowers the runtime's `#private` fields and methods to `WeakMap`/`WeakSet` helpers (78 `new WeakMap` in the bench bundle, 0 at `es2022`). That is 70-80% of the web handle call (see E4 below) and 1.6x (keyed insert) to 5.5x (change-set) on the web rows: same harness, `build.target: "es2022"`, load 27-29: `sync_call` 1.83 us, `callSync` 1.61 us, 1 KB 3.29 us, keyed insert 22.5 us, change-set 31.4 us, merged drain frame 2.4 ms, against 5.6-6.0, 5.7, 8.0-8.3, 35.7-37.0, 173-174 us and 5.9-6.2 ms for the default build at load 19-22. The label said "vite production build", the findings attributed the cost to the code, and "the web's 1 KB row is over its target" is a property of the build target. | **Fixed (disclosure)**: the script's web app label names the target and the lowering; the two committed web files carry a review annotation in `notes` (no number changed) which the block renders; RESULTS.md method bullet, findings 1, 2, 4 and ADR-031's entry corrected. **Which build to publish is E4's call** (both, or the template's default plus `es2022`) |
| M1 | Medium | `crates/undra-cli/templates/android/app/build.gradle.kts:58` | What an external user gets: `undra init` outside a checkout (no `--undra-path`) writes `implementation("dev.undra:runtime:0.1.0")` **and now** `implementation("dev.undra:android-adapters:0.1.0")` against `google()`/`mavenCentral()` with no `includeBuild`; neither coordinate is published (Maven Central is deferred to after v2), so Gradle fails to resolve (it already failed on `runtime`; this adds a second, an AAR, which needs its own publication setup). With `--undra-path` (checked: `includeBuild(".../runtimes/kotlin/undra-runtime")` is written) the composite resolves it when the runtime build finds an SDK (ANDROID_HOME/ANDROID_SDK_ROOT or `local.properties` of the runtime build or of the directory Gradle was started in) - same as the playground. The gated test covers the in-checkout case only. | **Open**: the Maven publication plan must list `android-adapters` (AAR: `maven-publish` + `android { publishing { singleVariant("release") } }`), and until then `undra init`'s help/README should say registry mode does not resolve on Android |
| L1 | Low | `scripts/bench-device-report.mjs:302` | Verdict `over, ${(p50/target).toFixed(1)}x`: a device row at 1.04x its target printed "over, 1.0x"; nothing said the verdict is on the p50 (the blueprint gives no percentile). Checked with fake device files: 60.4 ns vs 60 now "over, 1.01x", exactly on target "within", 2.04x, 12.3x, cold 3.01 ms "over, 1.01x". | **Fixed** (`overRatio`, rounds up; the block's intro states p50) + test |
| L2 | Low | renderer intro; `RESULTS.md` | p50/p99 of a batched row are over batch means, so the web rows' and every handle-call row's p99 is not a per-call tail; said once in the method, not where the numbers are. | **Fixed** (block intro) |
| L3 | Low | `scripts/bench-device-report.mjs:378` | Drain header "Unmerged frame, estimated (median to mean entry)" over `range()`, which prints min to max (the web's mean entry is below its median, so the order was reversed). | **Fixed** (header) |
| L4 | Low | `ios/.../BenchRunner.swift:66` | `loadCore()` emptied the Kv directory (`FileManager.removeItem`) inside every timed load: about 10 us when absent, 90 us with a file (measured on this host). Under 1% of the cold load, about 3% of the 337 us warm reload; only ever slower. | **Fixed** (`emptyKvDirectory()` before each clock; quick iOS run on a cloned simulator passes) |
| L5 | Low | `android/.../BenchRunner.kt:148, 365` | The restore (cold launch and in-process) was timed on the instrumentation thread around `runOnMainSync { restore }`, so it included the hop to the main looper and back; only ever slower (iOS restores on the main actor directly). | **Fixed** (`timedRestoreOnMain`; `:app:compileBenchmarkKotlin` + androidTest compile; not re-run on an emulator) |
| L6 | Low | `bench/RESULTS.md:615` | "after every operation the runner compares the mirror's state" - true on iOS and Android; the web checks after every batch (cumulatively: the reply sum, the list length, the counter, so a missing apply still fails). | **Fixed** (wording) |
| L7 | Low | `scripts/bench-device.sh:87` | `--quick` wrote its file under `$WORK`, which the exit trap deletes, then printed "results in <deleted dir>". | **Fixed** (own `mktemp -d`) |
| L8 | Low | `scripts/bench-device-report.test.mjs:252` | The blueprint check was `html.includes("≤ 60 ns")` anywhere in the page: swapped iOS/Android numbers, or a number from another row, passed. | **Fixed**: parses section 14's table and checks each target in its row and column (header order asserted) |
| L9 | Low | `bench/RESULTS.md:756` | "a row moves by 5% to 30% between runs" understated load sensitivity: see Reproducibility. | **Fixed** (a sentence with the reruns) |
| I1 | Info | `undra bindgen --check` | Without `--docs` the check reports all twelve playground files stale (they are generated with docs); `undra bindgen -C examples/playground --check --docs` is clean at `0x04d2adf769c58b9f`. | Note |
| I2 | Info | `bench/results/device/*.json` `finished_at` | It is the time of `finalize`, not of the measurement; the gaps (iOS 62 s, web 13 s, Android 7 s between runs of `--runs 2`) fit the script's sequence (a web run took 19 s here at load 20), Android's is tight but possible on an AOT-compiled emulator. | Note |
| I3 | Info | `.10x/decisions/sde/device-bench.md` finding 1 | The record's cause list for the web call is superseded by E4 below (not edited: the record is the author's). | Note |

## The attacks

1. **What each row measures - holds, with L4/L5.** iOS (`BenchRunner.swift:139-209`) and Android (`BenchRunner.kt:163-226`)
   run every row on the main thread; the clock pair brackets only the generated call, which returns after the immediate
   drain (Swift `UndraCore.callSync` → `Mirror.withImmediateDrain`, TS `#onReply` → `queueFlush` before `resolve`, so the
   apply is inside the `await`); checks, list resets and JSON are outside. `sync_call` is batched (1,000; 200 samples
   after 20/30 warm-up batches); the other phone rows are per operation with a 41.67 ns step and a 7.7 ns (iOS) / 22 ns
   (Android) read pair, small against 0.46 to 28 us. The web (`runner.ts:100-117`) batches everything: with
   cross-origin isolation `performance.now()` steps 5 us (`resolution_ns` 4,999.99 in the files, `crossOriginIsolated:
   true`), and a batch lasts 2.3 ms (1 KB, the shortest) to 19.5 ms (`callSync`), so a batch mean is resolved to better
   than 0.25%. **The 3.2 us web p50 is valid as a mean cost per call; its p99 is a p99 of batch means, not a call's tail (L2).**
   The `await` per call is what generated TypeScript costs an app (the binding is async), and a microtask hop is 18 to
   44 ns of it. The mirror check: per operation on the phones (rows count, the id at 5,000, `s099`), per batch on the web
   (L6).
2. **Labels and verdicts - held for the committed files, H1 for the tooling.** The renderer's verdict is `none (not a
   device)` / `none (no reference machine)` unless `kind === "device"` (`verdictFor`); every committed file is
   simulator/emulator/browser with the verbatim not-a-device claim and `is_virtual: true` on both phones; headings say
   "A simulator, not a device" / "An emulator, not a device" / "A desktop browser, not a device"; targets are in a
   "Blueprint target" column only. Fake device files render `within` / `over, Nx` correctly after L1.
3. **Reproducibility.** Reruns at the commit under review, same script, results kept out of the tree:

   | Target | Load | Handle call | 1 KB | Keyed insert | Change-set | Cold start | Merged frame | Unmerged / merged |
   |---|---|---|---|---|---|---|---|---|
   | iOS sim, committed (3 runs) | 3.1 to 10.6 | 294-302 ns | 0.46-1.13 us | 8.0-8.8 us | 26.2-27.9 us | 2.22-2.67 ms | 787-820 us | 4.2-4.3x |
   | iOS sim, review (1 run, own clone, deleted after) | 94 → 48 | 494 ns | 1.88 us | 13.8 us | 36.9 us | 3.18 ms | 614 us | 3.4x |
   | Chromium, committed (2 runs) | 2.3 to 3.7 | 3.16-3.48 us | 4.57-5.30 us | 20.6-22.6 us | 87.7-95.9 us | 4.16-4.50 ms | 4.03-4.28 ms | 2.8x |
   | Chromium, review (2 runs) | 19 to 22 | 5.60-6.01 us | 8.04-8.27 us | 35.7-37.0 us | 173-174 us | 6.46-7.47 ms | 5.93-6.16 ms | 2.9-3.0x |
   | Chromium, review, `es2022` build (1 run, not published) | 27 to 29 | 1.83 us | 3.29 us | 22.5 us | 31.4 us | n/a (3 launches) | 2.40 ms | 3.3x |

   My two web runs agree within 7% of each other; against the committed ones they are 1.4x to 2.0x, which is the host
   (this machine sat at load 15 to 106 all session), not the harness: the shape (row order, drain ratio, `callSync`
   ≈ async call) reproduces. The load is read correctly on macOS: `sysctl -n vm.loadavg` prints `{ 1m 5m 15m }`, so
   `awk '{print $2}'` is the one-minute figure. Android was not rerun (another agent held the `undra` AVD all session;
   its bench changes are compile-checked only).
4. **The core hook - holds.** `bench_list_update_burst` (`core/src/bench.rs:108-126`) is a public store method called
   through the generated bindings (R10; the iOS producer thread uses the public `UndraCore.callSync` with the generated
   method id because the generated method is main-actor), deterministic (positions `step * 7919 % len`, no clock, no
   randomness: R12), unit-tested (one change-set of one `Update` per step). It extends the playground's existing `Bench`
   store, an example app's bench store, not the Undra product surface. Bindings: `undra bindgen --check --docs` clean
   (I1); contract scenarios 18 x 3 = 54/54 rerun.
5. **The drain method - sound, and stated.** iOS/Android: the per-entry sample is `benchListUpdateBurst(1)` on the main
   thread against the same 10,000-row list, the same `Update` op, the same mirror and the same timer
   (`DrainStats.duration`) as the merged frame, 8 after every frame (1,920 samples). The ADR-031 entry states the method,
   that it is an estimate, that the runtime was not reverted, and the error in both directions (the one-entry drain's
   fixed work overcounts; the producer-side per-change-set copy/append is left out). The web is a different quantity
   and says so: merged = the awaited burst call (1,667 core transactions on the main thread plus one drain), per entry
   = a call committing one update minus one committing none (batches of 20, 5 us clock: ±0.25 us on 7 us); both sides
   carry the core's transaction cost, so the 2.7-2.8x is internally consistent but is not comparable with the phones'
   drain-only 4.2-4.3x / 122-137x; the drain alone (0.76-0.81 ms) is quoted beside it. H2 applies to the web figures.
6. **Finding 1 (binding call cost) - the numbers hold, the web cause did not.** See E4.
7. **Template - M1.** The gated in-checkout build is the only one exercised; an external project (verified by running
   `undra init` outside the checkout) gets two unpublished coordinates.
8. **CI - holds.** The step is in `ci.yml`'s Linux `rust` job after `setup-node` 22, runs in about 0.1 s, needs only the
   checkout; `bench.yml` mentions devices in a comment and runs no device harness.

## E4 seed: where a handle call goes (cause analysis)

Method: the same `Bench.benchAdd` path decomposed layer by layer, 7 x 100,000 calls per layer, minimum per call;
the web in headless Chromium 153 (cross-origin isolated) and Node 24 against the playground's `release-wasm` core
(captured `WebAssembly.Instance` for the raw export), bundled once at `es2022` and once at Vite's default target; Swift
as a macOS release executable over the same `UndraRuntime` and the release host core, plus a 4 s `sample` profile of
the generated call (2,542 samples in `benchAdd`). Review tooling only, not committed.

| Web layer (ns per call) | `es2022`, Chromium | Vite default target, Chromium | `es2022`, Node 24 |
|---|---|---|---|
| empty export (`undra_abi_version`) | 2 | 3 | 2 |
| raw `undra_call_sync` + `undra_buf_free`, payload in memory | 130 | 204 | 154 |
| ... plus copy in and out | 235 | 341 | 283 |
| `core.callSync` with pre-encoded args | 715 | 3,717 | 713 |
| args writer + `callSync` + `decodeValue` (the `callSync` row) | 1,034 | 5,025 | 1,295 |
| `await bench.benchAdd` (the generated binding) | 1,241 | 5,715 | 1,229 |
| args `UndraWriter` alone / `encodeCall` (BigInt handle) alone / `decodeValue(u32)` alone | 283 / 312 / 30 | 1,376 / 1,740 / 904 | 272 / 308 / 52 |
| one microtask hop | 18 | 24 | 44 |

(The default-target column ran at load ~40 against ~17; the raw export moved 1.6x, the JS-only rows 5x to 30x.)

| Swift (macOS host, release) | ns per call | Share of `benchAdd` samples |
|---|---|---|
| raw C ABI `undra_call_sync` + `undra_buf_free` | 66 | core + ABI 13% |
| `UndraCore.callSync`, pre-encoded args | 346 | `withImmediateDrain` (2 lock pairs, `Thread.isMainThread`, `MainActor.assumeIsolated` with its executor check, `flush()` of an empty queue) ~31%; reply decode + `Array(reply.body)` + `Wire.Reply` teardown 14%; `makeCallPayload` (writer growth) 9%; `takeBytes` copy + free 8%; call-id reserve/remove (lock + dictionary) 5% |
| `bench.benchAdd(a:b:)`, generated | 406 | args `UndraWriter` (grows its array twice) 12% |

The cause in five lines:

1. Web: the wasm export is 130 to 200 ns (the core at `-Oz` plus the reply buffer), already above the 80 ns in-thread target on a desktop; everything else is TypeScript.
2. Web: at Vite's default target (the playground's and the `undra init` template's build) every `#private` field of each `UndraWriter`, `UndraReader` and payload object a call allocates becomes a `WeakMap` set/get, which is 70-80% of the call (1.2 us at `es2022` vs 3.7-5.7 us lowered).
3. Web at `es2022`: what is left is per-call allocation and encoding (args writer ~280 ns, the call header with a `BigInt` handle ~310 ns, copy in/out ~100 ns); the promise (~20-40 ns) and the mirror's empty flush are noise.
4. Swift: the core and the C ABI are 13%; ~31% is the read-your-writes wrapper run on a call that produced no change-set (`MainActor.assumeIsolated` executor checks, locks, an empty `flush()`), and ~45% is four to five heap allocations and copies (args writer growth, payload writer, `takeBytes`, `Array(reply.body)`) plus the pending-dictionary entry.
5. E4 levers: web `build.target: "es2022"` in the template (or no `#private` on hot classes), no `BigInt` in the header, encode args into the wasm scratch and decode in place, and measure an `opt-level=3` wasm; Swift a no-change-set fast path in `withImmediateDrain`, sized writers in generated code, decode the reply from the `UndraBuf` before freeing it, and no pending-map entry for direct sync calls.

## Verification (at `80a4058`)

`cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace`
**2,238 passed, 0 failed, 10 ignored**; `node --test scripts/bench-device-report.test.mjs` **16 passed** (14 + 2 new; three
changed); web `npm test` 112 passed, `npm run typecheck` clean; `bash contract-tests/run-all.sh` S01-S18 x ts/kotlin/swift
**54/54**; `undra bindgen -C examples/playground --check --docs` clean; `node scripts/bench-device-report.mjs validate`
on all seven files ok, `render` regenerated the block (intro, drain header, the web annotation); iOS `--quick` on a cloned
simulator after the Swift change: pass; Android `:app:compileBenchmarkKotlin :app:compileBenchmarkAndroidTestKotlin`:
pass. The review's simulator clone was deleted; no other agent's simulator or emulator was booted, stopped or used.
