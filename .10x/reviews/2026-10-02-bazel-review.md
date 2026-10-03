# bazel (ADR-061, Bazel rules that run the undra CLI; lint exclusions for generated code) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/bazel` at
`1a09b85` (draft PR #7; main `267b62c` merged in as `bb1184c`) · **Read:** `CLAUDE.md` (R1, R3, R7, R10), `docs/AGENT_WORKFLOW.md` 4,
ADR-061 and its implementation note, `.10x/decisions/sde/bazel.md`, SPEC 13, all of `bazel/` and `examples/bazel/`, the CLI diff
(`--library`, `lint.rs`, the Kotlin header), the CI jobs, the docs page. **Method:** every claim re-run on this Mac (Apple silicon,
Bazel 8.8.1, rustc 1.99.0, binaryen 133): the web core built by Bazel, by `undra build` in two directories, by `undra build` at the
action's own path, and by `scripts/wasm-size.sh`; the bindings diffed against `undra bindgen`'s; the sandbox's execution root probed
with a throwaway `genrule`; the core built with `--spawn_strategy=local`; the Swift package analysed for a Linux target platform;
`bazel query 'buildfiles(..)'` for what loads the telemetry repository; a consumer package written as a Bazel user would write it;
the user mistakes (wrong glob, wrong namespace, web build as `core`, a file outside the workspace) run for their messages; Android
built with the SDK and NDK r27 on the machine; kotlinc 2.4.20 run on `@file:Suppress` variants; a second clone built cold with a
fresh output base and the sandbox's network off. **Fixes:** `699a5a3`, `cd3ae41`, `ef372cf`, `7b4e4b6`, `22dacb7`, `01a2ec0`, and
the docs commit after them.

## Verdict

**Merge after fixes; the fixes are on the branch.** The decision is right and now measured: the action builds what the CLI builds.
Run by hand at the action's directory, `undra build` (rustup's 1.99.0, Homebrew's binaryen 133, the normal Cargo registry) gave the
Bazel module byte for byte (sha256 `c47ec84c..`, 274,053 bytes, 117,958 gzipped), and the Bazel bindings are `undra bindgen`'s file
for file (schema hash `0xce5731fc471dc5b8`; `bindgen --check` passes on that tree). What was wrong was around the CLI call: what an
action reads, where it builds, and what the docs said about both. In a sandbox every input is a symlink, so the "digest of the
inputs" that named the build directory hashed nothing; without a sandbox the action copied the whole source tree and failed on the
example's own `.cargo/config.toml`. The first Linux run failed in analysis (the Swift macro dropped `target_compatible_with`), every
consumer of `defs.bzl` loaded Aspect's telemetry repository, and the documented way to point a JVM test at the core did not work.
All fixed with a run or a test that failed first. Nothing blocking is open.

## Findings

| # | Sev | Where (at `1a09b85`) | Finding | Status |
|---|---|---|---|---|
| H1 | High | `run.sh` `copy_tree "$APP_ROOT"`, `digest_tree` | **An action read its execution root, not its inputs.** With `--spawn_strategy=local` (common on macOS) the root is the source tree: `bazel build //:core_host --spawn_strategy=local` failed reading the example's undeclared `.cargo/config.toml` (`patch location .../crates/undra does not contain packages`). In the sandbox the copy took every input, toolchains included (about 1.2 GB of Rust for a web build), into the app's copy; and since sandbox inputs are symlinks (probed: `find . -type f` lists nothing), the "digest of the inputs" was a constant per platform. | **Fixed** (`699a5a3`): each rule writes a manifest of its declared files; the action copies exactly those (one `tar` for sources). `--spawn_strategy=local` builds. Web action 24.5 s → 17 to 21 s. Test `//tests:actions_tests` (`below`). |
| H2 | High | `run.sh` (`"$(abs "$CLI")" "$@"` from the execution root) | **Cargo read configuration from the output base up.** The CLI ran with the execution root as working directory, and Cargo walks up from there: H1's failure is that walk; on Linux the output base is below `$HOME`, so `~/.cargo/config.toml` (rustflags, a wrapper) would enter the build. | **Fixed** (`699a5a3`): `undra build`/`bindgen` run from the staged project. Linux not run locally. |
| H3 | High | `swift.bzl` (`compatible = kwargs.pop(..)`, never used) | **The first Linux run failed** in analysis: `//swift:hello` reached rules_swift's toolchain (`Swift requires the configured CC toolchain use clang`); the "skipped on Linux" claim had no run. | **Fixed** (`cd3ae41`): every target of the macro and the runtime's Swift targets are Apple-only; `bazel build --nobuild //swift:all --platforms=<linux>` failed before, passes after. |
| H4 | High | `defs.bzl` loads `ts.bzl` | **Telemetry was on for every user of the rules**, not only TypeScript ones: `buildfiles(//kotlin:all)` listed `@aspect_tools_telemetry_report`, whose repository rule POSTs to Aspect unless `DO_NOT_TRACK`/`ASPECT_TOOLS_TELEMETRY` is in the repository environment. A module cannot set that for the repository using it (the extension has no tag; read it in `extension.bzl`). | **Fixed** (`ef372cf`): `undra_ts_library` is in `ts.bzl`; the Kotlin and Swift packages no longer load it (query: 0 entries). The docs' set-up section gives TypeScript users the `.bazelrc` line. |
| M1 | Medium | `run.sh` `/tmp/undra-bazel` | **The shared stage parent broke a second user and was tamperable.** It is created `0755` by its first user; another user's `mkdir` of a lock fails, the loop sees no owner and waits 30 minutes, then fails. The owner of that directory can rewrite another user's build. Under the output base it cannot live (that path is per checkout and per user, which is what must not reach the symbols). | **Fixed** (`699a5a3`): `/tmp/undra-bazel-<hash of mode, label, platform>`, the directory itself the lock (`mkdir -m 700` in sticky `/tmp`); a dead build's directory is taken over; another user's sends this build to a `-u<uid>` directory. |
| M2 | Medium | `kotlin.bzl` doc, `NativeLibrary.kt` | **The documented JVM wiring failed.** A test written from the macro's doc, `jvm_flags = ["-Dundra.native.hello_core.path=$(rootpath //:core_host)"]`, died with `Expecting an absolute path of the library` (the example's own test hid it behind a second property). | **Fixed** (`7b4e4b6`): the runtime takes the path from the working directory; `//consumer:summary_test` failed before, passes; `NativeShapeTests` checks the relative path is what is tried; runtime suite 0 failed. |
| M3 | Medium | README, `BUILD.bazel` comment, docs Platforms | **Android was described as one flag away.** `bazel build //:mobile_android --config=android` stops at analysis (`Unable to find a CC toolchain ... android_arm64`) on a Mac with NDK r27 and `cargo-ndk`; the README said the guide explains how to add it (it does not). `undra_android_library` had never run. | **Fixed** (`22dacb7`, docs): ran it, it builds with the SDK alone (`//android:hello --config=android`, manual); every text now says the core does not build without a C++ toolchain for Android platforms. Not in CI: `rules_android` then downloads its tools, one from googlesource without a checksum. |
| M4 | Medium | ADR-061 4 and Consequences, docs "Why it is hermetic" | **The reproducibility claims did not hold as written**: "a digest of its inputs" (see H1), "the same bytes on every machine" (one machine measured), and the cause given was half of it (below). | **Fixed** (docs, ADR amendment): named by the target; two clones on one Mac measured; the causes listed. |
| M5 | Medium | `ci.yml` `bazel-example-macos` | **The macOS job re-ran the Linux job's targets** (JDK, Kotlin, Node, ktlint) on the scarcest runner; this PR's run waited 37 minutes for a macOS slot. | **Fixed** (`22dacb7`): it runs the Swift test and the iOS core only. Both jobs stay in "All green" and run on every change (decision below). |
| L1 | Low | `bindings.bzl`, `core.bzl`, `run.sh` | Errors pointed at the CLI, not the target: `core = ":core_web"` gave a `dlopen` failure advising `undra build --platform host`; a glob that missed the core gave C0005 advising to edit undra.toml; a file outside the workspace root was silently not copied. | **Fixed** (`699a5a3`): the first and last fail at analysis naming the attribute; a failed build adds which files it could see and that a missing one belongs in `srcs`. Namespace mismatch and `--library` errors were already readable (run). |
| L2 | Low | `.gitignore` `bazel-*` | Matched every `bazel-*` path in the repository (`site/docs/bazel-x.html`). | **Fixed** (`22dacb7`): the two workspaces only. |
| L3 | Low | `lint.rs`, `kotlin.rs`, docs | "ktlint (every version), detekt and the IDE" was not run. The brief's worry that `@Suppress("ALL")` hides compiler warnings is unfounded: kotlinc 2.4.20 still reports a deprecation under it (only `"warnings"` silences), so the runtime's `-Werror` build of the golden bindings still sees generated code. | **Fixed** (`01a2ec0`): wording; trees and the CLI golden regenerated. The annotation stays beside the `.editorconfig`: it travels with a file that leaves its tree (the srcjar) and is what detekt reads. |
| L4 | Low | CLI, web action | Without `node` on the action's PATH, `undra build --platform web` builds a debug host library just to read the schema hash for the symbol manifest (about 5 s per web build). | Open: follow-up (give the action Node from the toolchain). |
| L5 | Low | `run.sh` | The CLI's "Built:" table prints on every successful action. | Open, by choice: it carries the size and the wasm-opt warning. |

