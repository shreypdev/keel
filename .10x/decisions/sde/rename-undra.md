# SDE - the rename to Undra (wt/rename, 2026-09-30)

Executes ADR-030. This record is excluded from `scripts/rename-keel-to-undra.sh` on purpose:
it has to name the old spelling ("keel") to say what happened to it.

## What was done

* `scripts/rename-keel-to-undra.sh` (bash 3.2, `set -euo pipefail`, perl): (a) `git mv` of every
  tracked path whose name carries the old name (983 paths: the 11 `keel-*` crate directories, the facade
  `keel` -> `undra`, `runtimes/ts/@keel` -> `@undra`, `KeelRuntime`, the Kotlin `dev/keel/`
  source trees, `keel.toml`, `keel-*` decision files, ...); (b) the ordered content rules over
  tracked text files (1,057 files); (c) a summary. Idempotent (a second run is a no-op), accepts
  files or directories as arguments in either spelling (`site/blog` works and renames the
  `keel-*` post directories inside it).
* The mechanical commit equals the script's output byte for byte: running the script on a
  pristine `git archive` of the merge base and comparing `git ls-tree -r` blob hashes differs
  only in the script file itself. Everything after that commit is a named, reviewable fix.
* The branch is `main` plus one `git merge main` commit (the later `main` commits, all under
  `contract-tests/`, merged cleanly; `scripts/rename-keel-to-undra.sh contract-tests` then
  renamed the one line they brought); `main` fast-forwards to it.

## The rules, and the ones that are not "keel -> undra"

The ordered rules are exactly the brief's (`@keel/`, `dev.keel`, `dev/keel/`, `Java_dev_keel_`,
`KeelRuntime`, then `Keel`/`KEEL`/`keel`). They are all equal to the generic rule for their
input, so the order only documents intent. Four additions, each because the generic rule is
wrong:

1. `https://keel.dev/errors/` -> `https://shreypdev.github.io/undra/docs/errors.html#` (and the
   slashless base without `#`). keel.dev is someone else's site and undra.dev is not ours, so
   no domain is derived from the name. Diagnostics build `<base>#<code>`; `DOCS_BASE` and the
   E0062 failure message were changed from `{DOCS_BASE}/{code}` to `{DOCS_BASE}#{code}`, and the
   three port tests that looked for `errors/E0062` look for `errors.html#E0062`.
2. The README label `[keel.dev docs & site` -> `[Docs & site` (main already made this change;
   same result). Any other bare `keel.dev` -> `https://shreypdev.github.io/undra/` (one fake
   URL in a `undra-macros` port test).
3. The script's own file name is protected in prose.
4. Articles: "a Keel project" -> "an Undra project" (`an` before Undra/undra/UNDRA, also across
   a wrapped line and before code spans). About 150 sentences; without it `undra --help` said
   "A Undra project". camelCase test names (`testDurationIsAKeelCodec` ->
   `...IsAUndraCodec`, five Swift test names) keep "A": cosmetic, left.

## False positives (step 1 of the brief) and how each was decided

A scan of every token containing "keel" as part of a longer word, and of the byte/hex/base64
spellings:

