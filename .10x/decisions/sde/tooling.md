# SDE — tooling: doctor depth, build-system integration, CI, upgrade (wt/tooling, 2026-10-01)

Track D2–D5 of the v1.x design (`.10x/specs/2026-10-01-v1x-default-choice-design.md`). The goal: an engineer
who runs `undra init` never performs a manual build step, never discovers a missing prerequisite at the wrong
moment, upgrades with one command and gets CI for free. No ADR: nothing here touches the wire, the runtime
model, the threading model, the C ABI or a shape bindgen generates (the generated bindings are byte-identical:
`undra bindgen -C examples/playground --check --docs` passes). What changed are the `undra init` templates
(the Xcode project, `build.gradle.kts`, `vite.config.ts`, the README, one new workflow file), the CLI, and one
new subpath export of the TypeScript runtime; SPEC 13 records all of it.

## D5 — `undra doctor` (commit `a1bbf41`)

`commands/doctor.rs` became `commands/doctor/` (`finding.rs`, `rust.rs`, `ios.rs`, `android.rs`, `web.rs`,
`system.rs`, `testing.rs`). Every check is declared once as a `Check { id, anchor, audience, optional }` and makes
`Finding`s: a stable id (`android.ndk`), a **state** (`ok` / `missing` / `wrong-version` / `not-applicable`), a
**severity** (ok / warn / fail / skip: what the exit status follows), the observed value, the fix as exact
commands, the `docs/ONBOARDING.md` anchor. 34 checks: rustup, active channel, rustc at the MSRV (read from
`CARGO_PKG_RUST_VERSION`, so CLAUDE.md's 1.85 has one source), cargo, the Rust targets by platform (the iOS ones
follow `[ios] simulator_archs`, the Android ones `[android] abis`, `x86_64-apple-ios` is optional outside a
project); Xcode (version >= 16, license, command line tools vs full), where `xcode-select` points (and the
`DEVELOPER_DIR` fallback), `lipo`, a simulator runtime; the Android SDK, `ANDROID_HOME`, platform 35+, `adb`, an
attached device (a start-the-emulator or create-the-AVD fix), NDK r27+, `ANDROID_NDK_HOME`, `cargo-ndk` >= 3.5,
JDK 17+, the Gradle wrapper (inside a project), the `undra` AVD (contributors); Node 20+, npm, `wasm-opt`
(optional, says what it buys); free disk (warn under 10 GB); `undra` on `PATH` at the running version;
`kotlinc` (marked *for contributors*; skipped, not warned, for everyone else).

* `--fix` prints **only** the block (stdout; the tally goes to stderr) so it can be read, saved or piped: comments
  say what each group is for, repeated commands appear once, `rustup target add` lines are folded into one.
  Nothing is ever run. `--json` prints the report (`--fix` and `--json` are alternatives: clap `conflicts_with`).
* Contributor = `UNDRA_CONTRIBUTOR=1`, a project with `[undra] path`, or a working directory inside a checkout.
* Deviation: a missing JDK is a **failure inside a project with an Android app** and a warning elsewhere (Gradle
  cannot build without it; `undra build` can). `adb` and "a device is attached" are warnings, never failures.
* Tests: 48 unit tests against described machines (`testing.rs`: a mac with everything, the same minus one thing,
  a bare Linux box); a meta test proves every declared check is reported on some machine and nothing undeclared is;
  another that every anchor is a heading of `docs/ONBOARDING.md` (headings read outside code fences); a JSON-shape
  test. `tests/doctor.rs`: the real doctor exits 0 or 1 without panicking, `--json` is the documented document and
  agrees with the exit status, `--fix` is comments and commands only. `ONBOARDING.md` section 1 was rewritten with one
  heading per prerequisite (the anchors).

## D3 — `undra build` is never a manual step (commits `a668a25`, `27483ee`)

**Android.** `android/app/build.gradle.kts` has an `UndraBuild` task class and `undraBuild` (`preBuild` depends on
it): inputs `core/src/**`, `Cargo.toml`, `Cargo.lock`, `build.rs` of the core and the project's `undra.toml`,
`Cargo.toml`, `Cargo.lock`; output `build/android/jniLibs`; Gradle's own up-to-date check does the skipping. One task,
not one per variant, because both profiles write the same directory: the profile is chosen from the task graph
(`assembleRelease`, `bundleRelease`, ... contain `Release`), `-PundraRelease=true|false` overrides, `-PundraSkipBuild`
or `UNDRA_SKIP_BUILD=1` skips it (a build that will only run against `undra dev`). `undra` is `UNDRA_BIN`, then `PATH`,
then `~/.undra/bin`, `~/.cargo/bin`, Homebrew (an IDE-launched Gradle has a short `PATH`); when it is absent the build
fails with the CLI's own error shape, `error[undra::C0003]` (no new code: C0003 is "a tool the command needs is not
installed"). `ANDROID_HOME` is handed to `undra` from AGP's SDK location when the environment has none.