## Reproducibility (the brief's item 1)

| Build | Bytes | gzip -9 | sha256 |
|---|---|---|---|
| Bazel, stage at `1a09b85`'s path | 274,053 | 117,958 | `c47ec84c..` |
| `undra build` by hand at that same path | 274,053 | 117,958 | `c47ec84c..` (identical) |
| `undra build`, directory a | 274,598 | 118,223 | `83d08990..` |
| `undra build`, directory b | 274,598 | 118,226 | `33767fde..` |
| `scripts/wasm-size.sh` (same template) | 274,592 | 118,234 | `19f9f66d..` (ceiling 120,000) |
| Bazel after the fixes, this worktree | 274,559 | 118,209 | `6e1c9ca6..` |
| Bazel after the fixes, a fresh clone, fresh output base, network off | 274,559 | 118,209 | `6e1c9ca6..` (identical; host library `1f6d743a..` and the bindings too) |

Why `undra build` is not location-independent (recorded, not fixed: not small): the shim crate is named `undra_core_<fnv1a32 of the
project path>` (`shim.rs` 55-62), and Cargo's disambiguator of a path package outside the workspace it builds hashes its absolute
path (in a and b the core is `Csb02I6R6j2gI_10hello_core` and `Cs7uezLRXDYTF_10hello_core`). LLVM's identical-code folding and
`wasm-opt` pick representatives and order by name, so the function set moves (919 against 927). Path strings are not the cause:
both modules carry the same remapped ones (`/undra/src/crates/..`, `/undra/deps/..`). Fix: name the shim by the project's id, and
build the core as a member of the shim's workspace (or from the registry, once published).