| Token | Count | Decision |
|---|---|---|
| `libkeel` (`libkeel_core.*`, `libkeel_ffi`), `lkeel` (`-lkeel_core`), `nlibkeel` (a `\nlibkeel_core.so` test string) | 112 / 2 / 1 | renamed - real identifiers |
| `keelbuf` (a WAT function in the TS stub core) | 4 | renamed |
| camelCase: `useKeel`, `openKeel`, `isKeelClass`, `forKeelLevel`, `startKeel`, `rememberKeel` | ~100 | renamed |
| natural-language words (keeled, keelson, keelhaul, nautical prose) | 0 | none exist; no sentence used "keel" as the ship part |
| the envelope magic: the four bytes `4B 45 45 4C` (ASCII of the old name) in Rust, TS, Kotlin, Swift, the wire vectors | ~60 sites | **frozen**: a wire change needs an ADR (CLAUDE.md, R7/R11) and `UNDRA` is five bytes anyway. Spelled as bytes/hex everywhere (`MAGIC = [0x4B, 0x45, 0x45, 0x4C]`, SPEC 3.2, the blueprint figure, four error messages now say `4b45454c`); tests that need a wrong magic write the lower-case form as bytes. `0x4B45454C` as a fuzz seed is arbitrary and unchanged. |
| hashes of the name: `fnv1a64("keel")` known-answer vector (JSON source, Swift copy, regenerated Kotlin table, Rust x4, TS, Swift) | 12 files | now hashes `"undra"` = 12206477163874244763 (LE hex `9b58a4cf4e1e66a9`) |
| the Kv key `keel.query.queue` (offline queue) -> `undra.query.queue`; the Swift Kv file-name test pins `fnv1a64-fnv1a32` of the key | 1 test | recomputed: `def1907793b60cec-f2daf06c` |
| `github.com/shreypdev/keel-swift` (separate Swift distribution repo, 5 refs) | 5 | renamed `undra-swift`; the repo does not exist yet (dist piece) |
| `playground.keel.test`, `~/src/keel`, `avdmanager create avd -n keel` | few | renamed; the AVD on this machine is still called `keel` (create `undra` or rename it: `undra doctor`/the README now say `undra`) |
| `claude/keel-framework-takeover-66c4ea` in status.md / handoff.md | 2 | renamed mechanically to a branch name that never existed; both files are the integrator's |
| `.10x/specs/**`, `.10x/decisions/*/launch-v2.md` (the rename's own planning records) | 4 files | **excluded by default** ("the working name Keel" would read "the working name Undra"); naming them as script arguments lifts it |
| `examples/playground/.proof/*.log` (run logs, text) | 3 | rewritten like any text (they record runs of the tool); the PNGs are binary and untouched |

## Fixes a text rename cannot make (the commits after the mechanical one)

* Envelope magic + FNV vector (above); `Cargo.lock` (`cargo build`), three `package-lock.json`
  (`npm install --package-lock-only`; npm 11 also writes the runtime's `peerDependencies` into
  the lock root entry - format drift, not a dependency change).
* `cargo fmt`: 67 files (imports re-sorted, lines re-wrapped for the longer name).
* Fixed-width expectation: the CLI's aligned-table test pads to `libundra_core.so` (+1 space).
* Diagnostics docs link shape (`#`), see rule 1.
* The XCFramework library name (see "Found on the way").

## Goldens

All regenerated through their own mechanisms, never hand-edited, and each compared to the
script's output with a character-multiset check (whitespace, commas and, for trybuild, carets
and `line:col` ignored):

* `UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden`: 40 files; `UPDATE_GOLDEN=1
  cargo test -p undra-cli --test bindgen_schema`: 2 files; `UPDATE_SNAPSHOTS=1 cargo test -p
  undra-macros --lib`: 6 files; `TRYBUILD=overwrite ... --test compile_fail`: 6 stderr files;
  `undra bindgen -C examples/playground --docs`: 10 files.
* Outcome: **the regenerated files are not byte-identical to the script's output in 64 files,
  and the templates need no attention**: the generators sort imports and wrap at a width, and
  `UndraCodec` sorts after `Payloads`/`Timestamp` where `KeelCodec` sorted before (Kotlin
  imports, TS import/export lists), one TS signature wraps, macro token snapshots wrap, trybuild
  carets/columns are one character longer. The multiset check finds no other difference. The
  schema hash is unchanged (`0x0f95cc4a735f44cd` for the playground): no name feeds it.
