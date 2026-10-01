# Onboarding — build and test Undra on your machine

Everything here was exercised end to end on a clean Apple-Silicon Mac. Linux notes are
inline where the path differs; Windows is untested. Time to a green core suite: ~10
minutes. Time to the full five-language matrix: ~30 minutes including downloads.

## 1. Install the toolchain

### Required for the Rust workspace (everything in `crates/`)

```bash
# Rust — stable, plus the cross targets Undra ships to
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
rustup target add wasm32-unknown-unknown \
  aarch64-apple-ios aarch64-apple-ios-sim \
  aarch64-linux-android x86_64-linux-android
```

### Required for the platform runtimes and apps

```bash
# Node 22+ (TypeScript runtime, web app, wasm acceptance tests)
brew install node

# JDK 17 + Kotlin + Gradle (Kotlin runtime, Android app)
brew install openjdk@17 kotlin gradle

# binaryen (wasm-opt, used by `undra build --platform web`)
brew install binaryen

# The Kotlin local test runner needs one jar (any location; env.sh looks in ../.tools/lib)
mkdir -p ../.tools/lib && curl -sSfLo ../.tools/lib/kotlinx-coroutines-core-jvm-1.6.4.jar \
  https://repo1.maven.org/maven2/org/jetbrains/kotlinx/kotlinx-coroutines-core-jvm/1.6.4/kotlinx-coroutines-core-jvm-1.6.4.jar
```

### Required for iOS (macOS only)

Install **Xcode from the App Store** (the Command Line Tools alone have no XCTest and no
simulators), then:

```bash
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -license accept
xcodebuild -downloadPlatform iOS        # simulator runtime (one-time, large)
```

If you cannot run `sudo`, `scripts/env.sh` falls back to setting `DEVELOPER_DIR` for you.

### Required for Android

```bash
brew install --cask android-commandlinetools
export ANDROID_HOME=/opt/homebrew/share/android-commandlinetools
SDKM=$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager
yes | $SDKM --licenses
$SDKM "platform-tools" "platforms;android-35" "build-tools;35.0.0" \
      "ndk;27.2.12479018" "emulator" "system-images;android-35;google_apis;arm64-v8a"
$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager create avd -n undra \
      -k "system-images;android-35;google_apis;arm64-v8a" -d pixel_7
cargo install cargo-ndk
```

### One command wires it all up per shell

```bash
source scripts/env.sh    # PATH, UNDRA_KOTLIN_* jars, DEVELOPER_DIR fallback
undra doctor              # (after `cargo install --path crates/undra-cli`) prints what's missing, with the fix
```

## 2. Run the suites

Every suite is local; nothing needs the network after install.