**iOS.** The Xcode project has a Run Script phase **Build the Undra core**, first in `buildPhases`, running
`undra -C "$SRCROOT/.." build --platform ios --configuration "$CONFIGURATION"` with `inputFileListPaths` /
`outputFileListPaths` (`ios/Config/undra-core-{inputs,outputs}.xcfilelist`) and user-script sandboxing off for the
target (the phase runs Cargo). Three things were found by running real `xcodebuild`, not assumed:

1. *An XCFramework in the Frameworks phase fails the build on a clean tree* ("There is no XCFramework found at ...":
   Xcode reads it while planning, before any phase runs). The project therefore no longer references the XCFramework
   at all; it was already linked with `-force_load` in `OTHER_LDFLAGS`, which is all the core needs (the XCFramework is
   headerless). The outputs list names the slices' libraries so the link is ordered after the phase.
2. *Xcode looks at a directory's own entry, not at what is in it* (measured: editing `core/src/lib.rs` in place with only
   `core/src` listed does not re-run the phase; adding a file does, through the directory's time). So the input list names
   every file: `undra init` writes it for the template core and every iOS `undra build` **refreshes it** from what the
   core is really built from (`CoreInfo.local_dirs` + manifests + `src/**`; `builds/xcode.rs`), unchanged content is not
   rewritten. After it changes Xcode runs the phase once more, then it is stable (measured).
3. *A configuration switch must re-run the phase* or a Release build ships the Debug core: `undra build --configuration
   <NAME>` (new flag, conflicts with `--release`; a name containing `Release` builds release, others debug) writes
   `build/ios/.undra-configuration-<NAME>` and removes the other stamps; the outputs list names
   `.undra-configuration-$(CONFIGURATION)`.

Also: Xcode's build environment (`SDKROOT`, `CC`, `ARCHS`, ...) breaks Cargo's toolchain lookups, so the script hands
`undra` a clean environment (`env -i` plus `HOME`, `PATH`, `DEVELOPER_DIR`, `CARGO_HOME`, `RUSTUP_HOME`,
`CARGO_TARGET_DIR`); a missing `undra` prints `error: [undra::C0003] ...` (Xcode shows `error:` lines in the issue
navigator). The script has no backtick in an `echo` (a test checks).

**Web.** `@undra/runtime/vite` (`src/vite.ts`, subpath export `./vite`, SPEC 13 updated): zero dependencies, Node only,
strict TypeScript. It types the parts of Vite it touches structurally (`ViteConfigLike`, `ViteDevServerLike`, ...)
instead of importing `vite`, and was checked against the real Vite 6 types (a scratch `tsc` over the generated
`vite.config.ts` with `exactOptionalPropertyTypes`). `buildStart` runs `undra -C <root> build --platform web` for
`vite build` and `vite dev`; under `vite dev` a failed *first* build is logged, not fatal (the next save is the retry);
`configureServer` watches `<core>/src`, `<core>/Cargo.toml`, the workspace manifest and `undra.toml` (the core's path
is read from `[core] path`), debounces (150 ms), builds one at a time (a change during a build queues exactly one
more), then `server.ws.send({type:"full-reload"})`; a failed rebuild goes to the overlay and the log, never reloads.
`tsconfig.build.json` excludes it (that config has `types: []`) and `tsconfig.vite.json` compiles it with Node's types;
`npm run build` and `typecheck` run both. In a checkout the template imports the plugin's source by relative path
(`vite.config.ts` is bundled by Vite, and a checkout has no `node_modules/@undra/runtime`); registry projects import
`@undra/runtime/vite` (checked by installing the packed tarball into a registry-mode project: module resolution works
and the plugin runs).

**Evidence** (`UNDRA_REQUIRE_TOOLCHAINS=1 cargo test -p undra-cli --test build_systems -- --nocapture`, 6 pass, 103 s;
a fresh `undra init` project, no earlier `undra build`):

