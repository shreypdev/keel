# Onboarding — build and test Undra on your machine

Everything here was exercised end to end on a clean Apple-Silicon Mac. Linux notes are
inline where the path differs; Windows is untested. Time to a green core suite: ~10
minutes. Time to the full five-language matrix: ~30 minutes including downloads.

## 1. Install the toolchain

`undra doctor` checks everything in this section and prints the exact command for each gap. Every finding
links to the heading below that explains it: `undra doctor --fix` prints all the commands as one block to
read and paste (it never runs them), and `undra doctor --json` gives the same report to tools (state,
observed value, fix and anchor per finding). Rows marked *contributors* matter only if you work on Undra
itself; building an app never needs them.

On macOS the fixes install with Homebrew, and they are written for the shell you run doctor in: when `brew` is
not on that shell's `PATH` but Homebrew is installed, a fix starts with `eval "$(/opt/homebrew/bin/brew shellenv)"`
(Apple silicon's `/opt/homebrew` is not on the default `PATH`); when Homebrew is not installed, it starts with
Homebrew's own installer. A tool Homebrew already installed out of `PATH`'s reach is reported as such, with only
the `shellenv` line as its fix. The paths below are Apple silicon's; on an Intel Mac Homebrew is `/usr/local`
(`brew --prefix` says which).

### Rust and its targets

#### Rust

`rustup` with the stable channel, 1.85 or newer (the MSRV of the workspace, edition 2024). Doctor checks
`rustup`, the active channel, `rustc` and `cargo`.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
rustup update stable              # when doctor says the version is too old
rustup default stable             # when a nightly or beta channel is active
```

#### Rust targets

Doctor checks the targets of the platforms in scope: `wasm32-unknown-unknown` for the web;
`aarch64-apple-ios` (devices) and `aarch64-apple-ios-sim` (the simulator on Apple silicon) for iOS, plus
`x86_64-apple-ios` when `[ios] simulator_archs` in `undra.toml` lists `x86_64`; and the target of every ABI in
`[android] abis` (`aarch64-linux-android` for `arm64-v8a`, `x86_64-linux-android` for `x86_64`).

```bash
rustup target add wasm32-unknown-unknown \
  aarch64-apple-ios aarch64-apple-ios-sim \
  aarch64-linux-android x86_64-linux-android
```

### The web app and the TypeScript runtime

#### Node and npm

Node 20 or newer (the TypeScript runtime's `engines`; 22 or newer is what CI uses) and the npm that comes with it.

```bash
brew install node                 # or: fnm install --lts
```

#### wasm-opt

Optional. `undra build --platform web` works without it; with binaryen's `wasm-opt -Oz` the wasm core is
10-20% smaller.

```bash
brew install binaryen             # Linux: sudo apt-get install -y binaryen
```

### iOS (macOS only)

#### Xcode

Install **Xcode from the App Store** (16 or newer; the Command Line Tools alone have no XCTest, no
`xcodebuild` and no simulators), then point the system at it and accept the license. Doctor reports where
`xcode-select` points, and says so when it points at the command line tools although Xcode is installed.

```bash
open macappstore://apps.apple.com/app/xcode/id497799835
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -license accept
```

If you cannot run `sudo`, `undra` and `scripts/env.sh` fall back to setting `DEVELOPER_DIR` for their own
builds (they do not change what `xcodebuild` does when you run it yourself).

#### iOS simulator runtime

At least one iOS simulator runtime, to run the app (building does not need it).

```bash
xcodebuild -downloadPlatform iOS        # one-time, large
```

### Android

#### Android SDK

The SDK with `platform-tools` (`adb`) and platform 35, and `ANDROID_HOME` pointing at it (Gradle and Android
Studio read it; `undra` finds the usual locations without it).

```bash
brew install --cask android-commandlinetools
export ANDROID_HOME="$(brew --prefix)/share/android-commandlinetools"
SDKM=$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager
yes | $SDKM --licenses
$SDKM "platform-tools" "platforms;android-35" "build-tools;35.0.0"
```

On Linux download the command line tools from <https://developer.android.com/studio#command-line-tools-only>
into `$HOME/Android/Sdk/cmdline-tools/latest` and use the same `sdkmanager` lines.

#### Android NDK

NDK r27 or newer (16 KB page alignment, which Google Play requires), and `ANDROID_NDK_HOME` (cargo-ndk and
Gradle read it; `undra` finds `$ANDROID_HOME/ndk/<version>` without it).

```bash
$SDKM "ndk;27.2.12479018"
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/27.2.12479018
```

#### cargo-ndk

`undra build --platform android` runs `cargo ndk`. 3.5 or newer, which aligns libraries to 16 KB pages.

```bash
cargo install cargo-ndk
```

#### JDK 17

Gradle and the Android Gradle plugin need JDK 17 or newer (Android Studio bundles one). A missing JDK is a
failure inside a project with an Android app and advice elsewhere; `undra build` itself does not use Java.

```bash
brew install openjdk@17               # Linux: sudo apt-get install -y openjdk-17-jdk
export JAVA_HOME="$(brew --prefix openjdk@17)/libexec/openjdk.jdk/Contents/Home"
export PATH="$JAVA_HOME/bin:$PATH"    # Homebrew's openjdk@17 is keg-only: not on PATH by itself
```

#### Gradle wrapper

A generated project's `android/` has `./gradlew` (`undra init` copies it from a checkout or creates it with
`gradle wrapper`). Doctor checks for it inside a project.

```bash
brew install gradle
(cd android && gradle wrapper --gradle-version 8.14.3)
```

#### adb and a device

`adb` (from `platform-tools`) and, to run the app, an attached device or a running emulator. Neither is
needed to build. `adb devices` says `device`, not `unauthorized` or `offline`.

```bash
adb devices
$ANDROID_HOME/emulator/emulator -avd undra &      # an emulator, when no phone is attached
```

#### The undra emulator (contributors)

The device tests and benchmarks of this repository boot the AVD named `undra`.

```bash
$SDKM "emulator" "system-images;android-35;google_apis;arm64-v8a"
$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager create avd -n undra \
      -k "system-images;android-35;google_apis;arm64-v8a" -d pixel_7
```

(`x86_64` instead of `arm64-v8a` on Intel and Linux hosts.)

### Tools for contributors

#### Kotlin compiler (contributors)

Only the Kotlin runtime tests of this repository compile with `kotlinc`; an app never does. They also need the
kotlinx-coroutines jar below.

```bash
brew install kotlin
# The Kotlin local test runner needs one jar (any location; env.sh looks in ../.tools/lib)
mkdir -p ../.tools/lib && curl -sSfLo ../.tools/lib/kotlinx-coroutines-core-jvm-1.6.4.jar \
  https://repo1.maven.org/maven2/org/jetbrains/kotlinx/kotlinx-coroutines-core-jvm/1.6.4/kotlinx-coroutines-core-jvm-1.6.4.jar
```

### The machine

#### Disk space

Rust, Xcode (DerivedData, simulators) and Gradle keep several gigabytes of caches each. Doctor warns under
10 GB free on the disk of the project (or your home directory).

```bash
cargo clean
xcrun simctl delete unavailable
```

#### undra on PATH

A generated project builds its core by itself: the Gradle task, the Xcode build phase and the Vite plugin
run `undra build` and look for `undra` on `PATH` (the Xcode phase and Gradle also look in `~/.undra/bin`,
`~/.cargo/bin` and Homebrew's directories, since a GUI-launched build has a short `PATH`). Doctor checks
that one is found and that it is the version that is running.

```bash
curl -fsSL https://shreypdev.github.io/undra/install.sh | sh
export PATH="$HOME/.undra/bin:$PATH"
```

### One command wires it all up per shell

```bash
source scripts/env.sh    # PATH, UNDRA_KOTLIN_* jars, DEVELOPER_DIR fallback
undra doctor              # (after `cargo install --path crates/undra-cli`) prints what's missing, with the fix
undra doctor --fix        # the fixes as one block you can read and paste
```

## 2. Run the suites

Every suite is local; nothing needs the network after install.

| Suite | Command | Expect |
|---|---|---|
| Rust workspace | `cargo test --workspace` | 2,100+ pass |
| Lints (CI-equivalent) | `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` | clean |
| TypeScript runtime | `cd runtimes/ts/@undra/runtime && npm ci && npm test` | 890+ pass |
| Kotlin runtime | `runtimes/kotlin/undra-runtime/scripts/test-local.sh` | 629 cases, 0 failed (2 skipped without a native library) |
| Kotlin runtime under CI's compiler | `kotlinc` 2.0.21 on PATH (CI downloads it; brew's is newer and infers more) — `PATH=<kotlin-2.0.21>/bin:$PATH runtimes/kotlin/undra-runtime/scripts/test-local.sh` | same count; a passing run under brew's Kotlin alone is not proof |
| Kotlin over the real JNI core | `cargo build --manifest-path crates/undra-ffi/tests/fixture/Cargo.toml` (the fixture core, namespace `undra_fixture`), then `UNDRA_NATIVE_LIB_DIR=$PWD/crates/undra-ffi/tests/fixture/target/debug runtimes/kotlin/undra-runtime/scripts/test-local.sh`; `bash crates/undra-ffi/tests/jni/run.sh` is the end-to-end leg | the JNI smoke cases run (1 skipped: the one that needs the library absent) |
| Android adapters, JVM unit tests (needs the Android SDK) | `cd runtimes/kotlin/undra-runtime && ./gradlew :android-adapters:test` | 130 pass, 1 skipped (the debug and release variants both run) |
| Android adapters, instrumented tests (needs a booted emulator or device; set `ANDROID_SERIAL` if several are attached) | `cd runtimes/kotlin/undra-runtime && ./gradlew :android-adapters:connectedAndroidTest` | 112 pass, 1 skipped (the test that switches the device's network off runs only with `-Pandroid.testInstrumentationRunnerArguments.undra.networkToggle=true`) |
| Playground Android app on the real adapters (offline queue surviving a killed process) | `bash examples/playground/android/smoke.sh` (needs a booted emulator; it switches airplane mode on and off) | `SMOKE PASSED` |
| Swift runtime | `cd runtimes/swift/UndraRuntime && swift test` | 526 pass |
| Swift runtime over the real C ABI table (the fixture core) | `bash crates/undra-ffi/tests/swift/run.sh` | 6 pass |
| wasm ABI (real module + real TS runtime) | `bash crates/undra-ffi/tests/wasm/run.sh` | 19 + 24 pass |
| C host harness (through the fixture core's table, `undra_fixture_undra_api`) | `bash crates/undra-ffi/tests/c/run.sh` (add `UNDRA_C_SANITIZE=1` for ASan) | ok |
| Contract scenarios ×3 platforms | `bash contract-tests/run-all.sh` | 57/57 pass (S01 to S18 and S26 "two cores", per platform) |
| Two cores in one process (ADR-044): the playground core as `playground_a` and `playground_b` | `examples/two-cores/{ios,android,jvm,node}/run.sh` (iOS: the booted simulator, `CONFIGURATION=Release` for the fat-LTO cores; Android: the attached emulator) | `two-cores <platform>: passed` |
| Build systems of a generated project: Gradle, `xcodebuild` and `npm run build` each build the core with no earlier `undra build` (a clean project, then up-to-date, then a change, then the other variant and back; the app links the new core) | `UNDRA_TEST_BUILD_SYSTEMS=1 cargo test -p undra-cli --test build_systems -- --nocapture` (`UNDRA_REQUIRE_TOOLCHAINS=1` makes a missing toolchain a failure; needs `java` on PATH, which `scripts/env.sh` puts there) | 6 pass; skips, saying why, where a toolchain is missing |
| `undra upgrade` end to end (regenerates the bindings against a local clone standing in for GitHub) | `UNDRA_TEST_UPGRADE_E2E=1 cargo test -p undra-cli --test upgrade` | 15 pass |
| Distribution: npm packages (build, pack, `npm install -g`, run) | `bash packaging/npm/test.sh` | all checks pass |
| Distribution: the curl installer against a served release (checksums, tampering, platforms) | `bash packaging/test-install.sh` | all checks pass |
| Benchmark budget gate | `cargo test -p undra-bench --test budgets --release` | pass |
| Benchmarks (numbers for humans) | `cargo bench -p undra-bench` | see `bench/RESULTS.md` |
| Device bench: the blueprint rows through the generated binding and the mirror, on a simulator, emulator, browser or phone | `scripts/bench-device.sh --device ios`, `--device android` (boots the `undra` AVD if nothing is attached; `--target <serial>` for a phone), `--device web`; add `--quick` to check the plumbing in seconds | writes `bench/results/device/<date>-<target>.json` and the device tables of `bench/RESULTS.md`; needs the iOS simulator + Xcode, the Android SDK + NDK, or Playwright's Chromium (`cd examples/playground/web && npx playwright install chromium`) |
| React Native runtime: C++ host under ASan + UBSan (both shims) and the JSI layer against React Native's headers | `runtimes/rn/@undra/react-native/cpp/test/run.sh` (needs `npm ci` in `examples/playground/rn` for the headers, and `undra build --platform host` of the playground and of `examples/two-cores/a`, which it runs when missing; `UNDRA_RN_REQUIRE_JSI=1` makes a missing one a failure) | 15 store checks, then 29 + 29 host checks (the linked shim on macOS only); `UndraJsi.cpp`, `UndraTurboModule.cpp` and (macOS) `UndraPlatformApple.mm` compile |
| React Native runtime: unit tests, typecheck, contract column | in `runtimes/rn/@undra/react-native`: `npm ci`, `npm test`, `npm run typecheck` (build `runtimes/ts/@undra/runtime` first), `npm run test:contract` (needs `undra build -C examples/playground --platform web`) | 65 pass; clean; 17 pass + S17 skipped |
| React Native playground app on a device | `scripts/rn-device-checks.sh ios` (the iPhone simulator; CocoaPods) or `scripts/rn-device-checks.sh android --target <serial>` (an emulator such as the `undra-rn` AVD, or a phone) | `UNDRA-RN CHECKS 19/19 passed` (iOS) or `20/20` (Android) |
| Device bench report (CI runs it) | `node --test scripts/bench-device-report.test.mjs` | 14 pass |

Gotcha worth knowing: the bindgen tests that compile and run the generated Kotlin, TypeScript and
Swift (`typecheck_kotlin`, `typecheck_ts`, `run_ts`, `typecheck_swift`) **skip, and pass, when their
compiler is not found** (`kotlinc`, `tsc`, `swift`). Put `runtimes/ts/@undra/runtime/node_modules/.bin`
and a JDK 17 on `PATH`, and run with `UNDRA_REQUIRE_TOOLCHAINS=1` so a missing tool fails instead of skipping.

Gotcha worth knowing: since C ABI version 2 (ADR-044) `undra-ffi` exports nothing of its own;
every native harness (C, Swift, JNI) runs against the fixture core
(`crates/undra-ffi/tests/fixture`, namespace `undra_fixture`), which exports its table and, with
the `jni` feature it always builds with on native targets, `JNI_OnLoad`.

## 3. Run the reference app

```bash
cargo install --path crates/undra-cli
undra build -C examples/playground --platform web,ios,android   # artifacts under examples/playground/build/
cd examples/playground/web && npm install && npm run dev       # Chrome
bash examples/playground/ios/smoke.sh                          # boots a simulator, installs, screenshots
# Android: bash examples/playground/android/smoke.sh, or see examples/playground/android/README.md (gradlew assembleDebug + the `undra` AVD)
```

The playground is wired by hand, so it still takes the explicit `undra build` above. A project made by `undra init` does not:
its Gradle task, Xcode build phase and Vite plugin run `undra build` themselves before each app builds
([`docs/DEV_LOOP.md`](DEV_LOOP.md#production-builds-are-not-a-separate-step)), `undra init` writes its CI workflow
(`.github/workflows/undra.yml`), and `undra upgrade` moves it to a newer Undra in one command.

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