| Suite | Command | Expect |
|---|---|---|
| Rust workspace | `cargo test --workspace` | 2,100+ pass |
| Lints (CI-equivalent) | `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` | clean |
| TypeScript runtime | `cd runtimes/ts/@undra/runtime && npm ci && npm test` | 890+ pass |
| Kotlin runtime | `runtimes/kotlin/undra-runtime/scripts/test-local.sh` | 588 cases, 0 failed (2 skipped without a native library) |
| Kotlin runtime under CI's compiler | `kotlinc` 2.0.21 on PATH (CI downloads it; brew's is newer and infers more) — `PATH=<kotlin-2.0.21>/bin:$PATH runtimes/kotlin/undra-runtime/scripts/test-local.sh` | same count; a passing run under brew's Kotlin alone is not proof |
| Kotlin over the real JNI core | `cargo build -p undra-ffi --features jni`, then `UNDRA_NATIVE_LIB_DIR=$PWD/target/debug UNDRA_NATIVE_NAME=undra_ffi runtimes/kotlin/undra-runtime/scripts/test-local.sh` | the JNI smoke cases run |
| Android adapters, JVM unit tests (needs the Android SDK) | `cd runtimes/kotlin/undra-runtime && ./gradlew :android-adapters:test` | 130 pass, 1 skipped (the debug and release variants both run) |
| Android adapters, instrumented tests (needs a booted emulator or device; set `ANDROID_SERIAL` if several are attached) | `cd runtimes/kotlin/undra-runtime && ./gradlew :android-adapters:connectedAndroidTest` | 112 pass, 1 skipped (the test that switches the device's network off runs only with `-Pandroid.testInstrumentationRunnerArguments.undra.networkToggle=true`) |
| Playground Android app on the real adapters (offline queue surviving a killed process) | `bash examples/playground/android/smoke.sh` (needs a booted emulator; it switches airplane mode on and off) | `SMOKE PASSED` |
| Swift runtime | `cd runtimes/swift/UndraRuntime && swift test` | 433 pass |
| wasm ABI (real module + real TS runtime) | `bash crates/undra-ffi/tests/wasm/run.sh` | 29 pass |
| C host harness | `bash crates/undra-ffi/tests/c/run.sh` (add `UNDRA_C_SANITIZE=1` for ASan) | ok |
| Contract scenarios ×3 platforms | `bash contract-tests/run-all.sh` | 54/54 pass |
| Distribution: npm packages (build, pack, `npm install -g`, run) | `bash packaging/npm/test.sh` | all checks pass |
| Distribution: the curl installer against a served release (checksums, tampering, platforms) | `bash packaging/test-install.sh` | all checks pass |
| Benchmark budget gate | `cargo test -p undra-bench --test budgets --release` | pass |
| Benchmarks (numbers for humans) | `cargo bench -p undra-bench` | see `bench/RESULTS.md` |
| Device bench: the blueprint rows through the generated binding and the mirror, on a simulator, emulator, browser or phone | `scripts/bench-device.sh --device ios`, `--device android` (boots the `undra` AVD if nothing is attached; `--target <serial>` for a phone), `--device web`; add `--quick` to check the plumbing in seconds | writes `bench/results/device/<date>-<target>.json` and the device tables of `bench/RESULTS.md`; needs the iOS simulator + Xcode, the Android SDK + NDK, or Playwright's Chromium (`cd examples/playground/web && npx playwright install chromium`) |
| Device bench report (CI runs it) | `node --test scripts/bench-device-report.test.mjs` | 14 pass |

Gotcha worth knowing: the bindgen tests that compile and run the generated Kotlin, TypeScript and
Swift (`typecheck_kotlin`, `typecheck_ts`, `run_ts`, `typecheck_swift`) **skip, and pass, when their
compiler is not found** (`kotlinc`, `tsc`, `swift`). Put `runtimes/ts/@undra/runtime/node_modules/.bin`
and a JDK 17 on `PATH`, and run with `UNDRA_REQUIRE_TOOLCHAINS=1` so a missing tool fails instead of skipping.

Gotcha worth knowing: the C harness builds `undra-ffi` **without** the `jni` feature and
overwrites `target/debug/libundra_ffi.dylib`. If you run the Kotlin JNI leg afterwards,
rebuild with `--features jni` first. CI jobs are isolated, so only local chained runs hit
this.

## 3. Run the reference app

```bash
cargo install --path crates/undra-cli
undra build -C examples/playground --platform web,ios,android   # artifacts under examples/playground/build/
cd examples/playground/web && npm install && npm run dev       # Chrome
bash examples/playground/ios/smoke.sh                          # boots a simulator, installs, screenshots
# Android: bash examples/playground/android/smoke.sh, or see examples/playground/android/README.md (gradlew assembleDebug + the `undra` AVD)
```

The live loop (edit Rust, every app picks up the new core, a dropped connection heals itself) is `undra dev`:
[`docs/DEV_LOOP.md`](DEV_LOOP.md) has the URL of each platform (the Android emulator is `ws://10.0.2.2:<port>`),
`undra dev --android`, how reconnecting works and a troubleshooting table.

## 4. Read before you write code

1. [`CLAUDE.md`](../CLAUDE.md) — the constitution (R1–R12) and engineering standards.
   These are enforced in review, not aspirational.
2. [`docs/SPEC.md`](SPEC.md) — the binding spec. Code that disagrees with it is wrong
   until an ADR changes it.
3. [`.10x/adrs/`](../.10x/adrs/) — why the big decisions were made (ADR-014 through 028).
4. [`.10x/status.md`](../.10x/status.md) and [`.10x/handoff.md`](../.10x/handoff.md) —
   where the work stands and what v1.x holds.
5. [`docs/AGENT_WORKFLOW.md`](AGENT_WORKFLOW.md) — how changes are made here: one
   worktree per piece, adversarial review before merge, cleanup after. It applies to
   humans and AI agents alike.

## 5. The bar for a change

Every piece lands whole (R4): unit tests next to the code, integration tests in
`tests/`, a contract scenario if the wire is touched, a benchmark if the boundary is
touched, docs on every `pub` item, `clippy -D warnings` clean, and the *full* matrix
green — not just the crate you touched. If your change alters the wire, the runtime
model, the threading model or a generated public shape, write the ADR first (R11).