```
gradle assembleDebug       > Task :app:undraBuild / undra: build --platform android / ==> Building the core for android (debug) / BUILD SUCCESSFUL
  (again)                  > Task :app:undraBuild UP-TO-DATE            BUILD SUCCESSFUL in 621ms
  (core/src changed)       > Task :app:undraBuild / ==> Building the core for android (debug)
gradle assembleRelease     undra: build --platform android --release / ==> Building the core for android (release)   arm64-v8a 1,105,976 bytes
xcodebuild -scheme ... build   PhaseScriptExecution Build\ the\ Undra\ core / note: undra build --platform ios --configuration Debug / ==> Building the core for ios (debug) / ** BUILD SUCCEEDED **
  (again)                  no PhaseScriptExecution line; ** BUILD SUCCEEDED **
  (lib.rs changed; a new file; an in-place edit of that file)   the phase runs each time
  -configuration Release   note: undra build --platform ios --configuration Release / ==> Building the core for ios (release)
npm run build              vite v6.4.3 building for production... / ==> Building the core for web (release) / web wasm 353.1 KB / built in 3.22s
without undra on PATH      error[undra::C0003]: `undra` was not found  (Gradle, the Xcode phase and the Vite plugin each, with the install command)
```

Tests: `tests/build_systems.rs` (6) as above; template unit tests in `init.rs` (the pbxproj phase order, file lists,
script content, every object id defined; the Gradle script; the Vite import in both modes); `builds/xcode.rs`;
`runtimes/ts/@undra/runtime/test/vite.test.ts` (24: a fake `undra` script records its arguments).
**Gating:** the three real-build tests run when `UNDRA_REQUIRE_TOOLCHAINS=1` (a missing toolchain fails) or
`UNDRA_TEST_BUILD_SYSTEMS=1` (skipped with the reason where the toolchain is missing); without either they skip with
the reason, because they take minutes and the repo's other heavy builds are opt-in too. What a toolchain needs is
decided by `undra doctor --json` on the project, so the tests use the same list a person does. The "undra is not
installed" tests skip when an `undra` is installed where the build systems look.

## D4 — CI (commit `058c69a`)