The bindings action reruns when the core changes (a doc comment changed: core_host, then UndraBindgen, re-ran), and every file
`undra bindgen` writes into a language's directory (`.gitattributes`, the lint files) is inside a declared tree artifact, which Bazel
clears before the action: nothing stale survives.

## CI shape (item 5)

Both Bazel jobs run on every change and are in `all-green.needs`. A path filter would save little: the results move with
`crates/**` (every core is built from them), `runtimes/**`, the CLI and the rules, which is nearly every change, and a job that can
be skipped needs a skip-aware roll-up, which "All green" is not. The macOS job is cut to what Linux cannot run. The repository cache
is keyed by the two lock files and `Cargo.lock`; a cold local run (fresh clone and output base, repository cache warm) is 81 s for
all six tests on an M-series Mac. A Bazel disk cache is a follow-up if the Linux job's cold time on CI grows.

## Not verified

The Linux job's full run and both jobs' durations on this head (the PR's checks after the push are the first), the macOS job on
`macos-15`'s Xcode, H2 on Linux, detekt/IntelliJ/SwiftLint/ESLint reading their files (not installed; nothing downloaded), the
Android core under Bazel (no C++ toolchain for Android platforms exists in the example), and two users on one Mac (the fallback
path is read, not run). Building `//android:hello` fetched `rules_android`'s own repositories (its extensions in the pinned graph:
`android_tools`, a Go SDK, Maven artifacts, `com_android_dex`), recorded in the example's `MODULE.bazel.lock`.