* The Kotlin `WireVectors.kt` table is regenerated by `gen-vectors.py` (its header carries the
  source's sha256).

## Verified (local, macOS, this tree)

| Suite | Result |
|---|---|
| `cargo fmt --check`, `clippy --workspace --all-targets -D warnings`, `cargo doc` (`-D warnings`) | clean |
| `cargo test --workspace` | 2,110 passed, 0 failed, 9 ignored (the gated heavy tests) |
| wasm32 builds of meta, wire, signals, runtime, ports, query, ffi, undra, playground-core; clippy ffi wasm32 | ok |
| TS runtime (`npm ci && npm test`, typecheck) | 897 passed |
| Kotlin `test-local.sh` (plain / over the real JNI core) | 454 cases, 0 failed (2 skipped / the 2 JNI smoke cases run, 16 `Java_dev_undra_runtime_UndraNative_*` exports) |
| Swift `swift test` | 328 passed; `undra-ffi/tests/swift/run.sh` ok |
| C harness (plain and ASan), wasm acceptance | ok; 29 (19 + 10) |
| `contract-tests/run-all.sh` | 51/51 (ts, kotlin, swift) |
| `cargo test -p undra-bench --test budgets --release` | 2 passed |
| playground web `npm ci && npm test` / `npm run build` | 6 passed / built |
| playground iOS: `undra build --platform ios` + `xcodebuild` for the simulator; installed and launched on the booted simulator | BUILD SUCCEEDED, bundle `dev.undra.playground`, Counter screen renders from the core |
| playground Android: `./gradlew assembleDebug`; installed and launched on the booted emulator | BUILD SUCCESSFUL, `dev.undra.playground`, Todos screen renders from the core |
| gated: `UNDRA_TEST_IOS=1` xcframework test, `UNDRA_TEST_ANDROID=1`, `schema_retention --ignored` (ADR-029) | pass |
| `undra doctor`, `undra --help` | ok |

## Found on the way (not caused by the rename)

1. **iOS `xcodebuild` of the playground was already broken on `main`.** Since 899ca8c the shim
   library is named `undra_core_<hash>` (per project) and the iOS build packaged it as is, while
   the generated Xcode project force-loads `libundra_core.a`: "Build input file cannot be
   found". Reproduced on a pristine archive of the pre-rename tree (`libkeel_core_<hash>.a` in
   the XCFramework). The existing gated test `ios_builds_an_xcframework_with_device_and_simulator_slices`
   asserts the canonical name and would have caught it; nobody runs it by default. Fixed in
   its own commit (`fix(cli): the XCFramework carries its libraries as libundra_core.a`: copy to
   the canonical name like the Android and host builds do); the gated test passes.
2. `runtimes/swift/.../Resources/wire-vectors.json` was already out of date against
   `contract-tests/wire-vectors.json` before the rename (missing `map_key_order_by_encoded_bytes`);
   `runtimes/swift/scripts/sync-vectors.sh --check` fails on `main` too. Left alone (syncing
   adds a vector the Swift test may not know); the rename edited the same copy in lockstep.

## Residual risks and things for the integrator

* **Wire magic still spells the old name** (4B 45 45 4C). Changing it to `UNDR` (55 4E 44 52) is
  a four-byte constant in each runtime, the wire vectors and SPEC 3.2; it needs an ADR and is
  free today. Recommend doing it before publication or never.
* `undra.dev` is not ours; no link points there. Diagnostics point at
  `https://shreypdev.github.io/undra/docs/errors.html#<code>`, a page the site piece must create
  (anchors `E0001` ... `E0064`, `C0001` ...); until then the links dangle on our own host.
* The repository must be renamed (`gh repo rename undra`) for the README badges, `Cargo.toml`
  `repository`, `Package.swift`/`init` templates and the `undra-swift` placeholder to resolve.
* `.10x/status.md` and `.10x/handoff.md` were renamed mechanically only; the takeover branch
  name in them is now fictional.
* In-flight branches (`site-v2`, `blog`, `stress`, `dist`) cross with `docs/AGENT_WORKFLOW.md`
  "Bringing a branch across the rename". Path-list mode was drilled on a scratch repo (modified
  file, new file in a moved directory, new `site/blog/keel-...` directory).
* The user's shell may still export `KEEL_*` variables; nothing reads them any more.