`undra init` writes `.github/workflows/undra.yml` (`ci.rs`, snippets in `templates/ci/`): `core` (fmt, clippy -D
warnings, test, `undra bindgen --check`), `web` (Node 20, `npm ci`, `npm test --if-present`, `npm run build`),
`android` (Temurin 17, NDK r27.2 through the runner's `sdkmanager`, `cargo install cargo-ndk`, `./gradlew
assembleDebug`), `ios` (macos-14, `maxim-lobanov/setup-xcode`, `UNDRA_LINK_CORE=1 xcodebuild ... -sdk
iphonesimulator`), only the jobs of the project's platforms, Rust targets from the project's ABIs and simulator
architectures. Every job installs `undra` with `curl -fsSL .../install.sh | sh` and `$HOME/.undra/bin` on
`GITHUB_PATH`; the version is **one** `env: UNDRA_VERSION: "<CLI version>"` at the top, which `install.sh` reads
itself and `undra upgrade` moves. Deviations:

* No workflow for a project that uses a local checkout (`--undra-path`, or created inside one): CI cannot see the
  checkout, so the job would fail; the README's file list omits `.github/` then.
* `npm ci` is `if [ -f package-lock.json ]; then npm ci; else npm install; fi`: `undra init` cannot create a lock file
  offline, and the first CI run must not fail for that. `npm test --if-present`: the web template has no test script.
* The template core was not rustfmt-clean, so the first run of the new `core` job would have failed `cargo fmt
  --check`; `templates/core/src/lib.rs` is now formatted (a test runs `rustfmt --check` on a generated core).
* The YAML is parsed in the tests by a small reader written for the subset workflows use (a YAML crate would have
  changed the shared `Cargo.lock`); `ruby -ryaml` (installed here) checks it as well and `actionlint` when present
  (it is not installed on this machine, and installing system tools was not asked for).

## D2 — `undra upgrade` (commit `bbfd72a`)

`undra upgrade [--dry-run] [--no-bindgen] [--docs]`. `upgrade.rs` has one line-level editor per file kind (they keep
comments and formatting, change only the version text, or for a git dependency the one `tag`/`rev`/`branch` pair, and
report what they saw in the same pass): `Cargo.toml` of any crate of the project (inline tables, plain strings, dotted
`[dependencies.undra]` sections, `[workspace.dependencies]`, `[dev-]`/`[build-]`/`[target.*.]` tables), `undra.toml`,
`package.json`, Gradle (`.gradle` and `.kts`), `project.pbxproj`, workflows. Pins are written the way `undra init`
writes them: crates and the workflow the exact release (`tag = "vX.Y.Z"`, `UNDRA_VERSION: "X.Y.Z"`), the runtimes and
`[undra] version` the release line (`^X.Y.0`, `X.Y.0`, `X.Y`) — a test ages a fresh `init` back to 0.0.9 pin by pin and
checks the upgrade reproduces the six pin files **byte for byte**.

* The version a project "is on" is the oldest *exact* pin (core crates, workflow), else the newest release line:
  `undra.toml`'s `0.1` must not drag the notes back to 0.0.
* A `path` dependency anywhere (the core, `[undra] path`, a `-SNAPSHOT` Gradle runtime, a local Swift package, a
  `file:` runtime) = a checkout: it says so and changes nothing. A project on a version newer than the CLI is
  refused with `C0014` (reused: "the core and the project disagree about where Undra comes from", whose SPEC row and
  docs entry now also say "or the project is on a newer Undra than this undra"). A git dependency on a fork is left
  and reported. `generated/` is never scanned (bindgen owns it).
* `migrations.rs`: a table of releases with kinded notes (`do`, `changed`, `new`), `between(from, to]`. The first
  entry is keyed to the current version, `0.1.0` (the workspace version today), and covers what changed since v1.0 for
  app authors: the `UNDR` wire (ADR-033), frame-coalesced delivery (ADR-031), the Swift error channel and
  `UndraCallError` (ADR-032), reconnect (ADR-051), `bindgen --docs` (ADR-050), the build integrations. A test keeps
  the table sorted and not ahead of the CLI; `docs/RELEASING.md` now says to add the entry when a release is cut.
  Honest caveat in the entry: Kotlin and TypeScript keep their existing exceptions / `onError`; only Swift has
  `UndraCallError` so far.
* `--dry-run` prints each line as `- old` / `+ new` and the notes; the real run prints the same, writes, regenerates
  the bindings through the same code path as `undra bindgen` (a failure there is reported with "the pins are already
  moved") and prints the notes. `--no-bindgen` and `--docs` are additions.
* Tests: 27 unit tests (editors, versions, migrations, the notes printer), `tests/upgrade.rs` (12) on `tests/fixtures/upgrade/` (the
  released-0.0.9 project with every pin kind, a commit pin, registry versions in a workspace, a checkout, a project
  ahead, a fork) and one end-to-end test (`UNDRA_TEST_UPGRADE_E2E=1` or `UNDRA_REQUIRE_TOOLCHAINS=1`) that points the
  project at a *local clone of this repository standing in for GitHub* (git `insteadOf` through `GIT_CONFIG_*`, Cargo
  told to fetch with the git CLI), upgrades v0.0.9 to the CLI's tag for real and checks the bindings were regenerated
  (18 s).

## Counts

`cargo test -p undra-cli`: 329 pass (255 unit; 74 integration: `ci_workflow` 7, `doctor` 5, `upgrade` 12, `cli` 10,
`diagnostics` 11, `build_systems` 6 which skip without the env, ...). `cargo test --workspace`: 2,441 pass, 0 fail,
11 ignored; `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` clean. TypeScript runtime:
990 tests, `npm run typecheck`, `npm run build`; `undra bindgen -C examples/playground --check --docs` up to date.
`node site/scripts/build-all.mjs` and `check-links.mjs` clean.

## Open items for the integrator

1. **CI wiring** (`ci.yml` is not touched from a worktree): add a macOS job step `UNDRA_REQUIRE_TOOLCHAINS=1 cargo test
   -p undra-cli --test build_systems --test upgrade --test ci_workflow` (macos-15 has Xcode, the Android SDK, Node and
   Gradle; it needs `cargo-ndk`, the Rust targets and the NDK r27 as the generated workflow installs them). Until then
   the tests are only run by hand.
2. `site/data/roadmap.json` still lists `undra upgrade` under "Next"; it can move to shipped when this merges.
3. The generated workflow pins the CLI's own version (`0.1.0` today) and the project pins `tag = "v0.1.0"`: both fail
   until that release exists (the header comment of the workflow says so). `macos-14` is what the brief names; GitHub
   retires old images, so check it before the first release.
4. Not exercised: Android Studio and Xcode GUI builds (only `gradlew` and `xcodebuild`), a device (not simulator) iOS
   build (needs signing), Windows (the CLI has no Windows support).
5. Known edges, all documented in the README and the notes: sources outside `core/` (a monorepo's path dependencies)
   are not Gradle inputs unless added with `undraBuild { sources.from(...) }`; the Xcode input list gains a file the
   build after the first edit that adds one (one extra run of the phase); `undra upgrade` does not edit an older
   project's Xcode project, Gradle script or `vite.config.ts` to add the build integrations (the notes say to copy them
   from a fresh `undra init`); the playground apps are wired by hand and still take an explicit `undra build`.
6. The `UNDRA_VERSION`/migration-entry key `0.1.0` is the workspace version; when `scripts/bump-version.sh` makes it
   `1.0.0`, re-key or extend the entry (the test that keeps the table "not ahead of the CLI" passes either way).
