# Review: launch-dist (ADR-063, "Launch distribution from GitHub"), PR #14

Adversarial review of `wt/launch-dist` at `70cf7f7` (+3,192 −1,264, 88 files), 2026-10-03; the fixes land on `824dc20` (the
branch with main merged, #12 and #13). Everything below was run on
this Mac (Xcode, Android SDK + NDK r27, JDK 17, Node 24, Rust 1.99.0) unless it says otherwise; the implementer's
record was not taken as evidence.

## Verdict

**Approve with the fixes below, which are on the branch.** The design holds: a project `undra init` writes builds on
iOS, Android and the web from one tagged copy of the repository with nothing else, the rehearsal proves it and fails
when the distribution is broken, and every address a generated project carries differs from the rehearsal's only in
its base. No High finding survives; the Medium ones (a version pull request that would have gone red on launch day, the
bindings' bytes depending on the CLI's patch, silent overrides, a rehearsal that could test a stale CLI, an adopt step
that cannot work, checklist gaps) are fixed with a test or a rehearsal run that failed first. What only a real tag can
show is listed at the end; the checklist's release candidate (step 4) exercises all of it before `v1.0.0`.

## Findings

| # | Sev | Finding | Status |
|---|---|---|---|
| M1 | Medium | `scripts/bump-version.sh 1.0.0` leaves the version PR red: `crates/undra-bindgen/tests/generators.rs:182` hard-codes `"^0.1.0"` (Rust is not rewritten), and `tests/upgrade.rs` ties the migration notes' key to the CLI's version, so the `1.0.0-rc.1` PR of step 4 fails too (it is re-keyed only at step 5). Found by running the bump to 1.0.0 on a copy and the CLI and bindgen suites. | Fixed (`28c27c8`: the test reads `RUNTIME_RANGE`; `d6b8bd7`: the upgrade test reads the key from `migrations.rs`) |
| M2 | Medium | The generated `package.json` asks for `^<generator version>` while the Swift package and Kotlin module name `[undra] version`: contrary to ADR-063 §1, `undra bindgen --check` in a project's CI fails when the developer's `undra` is another patch, and C0007 blames "the core changed". | Fixed (`28c27c8`): `Generator::ts_runtime_range`, set from a released project's version |
| M3 | Medium | `UNDRA_DIST_*` were honoured silently by `init`, `adopt`, `upgrade` and **every** `undra bindgen`, and not kept: a project made against a mirror got GitHub's URL in `generated/swift/Package.swift` at the next `undra bindgen` without them (shown on the rehearsal's project), and its CI's `--check` failed. Never at build time (`undra build` does not read them). | Fixed (`1734f88`): a warning per variable in all four commands; `undra bindgen` follows the core's `undra` git URL (`core/Cargo.toml`) |
| M4 | Medium | R7/R8: an `undra` of another release than the project's pins says nothing; `--check` then reports "the core changed (or they were edited by hand)". | Fixed (`1734f88`): a warning naming both releases and what to do (`undra upgrade` only when the CLI is newer); C0007's why/fix say it |
| M5 | Medium | The rehearsal could test a stale CLI: the snapshot kept the files' modification times while the CLI target directory is shared between rehearsals. Shown: `dist.rs` changed to drop the `v` of the asset URL, rehearsal "web ok", `undra CLI ok (0 s)`. | Fixed (`0ca510c`, `tar -xmf`); the same sabotage now fails (`web FAIL`, the asset is a 404) |
| M6 | Medium | `undra adopt`'s web step for a released project (`npm install <asset> <generated dir>`) cannot work: the generated package is not built (its exports name `dist/`) and, linked from outside the app, its `@undra/runtime` does not resolve (shown: `ERR_MODULE_NOT_FOUND`); building it needs `@undra/runtime` from the registry. `docs/REACT_NATIVE.md` did not say how an RN app takes its bindings. Not new with this piece, but it is the released path now. | Fixed (`9d3db06`, `915d9df`): alias + `dedupe` + `paths`, as `init`'s web app; REACT_NATIVE.md says how Metro resolves them |
| M7 | Medium | `swift_manifests.rs` compared products, dependencies, headers and paths only: a `swiftSettings`/`resources`/`exclude` added to a runtime target would not reach the root package. | Fixed (`28ae1a6`): whole-target comparison, shown to fail on an added `swiftSettings` |
| M8 | Medium | `docs/RELEASING.md` step 4 sends the founder through step 9's `brew` lines for a prerelease the tap never gets. | Fixed (`915d9df`) |
| M9 | Medium | Supply chain, not in ADR-063: the `@undra` npm scope is unclaimed while `@undra/react-native`, `@undra/testkit` and generated packages name `@undra/runtime` by a range and npm auto-installs a missing peer from the registry; JitPack's trust (it builds and serves unsigned bytes) and how an app pins it were not stated. | Documented (`915d9df`): ADR-063 §4 and Negative, RELEASING.md (reserve the scope; record JitPack's sha256s; Gradle dependency verification). **The founder's step.** |
| M10 | Medium | The *Launch rehearsal* workflow had never passed: on 70cf7f7 bash 3.2's empty array (fixed by the implementer, 17d5ded), on 824dc20 `bump-version.sh: cargo update --workspace --offline failed` 5 s in, because a fresh runner's Cargo cache lacks the workspace's dependencies. Reproduced with an empty `CARGO_HOME`; a founder's fresh clone hits the same at RELEASING step 5. | Fixed (`c3abbd2`): the rehearsal runs `cargo fetch --locked` before the bump; the script's error says to run `cargo fetch` (shown on an empty `CARGO_HOME`: only the workspace's 17 entries of Cargo.lock move). The run on 7b36eec then stopped at `test-npm-assets.sh`: "the web server did not start", log empty after 10 s with the server alive (`python3 -m http.server` resolves the host's name before it prints; slow on the macOS runner). Fixed (`7c2e2fe`): `packaging/serve-dir.py`, the same server without the lookup, 30 s wait |
| M11 | Medium | RELEASING step 5's hand re-key of the first migration entry (`0.1.0` → `1.0.0`) would itself turn the version PR red: `migrations.rs`'s unit test hard-codes `0.1.0`. | Fixed (`ce83e35`): the version script files notes kept under a never-released version (no tag `v<old>`) under the new one and leaves a released one's; the unit test reads the key. A copy bumped to 1.0.0 with no hand edit: 37 test binaries pass. RELEASING step 4 (the rc) is now **required** |
| L1 | Low | The runtimes' `Hello` versions (`RUNTIME_VERSION`, `UNDRA_RUNTIME_VERSION`) were not moved by the version script (the TS comment named a test that does not exist); the release would report `undra=0.1.0` from a 1.0.0 runtime in `undra dev`'s log. Swift's `UndraCore.undraVersion` is documented as the protocol's and left. | Fixed (`d6b8bd7`: script, `--check`, test) |
| L2 | Low | `launch-rehearsal.yml` did not run for changes of the runtime packages' manifests, the Vite plugin or the Swift runtime's manifest. | Fixed (`0ca510c`) |
| L3 | Low | The playground RN lock keeps `0.1.0` for its two linked packages after a bump (`npm ci` accepts it: run on the bumped copy). `bazel/MODULE.bazel` and its synthesized runtime say `0.1.0`. | Left (cosmetic; Bazel's own version) |
| L4 | Low | SwiftPM mirrors every advertised ref, PR heads included: GitHub reports 51,064 KB; the ADR said 47 MiB (a local mirror). | ADR updated |

## What was run

* `packaging/rehearse-launch.sh` from a clean directory (web, iOS, Android) on `70cf7f7`: all ok - `web 15 s`, `iOS 24 s`,
  `Swift package undra 0.1.0-rehearsal.1 from the bare clone`, `Android 22 s`. With the fixes and `--version 1.0.0` (the
  launch's own shape: tag `v1.0.0`, `from: "1.0.0"`, `:v1.0.0`, `.../v1.0.0/undra-runtime-1.0.0.tgz`): all ok, `init` printing
  the three override warnings, and in that project, without the overrides, `undra bindgen --check` up to date and
  `npm ls @undra/runtime` one copy (1.0.0). On the final head (default version): see the summary at the end.
* Sabotage: the root `Package.swift` deleted → `iOS FAIL` (`/Package.swift doesn't exist`), exit 1. The asset URL without
  `v` → false pass before M5's fix, `web FAIL` after.
* Lines compared: Cargo `git = <base>, tag = "v<v>"`; Swift `<base>` `from: "<v>"`, identity `undra` either way; Kotlin
  `com.github.shreypdev.undra:<module>:v<v>` from `<repo>` with `includeGroup`; npm `<base>/v<v>/undra-runtime-<v>.tgz`.
  All built by the same code (`dist.rs`); the overrides replace the base only.
* SwiftPM: a local tagged clone with `v1.0.0-rc.1`, `v1.0.0`, `v1.0.1-rc.1`, `v1.1.0-rc.1`; `from: "1.0.0"` resolves 1.0.0.
  No `unsafeFlags` in either manifest; the root package has no test targets; `swift build` at the root: ok.
* JitPack's command into a fresh local repository: six modules, POMs and `.module` files with
  `com.github.shreypdev.undra` / `v<v>`, inter-module dependencies on the same group and version, okhttp 4.12.0,
  coroutines 1.6.4, Kotlin 2.0.21; the Android app built against only that repository plus Google and Maven Central.
* npm: the web app's lock has the tarball's `resolved` URL and `integrity`; `npm ls @undra/runtime` in the web app and in
  an RN-host stand-in (RN tarball, runtime tarball, generated package, no registry): one copy each. pnpm and yarn are
  not installed here: not verified (both document tarball URLs).
* Version script: bump to 1.0.0 on a copy, then `cargo test -p undra-cli -p undra-bindgen` (M1: two failures, both fixed),
  `npm ci` of the RN playground (ok); `--check` names a file put back; malformed versions (`1.0`, `v1.0.0`, `1.0.0+build`,
  `01.0.0`) refused; idempotent (the test's second run).
* The fixes: `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo doc` with `-D warnings`, `cargo test -p
  undra-cli -p undra-bindgen` (37 binaries, 0 failed), `bash scripts/bump-version.test.sh`, the Kotlin `test-local.sh`
  (891 + 32 cases, 0 failed), the TypeScript runtime (typecheck; 2,043 tests), the playground web build, `undra bindgen
  --check --docs` on cookbook, fieldbook, playground, two-cores a and b (up to date; `ios15-sample` is checked without
  `--docs` in CI and is up to date that way), `swift build` of the root and the in-repository packages,
  `node site/scripts/build-all.mjs`, `check-links` (54 pages) and `--words`.
* `release.yml` read job by job; no npm-wrapper step or secret left; `packaging/homebrew/generate.sh` with a fake release
  (four CLI tarballs and three `.tgz` in `checksums.txt`): valid Ruby, the four URLs and hashes; `packaging/test-install.sh`:
  all checks pass; the runtime tarball holds both builds (`dist/` and `dist/dev/`) and the `exports` conditions, and the
  Vite hooks survive the production build.
* The Vite fix, live, on the rehearsal's released project (the tarball's `dist/vite.js`): `?undra=` page kept its items
  through two core edits without `UNDRA_SKIP_BUILD` (console: "kept its state"), the wasm page reloaded; a schema change
  left the served page with "The schema changed, state reset: run undra bindgen, then reload", and `undra bindgen`
  reloaded it through Vite. `test/vite.test.ts` with main's `vite.ts`: 7 of 32 fail. Devtools icon: the mark, with a test.

## CI on the way

* **Bench / Budgets (host, release), 824dc20: noise.** `stream/backpressure` throughput 12.41 M/s against the base
  measured in the same job at 18.97 M/s (gate: 1.5x), three attempts. The runtime is byte-identical to main's (`git diff
  origin/main 824dc20` over `crates/undra-{runtime,ffi,wire,meta,ports,macros}`, `crates/undra`, `bench`, `Cargo.lock`:
  empty). The row on main's last three Bench runs: 12.1 to 12.5 M/s (Xeon 8370C), 18.8 (EPYC 7763), 24.1 (EPYC 9V74);
  on 7b36eec, with this piece's code: 17.2 to 17.3 M/s, Bench green. No size row moved (Web size and iOS size green).
* **CI / Kotlin runtime (JVM), 7b36eec: a timing test.** `RemoteReconnectTests > the policy gives up after maxAttempts
  and closes the core as failed` timed out at 10 s; this piece changes no Kotlin source (build files and READMEs only),
  the job runs `test-local.sh` (kotlinc, no Gradle), which passes here (891 + 32 cases), and the job has not failed in
  the 40 CI runs before. Re-run on the final head.

## The *Launch rehearsal* workflow

Not one of the four "All green" checks, and it should not run on every pull request: its setup alone (Xcode, the NDK,
cargo-ndk) took four minutes on `macos-15` before the rehearsal step, and a cold rehearsal compiles the CLI and the core for
wasm, the simulator and two Android ABIs (locally, warm: about two minutes; the implementer estimated 20 to 30 minutes cold
on a runner). It is already both on demand (`gh workflow run launch-rehearsal.yml`, `-f version=`) and path-filtered to what
a released project is made of (the templates, `dist.rs`, the bindgen code, the version script, `packaging/`, `release.yml`,
`jitpack.yml`, `Package.swift`, the Kotlin build files, and, from this review, the runtime packages' manifests, the Vite
plugin and the Swift runtime's manifest). It runs before a release by hand as RELEASING.md says.

## What only a real tag can show

JitPack's hosted build (whether it sets `ANDROID_HOME` and has platform 35 / build-tools for AGP 8.7.3, its `GROUP`/`ARTIFACT`
values, that it serves `.module` files); SwiftPM and Xcode against github.com (the 50 MiB mirror); `gh release create` with
the seven assets; the tap push and `brew install`; npm following GitHub's asset redirect. Step 4 (`v1.0.0-rc.1`) runs every
one of them before `v1.0.0`; the JitPack build is the one most likely to need a retry or a patch.

## The rehearsal on the final head

```
Rehearsal of Undra 0.1.0-rehearsal.1
  remote (bare clone, tag)     ok   v0.1.0-rehearsal.1 at file://…/remote/undra.git
  undra CLI                    ok   undra 0.1.0-rehearsal.1 (unknown) (4 s)
  npm assets (3 tarballs)      ok   http://127.0.0.1:<port>/v0.1.0-rehearsal.1 (6 s)
  Kotlin modules (JitPack)     ok   com.github.shreypdev.undra:*:v0.1.0-rehearsal.1 (13 s)
  undra init                   ok   core pins v0.1.0-rehearsal.1 of the remote
  web (npm run build)          ok   built without the checkout in 13 s
  iOS (xcodebuild, simulator)  ok   built without the checkout in 23 s
  iOS: Swift package           ok   undra 0.1.0-rehearsal.1 from the bare clone (Package.resolved)
  Android (assembleDebug)      ok   built without the checkout in 19 s
Every platform built from the rehearsal release alone.
```

## Should block the launch

Nothing in the code. Before announcing: step 4 green end to end, and the founder's decision on reserving the `@undra` npm
scope (M9).
