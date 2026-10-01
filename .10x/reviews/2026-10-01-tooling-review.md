# Track D2–D5 tooling (`undra doctor`, build-system integration, CI from `undra init`, `undra upgrade`) — adversarial review

**Date:** 2026-10-01 · **Reviewer:** fable (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/tooling` at
`e3207e1` (cut from `a0d638f`; 95 files, +11,076/−899), `main` merged twice on the branch (`6cb7a71` at `e518653`: parity,
React Native, `cas_update`, CI on 1.98.1; `2bea5d9` at `38ea11d`: Android adapters, Track A) · **Read:** `CLAUDE.md` (R4, R6,
R8, R9), the design spec's Track D, the SDE record `.10x/decisions/sde/tooling.md`, `docs/SPEC.md` 12 and 13, `docs/DEV_LOOP.md`,
`docs/ONBOARDING.md` 1, `docs/RELEASING.md`, `docs/ERRORS.md`, ADR-031/032 (+A)/033/034/035/036/050/051, `.10x/status.md`, and the
diff (`commands/doctor/*`, `commands/upgrade.rs`, `upgrade.rs`, `migrations.rs`, `semver.rs`, `ci.rs`, `builds/xcode.rs`,
`commands/build.rs`, `commands/init.rs`, the `android`, `ios`, `web` and `ci` templates, `runtimes/ts/@undra/runtime/src/vite.ts`,
the six new test files and the fixtures) · **Method:** a failing test for every bug before its fix (run red against the old code
where the fix was a rewrite); real `gradlew`, `xcodebuild`, `npm run build` and `vitest` runs on fresh `undra init` projects in
the scratchpad; the real `undra doctor` under the plain shell, `scripts/env.sh`, `env -i PATH=/usr/bin:/bin` (with and
without `HOME`), inside and outside a project; `undra upgrade` on the playground, on copies of the fixtures, on crafted look-alike
projects · **Fixes:** `da4ebc8`, `e44910e`, `c5ab79e`, `925b3c8`, `2b246c0`, `66f6ccd`, `afd2173`, `ef4213b`,
`5710aea`, `02aeec2` (and this record).

## Verdict

**Merge after fixes; the fixes are on the branch.** The three build integrations do what the record says, and this review
saw each one with its own eyes on fresh projects: Gradle's `undraBuild` runs before `preBuild`, is `UP-TO-DATE` when nothing
changed, rebuilds on a `core/src` edit, and switching debug → release → debug leaves the right core in `jniLibs` **and in the
APK** each time (32.9 MB debug, 1.1 MB release, 32.9 MB debug again; `unzip -lv` of the APKs), also under Gradle's configuration
cache (stored, reused, still switching); Xcode's phase runs before Compile Sources, is skipped while nothing changed, and Debug
→ Release → Debug ends on the debug static library (58.3 MB / 7.2 MB / 58.3 MB) with the app relinked against the new core (a
string changed in `lib.rs` is in `<App>.debug.dylib` after the rebuild, although the library is named only in
`OTHER_LDFLAGS`); `npm run build` starts Vite, the plugin builds the wasm core and the bundle carries it. The three "undra is
missing" paths print C0003 with the install command. Nothing a repository ships as *data* can choose which binary the
integrations run (surface 1 below).

What was wrong is concentrated in **`undra upgrade`** and **`undra doctor`**, the two pieces that edit other people's files
and tell people what to type. `upgrade` rewrote a third-party Swift package's version because its URL contained `undra`
(M1), left a half-upgraded project when one write failed (M2), rewrote Gradle comments and turned a `$undraVersion` variable
into a literal (L2), ignored `@undra/react-native` (L3, a merge interaction) and refused its own projects when the CLI is a
release candidate (L1). `doctor` printed fixes that do not run in the shell it diagnosed: `brew install node` with no `brew`
on `PATH` (or with node already installed), Apple-silicon paths on Intel, a keg-only JDK left off `PATH`, Linux one-liners
whose second half needs a new shell (M5). One finding sits under all three integrations and predates the piece: the platform
libraries were built from a **copy of `Cargo.lock` taken at the first build**, so `cargo update` never reached a shipped app
even though Gradle re-ran its task for it (M3). The Vite plugin compiled the core on every Vitest run (M4). The migration
notes became false when parity merged and silent about Track A's breaking write rule (M6). Every finding is fixed except the
open items at the end; no High finding.

## Findings

| # | Sev | Where (at `e3207e1` unless noted) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `crates/undra-cli/src/upgrade.rs:835` | The pbxproj editor took every `XCRemoteSwiftPackageReference` whose `repositoryURL` *contains* `undra` (any case) for Undra's: a project depending on `https://github.com/acme/undra-charts` (`exactVersion 3.4.5`) had that requirement rewritten to Undra's release line, silently re-pinning someone else's package. | **Fixed** (`da4ebc8`): the URL is compared with `UNDRA_SWIFT_PACKAGE_URL` (new constant, also used by `init`) in the spellings Xcode writes (`.git`, ssh, case, trailing slash). Test `a_swift_package_whose_url_merely_contains_undra_is_not_moved` (red before). |
| M2 | Medium | `crates/undra-cli/src/commands/upgrade.rs:93-95` | Writes were sequential `write_if_changed`: a file that cannot be written (read-only `web/package.json`, the last of six) stopped the command after five files had moved, the "pins in lockstep" invariant broken on disk. A project whose core was missing had every pin moved and then failed at bindgen. | **Fixed** (`da4ebc8`, `ef4213b`): `write_all` checks every file is unchanged since it was read and opens it for writing before the first write, restores the written ones if a later write fails, and says "nothing was written" (C0010); a missing core is C0005 before any write (`--no-bindgen` still moves the pins). Tests `a_file_that_cannot_be_written_leaves_every_file_as_it_was`, `a_project_without_its_core_is_refused_before_anything_is_written` (both red before). |
| M3 | Medium | `crates/undra-cli/src/shim.rs:143` (pre-existing; exposed by D3's "Cargo.lock is an input"); `builds/xcode.rs:63` | The shim (what `undra build` compiles for iOS, Android and the web) seeded its `Cargo.lock` from the project's **once**. Measured: `cargo update -p itoa --precise 1.0.5` in a fresh project, then `gradlew assembleDebug`: the task re-ran (Cargo.lock is a Gradle input) and the library was still built with itoa 1.0.18. Xcode's input list did not name `Cargo.lock` at all; the Vite plugin did not watch it. | **Fixed** (`c5ab79e`): the shim and the dev runner re-seed whenever the project's lock file changes (a hash in `.undra-lock-seed`; Cargo's own completions are kept while it does not); `Cargo.lock` is an Xcode input and a Vite watch target. After the fix the same experiment compiles itoa 1.0.5 for both ABIs. Test `the_shim_lock_follows_the_projects_lock_file_and_keeps_what_cargo_added_otherwise` (red against the old seeding); `build_systems` asserts a `Cargo.lock` change re-runs the Gradle task. Caveat: a re-seed resolves the shim's own extra packages again, which reads the crates.io index (it printed `Updating crates.io index`), as a first build always did. |
| M4 | Medium | `runtimes/ts/@undra/runtime/src/vite.ts:275` | Vitest loads `vite.config.ts` and calls `buildStart`: adding Vitest to the generated web app made **every test run** a release wasm build of the core (measured with a fake `undra` under `vitest run`: one `undra -C … build --platform web` per run), so a web developer's tests needed the Rust toolchain. No `apply`/mode gate existed. | **Fixed** (`c5ab79e`): nothing is built or watched in Vite's mode `test` unless `undra({ inTests: true })`; `ViteConfigLike` gained `mode?`. Re-measured: zero calls under `vitest run`. Test `builds nothing under Vitest (mode test) unless asked to` (red before). Still assignable to the real Vite 8 `Plugin` under `strict` + `exactOptionalPropertyTypes` (scratch `tsc`). |
| M5 | Medium | `commands/doctor/web.rs:34-36`, `android.rs:129, 241, 273, 429, 462, 502`, `system.rs:116` | Fixes that do not run where they are printed. Under `env -i HOME=$HOME PATH=/usr/bin:/bin` (the GUI/fresh-shell case doctor exists for): `wasm-opt` installed at `/opt/homebrew/bin` was "not found, fix: brew install binaryen" (brew is not on that `PATH`; binaryen is installed); every brew fix assumed `brew` on `PATH` and printed nothing for a Mac without Homebrew; the SDK fix exported `/opt/homebrew/...` (wrong on Intel, where Homebrew is `/usr/local`); `brew install openjdk@17` alone leaves the keg-only JDK off `PATH`; `adb kill-server` named a bare `adb` that was found only in the SDK; on Linux `curl … get.sdkman.io \| bash && sdk install kotlin` and `fnm install` need a new shell, and `apt-get install gradle` installs Gradle 4.4, which cannot run on JDK 17. | **Fixed** (`2b246c0`): `brew(cx, args)` prefixes `eval "$(<brew> shellenv)"` when Homebrew is installed but off `PATH` and Homebrew's installer plus a shellenv that works on both prefixes when it is missing (the `--fix` block installs it once); a Homebrew tool out of `PATH`'s reach is reported as installed there with only the shellenv line; the cask path is `$(brew --prefix)/share/...`; the JDK fix exports `JAVA_HOME` and `PATH`; adb fixes use the quoted path found; SDKMAN/fnm are sourced in the same shell; Gradle on Linux comes from SDKMAN. Tests `a_tool_homebrew_installed_off_path_is_named_and_the_fix_puts_homebrew_on_path`, `without_homebrew_a_brew_fix_installs_homebrew_first`, `homebrew_paths_are_asked_of_brew_not_assumed_to_be_apple_silicons`, `adb_fixes_name_the_adb_that_was_found` (all four red before); the described "everything installed" Mac now has Homebrew on `PATH`, and the old assertions that expected `brew install X` on a Mac with no Homebrew were corrected. |
| M6 | Medium | `crates/undra-cli/src/migrations.rs:87` (after the merge) | The 0.1.0 notes said "only Swift has `UndraCallError` so far" — false once parity merged (Kotlin and TypeScript have the same closed set, ADR-032 amendment A) — and said nothing about Track A, whose write rule (ADR-035) makes a core that writes a store from a `spawn_blocking` worker or a host thread panic in **release** builds too. | **Fixed** (`da4ebc8`, `e44910e`): a `[do]` note for Kotlin/TypeScript (what to catch now, commands report to `onError`), a `[do]` for the write rule (E0065, `try_set`, `with_core`), a `[changed]` for `WeakCtx` and the isolated computed, a `[new]` for typed stream ends (ADR-036); "every runtime has a drain listener". Each claim was checked against `docs/ERRORS.md`, SPEC 5/16 and the ADRs. |
| L1 | Low | `crates/undra-cli/src/upgrade.rs:1006` | `ahead()` compared release-line pins with the target itself: `undra init` from `1.0.0-rc.1` writes `^1.0.0` / `1.0.0` / `"1.0"`, and `undra upgrade` from the same CLI refused that project with C0014 (`1.0.0 > 1.0.0-rc.1`). RELEASING describes release candidates. | **Fixed** (`da4ebc8`): a line pin is compared with the target's release; an exact pin with the target. Test `a_prerelease_undra_is_not_behind_the_release_line_it_writes`. |
| L2 | Low | `crates/undra-cli/src/upgrade.rs:762` | The Gradle editor rewrote `dev.undra:runtime:` anywhere on a line: in `//` and `/* */` comments, and it replaced a variable (`dev.undra:runtime:$undraVersion`, `${undraVersion}`) or a dynamic `+` with a literal, orphaning the variable's definition. | **Fixed** (`da4ebc8`): a comment mask (strings, line and block comments across lines); only a written-out version moves; a variable or `+` is reported (`Why::NotALiteral`, a warning naming the line). Tests `gradle_moves_literal_versions_only_and_never_a_comment`, `a_gradle_version_variable_is_reported_and_left`. |
| L3 | Low | `crates/undra-cli/src/upgrade.rs:707` (merge interaction with React Native) | `@undra/react-native` (peer-depends on the same line of `@undra/runtime`) was not a pin: an RN app's `package.json` ended with the runtime on the new line and the RN host on the old. | **Fixed** (`da4ebc8`): both packages move; `@undra/runtime-extras`, a `description` that quotes the name, `file:` RN dependencies are left (tested). |
| L4 | Low | `runtimes/ts/@undra/runtime/src/vite.ts:275, 289` | "One build at a time" covered rebuilds only: a save while `vite dev` was still doing its first build started a second `undra build` next to it (measured `start, start, end, end`), two processes writing `build/web/undra_core.wasm` through `wasm-opt`. | **Fixed** (`c5ab79e`): the first build shares the single-flight state; a save during it is built once after it. Test `never runs a rebuild next to the first build` (red before). |
| L5 | Low | `crates/undra-cli/templates/ci/install-undra.yml:3` | The workflow piped the site's **latest** `install.sh` to `sh`. The binary was pinned (`UNDRA_VERSION`) and checksummed, but the script that does the checking was a moving target served by every site deploy. | **Fixed** (`925b3c8`): `curl -fsSL "https://raw.githubusercontent.com/shreypdev/undra/v${UNDRA_VERSION}/site/install.sh" \| sh`, the installer of the pinned release's tag; `undra upgrade` still moves only `UNDRA_VERSION`. Test `the_workflow_holds_no_secret_and_reads_only` (permissions `contents: read` and nothing else, no `secrets.`, no `pull_request_target`, every piped script pinned). |
| L6 | Low | `crates/undra-cli/src/error.rs:47` | The errors page takes a C code's title from the first doc-comment line; the branch's two-line comment made C0014's title "…, or the project is". The upgrade's C0014 message had no golden, so the page showed only the other meaning. | **Fixed** (`afd2173`): a one-line title; golden `tests/golden/diagnostics/C0014-upgrade.txt` (test `c0014_a_project_on_a_newer_undra_than_this_one`); page regenerated (45 codes, 138 real messages). |
| L7 | Low | `crates/undra-cli/src/upgrade.rs:555` | A `[dependencies.undra]` section's `tag = "v0.1.0" # the release` lost its comment (the module doc promises comments survive). | **Fixed** (`da4ebc8`), in `cargo_look_alikes_are_left_alone`. |
| L8 | Low | `docs/DEV_LOOP.md:176`, SPEC 13 | "All three find `undra` on `PATH` (`UNDRA_BIN` names one)": the Xcode phase does not read `UNDRA_BIN` (its environment is the build settings). SPEC said the same. | **Fixed** (`c5ab79e`): the docs say Gradle and Vite take `UNDRA_BIN`, Xcode searches `PATH` and the install directories. Not changed in the phase on purpose (surface 1). |
| L9 | Low | `crates/undra-cli/src/commands/doctor/android.rs:462` | A JDK installed off `PATH` was a warning even inside a project with an Android app, where `./gradlew` cannot start; the record's own rule makes a missing JDK a failure there (seen on the playground: `android.jdk warn`). | **Fixed** (`66f6ccd`): a failure in a project, a warning elsewhere; asserted in `the_jdk_must_be_17_or_newer`. |
| L10 | Low | `crates/undra-cli/src/migrations.rs:149` | `it_starts_with_an_entry_for_the_current_version` asserted the first entry's key equals `CARGO_PKG_VERSION`: `scripts/bump-version.sh 1.0.0` turns the release pull request red, and after 1.1.0 the first entry can never equal the version. | **Fixed** (`da4ebc8`): the test checks the entry by its content and that it is not ahead; `docs/RELEASING.md` says to re-key it to `1.0.0` in the first release's pull request (open item 3). |
| L11 | Low | `crates/undra-cli/src/commands/doctor/mod.rs:173` | `eprintln!` in library code (CLAUDE.md). | **Fixed** (`2b246c0`): `Ui::note`. Pre-existing `println!` in `commands/dev.rs` and `eprintln!` in `lib.rs` are outside this piece (note). |
| I1 | Info | `templates/android/app/build.gradle.kts` (`whenReady`) | One task, one output directory: a build that asks for both variants (`./gradlew build`) gets a release core in both APKs. | Documented in DEV_LOOP. |
| I2 | Info | all three integrations | `.cargo/config.toml` (measured: a `rustflags` change left `undraBuild` `UP-TO-DATE`), `rust-toolchain.toml`, a new `rustc` and a new `undra` are inputs of none of them. | Documented in DEV_LOOP with the one command each. |
| I3 | Info | `undra init` + Xcode | The first build writes `Cargo.lock`, the refreshed input list then names it, so the second Xcode build runs the phase once more (a no-op Cargo), then it is stable; `build_systems` allows it as it already did for Gradle. | Noted in the test. |

## The attack on each surface

**1. Security of what `init` emits and `upgrade` rewrites.** *Can repository data choose the binary?* The Xcode phase runs
`command -v undra` over `$HOME/.undra/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH` and then `exec env -i … undra`;
it reads no `UNDRA_BIN`, so a checked-in `.xcconfig` cannot point it elsewhere (it could set `PATH`, but an `.xcconfig` and the
`project.pbxproj` that holds the script are both the repository's build code, which can run anything when built: nothing runs on
*open*). `undra.toml` and `.cargo/config.toml` change what `undra build` compiles, with exactly the trust `cargo build` already
gives a cloned project's `build.rs` and proc macros. Gradle reads `UNDRA_BIN` from the **environment** only
(`System.getenv`), never from `gradle.properties`; the only properties are `undraSkipBuild`, `undraRelease` and the dev-loop's
`undraDevUrl` (I note that the last is interpolated into a Java string literal unescaped, pre-existing and repository-controlled,
so not a new vector). The Vite plugin `spawn`s without a shell, with fixed arguments (`-C <root> build --platform web`); the
command is the `command` option of `vite.config.ts` (code) or `UNDRA_BIN` / `PATH` / the install directories. A *relative* `PATH`
entry (`.`) would resolve against the build's working directory in Gradle and Vite as it would in any shell; that is the user's
`PATH`. *The workflow*: `permissions: contents: read` and nothing else, no secret, `pull_request` (not `_target`), so a fork's PR
gets a read-only token and nothing to steal; the binary was pinned and sha256-checked, the installer was not (L5, fixed). Third-
party actions are pinned by major tag (`dtolnay/rust-toolchain@stable` is a branch), not by SHA: open item 6. *`upgrade`'s
rewriting*: a crafted project with a commented `# undra = { …, tag = "v0.0.1" }`, a `undra-something` from Undra's repository,
`@undra/runtime` in `devDependencies` next to `@undra/runtime-extras`, a `description` quoting the name, a Gradle comment and a
variable, a second Swift package with `undra` in its URL, a dotted section with a commented `# tag =` and a trailing comment:
before the fixes three of these were rewritten (M1, L2, L7) and one real pin was missed (L3); after, only the real pins move
(five new unit tests). CRLF: every editor keeps `\r\n` on changed and unchanged lines (`every_editor_keeps_crlf_line_endings`).
A pbxproj edited by Xcode after `init` (reordered keys, more phases): `upgrade` does not look for the build phase at all — it
moves only the package requirement, which Xcode writes in the same shape — and the notes say to copy the phase from a fresh
`init`, so it neither breaks nor claims to update it.

**2. Up-to-date logic.** Gradle (fresh project, `--undra-path`, shared target): `assembleDebug` runs `undraBuild` then the
APK; again: `UP-TO-DATE` (after one extra run while the first build settles `Cargo.lock`); `core/src` edit: runs; `Cargo.lock`
edit: runs (and, after M3, builds what it names); `.cargo/config.toml`: not an input (I2); a path dependency outside `core/`:
not an input unless added with `sources.from(...)`, which the generated README and DEV_LOOP say. Debug → release → debug in
one daemon: the third build is debug again in `jniLibs` and in the APK (now asserted in `build_systems`); the same under
`--configuration-cache`. Xcode: three builds Debug → Release → Debug: the phase runs each time a configuration changes (the
stamp), the simulator static library is 58.3 MB, 7.2 MB, 58.3 MB, and an edit to `lib.rs` is relinked (asserted now: the new
string is in the app bundle; the release size is asserted against the debug one). Vite: two saves within 150 ms are one build;
a save during a build is exactly one more; two saves during a build are still one more (`again` is overwritten); a failing
rebuild sends `{type: "error"}` to the overlay and no reload, and the old `build/web/undra_core.wasm` stays (the web build writes
it only after Cargo succeeds; `wasm-opt` failure falls back to a copy); the first build was outside the single flight (L4,
fixed). `vite build` and `vite dev` build; `vite preview` does not; Vitest did (M4, fixed).

**3. `undra doctor` honesty.** Real runs on this Mac (after the fixes): with `scripts/env.sh`, outside a project 26 ok, 3
warn, exit 0, inside the playground 28 ok, 2 warn; the plain shell outside (before the fixes) 24 ok, 5 warn; `env -i
PATH=/usr/bin:/bin` without `HOME` 10 ok, 12 warn, 5 fail, exit 1 (Rust "not found" is true there: `~/.cargo` cannot be named
without `HOME`); the same with `HOME` 20 ok, 7 warn, 1 fail (node, which comes from fnm on this machine), and inside the
playground 22 ok, 6 warn, 2 fail (node, and the off-`PATH` JDK since L9). Only the project's platforms are checked inside one;
`--platform tv` is C0009. No panic in any. The exit status is 1 exactly when a `fail` finding exists. `--json` parses, its keys
are serde_json's sorted map (two runs are identical apart from observed values such as free disk), and `--fix` and `--json`
are exclusive (clap). The `--fix` path: `run` computes the same
report (the checks run read-only queries: `--version`, `rustup show active-toolchain`, `xcrun simctl list runtimes`,
`adb devices`, which starts the adb server if it is down, `emulator -list-avds`, `df -Pk`) and prints `fix_script()`, a pure
string; there is no `Command::new` on the fix path and nothing in the script is executed. Each fix was read against what it has
to do on macOS and Linux; M5 lists the ones that did not work and how they are now written. A missing `adb` reports only
`android.adb` (warn) and no `android.device` finding; free disk is independent of it. `kotlinc`: `undra build --platform
android` needs `cargo-ndk`, the NDK and the Rust targets; Gradle needs a JDK and compiles Kotlin itself; `kotlinc` is used only by
the repository's Kotlin runtime tests and bindgen's `typecheck_kotlin`, so "for contributors" is right.

**4. `undra upgrade`.** The playground (`path` dependencies): lists the five checkout pins (`undra.toml` path, the `-SNAPSHOT`
Gradle runtimes, the local Swift package, the RN app's `file:` runtime) and changes nothing, `--dry-run` or not. A copy of the
0.0.9 fixture: `--dry-run` shows the seven lines in six files and the notes and writes nothing; the real run moves them (tests
cover byte-for-byte equality with a fresh `init`; the end-to-end test regenerates the bindings against a local clone). Newer
than the CLI: C0014, SPEC 12 row and errors page both carry the meaning (L6 fixed the page title and added the golden). A
`rev` pin: becomes `tag = "v<CLI version>"` in place, features and other keys kept, and the project's version is "an unreleased
commit" when no other pin names one — now said in `--help` and on the cli page. Missing files: a project without a workflow or
without `package.json` is a project with fewer platforms, and the upgrade moves what exists; a missing core is refused before
any write (M2); a file that cannot be written leaves the tree as it was (M2). Migration notes: compared with `docs/ERRORS.md`,
SPEC 5.1/5.6/16 and ADR-031/032/033/034/035/036/050/051 (M6). The `0.1.0` key: see open item 3.

**5. R8 for the CLI codes.** No new code. C0003 (the three integrations: Gradle and Vite in the four-line shape, Xcode as one
`error: [undra::C0003] …` line so the issue navigator shows it; each has the install command and the docs link; asserted by
`build_systems` and `vite.test.ts`) and C0014 (the upgrade refusal: what, why, fix, docs; golden added) are in SPEC 12's table;
`node site/scripts/build-errors.mjs` regenerates the page (45 codes, 138 real messages) and it is committed; the macros
catalogue test passes with the extra golden.

**6. Everything else.** Counts below. `npm pack --dry-run` of `@undra/runtime`: `dist/vite.js`, `dist/vite.d.ts` (+ maps) and
`src/vite.ts` are in the tarball, `exports["./vite"]` names them, `dependencies` is empty. Docs on every new `pub` item
(`cargo doc` is part of CI's `rust` job; clippy clean). `Cargo.lock` unchanged except what the merges brought. **CI wiring**
(`ci.yml`): the `rust` job (Linux) runs `build_systems` (the Vite tests), `upgrade` (end to end), `ci_workflow` and `doctor` with
`UNDRA_REQUIRE_TOOLCHAINS=1`; the `macos` job runs the Xcode tests with `UNDRA_REQUIRE_TOOLCHAINS=1` after it adds the iOS
targets; the `android` job adds Temurin 17, NDK r27.2 and platform 35 and runs the Gradle tests with `UNDRA_TEST_BUILD_SYSTEMS=1`
(they run where the image has the toolchain and print why they skipped where it does not). None of these steps has run on a
GitHub runner from here (open item 1). `site/data/roadmap.json`: the merge already moved the tooling item out of "Next" into
"Now" ("No manual build step"); moving it to "Since v1.0" is the integrator's at the checkpoint, as for the other pieces.

## Suite counts (on the final tip)

| Suite | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **2,530 pass**, 0 fail, 11 ignored (main at `38ea11d`: 2,400) |
| `cargo test -p undra-cli --lib` | 268 (was 255) |
| `UNDRA_REQUIRE_TOOLCHAINS=1 cargo test -p undra-cli --test build_systems --test upgrade --test ci_workflow --test doctor` | 6 + 15 + 8 + 5 = **34 pass, none skipped** (build_systems 96 s: Gradle debug/release/debug and Cargo.lock, Xcode Debug/Release/Debug with the relink and the linked size, `npm run build`, the three "no undra" paths; upgrade end to end against a local clone) |
| TypeScript runtime `npm test` / `typecheck` / `build` | **1,128 pass** (26 of them the Vite plugin, 24 before) / clean / clean; `npm pack --dry-run` has `dist/vite.{js,d.ts}`, `exports["./vite"]`, no dependencies |
| Kotlin runtime `test-local.sh` (brew 2.4.20 and CI's 2.0.21) | 612 cases, 0 failed, 2 skipped (no native library), under both |
| Swift runtime `swift test` | 480 pass |
| `bash contract-tests/run-all.sh` | **54/54** (18 × 3) |
| `undra bindgen -C examples/playground --check --docs` | up to date (`0xddcdea47fa95a8d4`) |
| `node site/scripts/build-all.mjs` (twice) + `check-links.mjs --words` | no diff the second time; links clean; landing 342/350 words |
| `Cargo.lock` | identical to `main`'s |

## Open items

1. The CI steps added to `rust`, `macos` and `android` are unproven on GitHub runners (the images' Xcode, simulator runtimes,
   JDK, platform 35 and NDK were not observed from here). The first CI run after the merge is the proof; if the `android` job's
   image lacks something the Gradle test says so and passes (by design), so read its log once.
2. `undra upgrade` does not read Gradle version catalogs (`gradle/libs.versions.toml`), a one-line (minified) `package.json`, or a
   renamed Cargo dependency (`my-undra = { package = "undra", … }`): such a pin is neither moved nor reported. `init` writes none of
   these; a project that adopts one is told nothing.
3. **The `0.1.0` key of the "Since v1.0" notes.** The workspace is `0.1.0`, so `init` pins `v0.1.0`, a tag that does not exist;
   `scripts/bump-version.sh 1.0.0` will make the first release `1.0.0`. A project made by a `0.1.0` CLI crosses no entry on its way
   to `1.0.0` unless the entry is re-keyed `1.0.0` in the release pull request (RELEASING now says so). The test no longer pins the
   key, so the bump will not turn CI red; re-keying is a decision for whoever cuts 1.0.0.
4. `scripts/bump-version.sh` does not set `runtimes/rn/@undra/react-native/package.json`'s version (React Native piece). `upgrade`
   now writes `^<line>.0` for `@undra/react-native`, which resolves only if that package is released in lockstep.
5. `undra doctor` has no React Native scope (CocoaPods, which `docs/REACT_NATIVE.md` and `scripts/rn-device-checks.sh ios` need).
6. Third-party actions in the generated workflow (`dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`,
   `maxim-lobanov/setup-xcode@v1`, `gradle/actions/setup-gradle@v4`) are pinned by tag, not by commit SHA (GitHub's hardening
   guidance; a project can let Dependabot keep SHAs fresh). `macos-14` will be retired by GitHub at some point.
7. From the record, still true: Android Studio and Xcode GUI builds, a device iOS build (signing) and Windows were not exercised;
   only `gradlew` and `xcodebuild` were.
