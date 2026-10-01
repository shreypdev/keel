# The C ABI as a per-core function table (ADR-044) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** adversarial reviewer (`docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/abi-table` at `5c621ef` (`main` `6c71cca` merged) · **Read:** `CLAUDE.md` (R1, R2, R6, R7, R8, R9, R11), ADR-044,
ADR-022, ADR-026, ADR-029, ADR-035, ADR-038, `docs/SPEC.md` 6, 6.1, 11, 13, `.10x/decisions/sde/abi-table.md`, and
`git diff main...HEAD`: all of `crates/undra-ffi` (`table.rs`, `native.rs`, `jni_shim.rs`, `api.rs`, `session.rs`,
`registry.rs`, `guard.rs`, `buf.rs`, the tests and harnesses), the CLI's `schema.rs`, `config.rs`, `session.rs`,
`shim.rs`, `builds/{ios,android,host,rn}.rs` and the shim template, `undra-bindgen`'s `naming.rs` and the three
generators' entry, the runtimes' table readers (Swift `CoreTable.swift`, `UndraCoreEntry.swift`, `InprocTransport`'s
claim; Kotlin `NativeApi`, `NativeLibrary`, `CoreEntry`, `InprocTransport`; TypeScript `UndraCore.unloaded`; React
Native `UndraApi*.cpp`, `UndraHost`), `undra.h`, `examples/two-cores` and `two-cores.yml` · **Fixes:** `e960303`,
`a4ed73e`, `655c434`, `9ba4174`, `0213b8f`; `main` merged at `97232f1` (derived-lists), bindings regenerated at
`f07515b`, the merged contract columns fixed at `939e1a1`; `main` `6db6749` merged again at the end (`07405d7`), the two-core
packages regenerated for its derived-list doc comment (`40dd40a`).

## Verdict

**Sound; merge.** The table does what ADR-044 says, and the property that matters most is now proved rather than
argued: two cores in one process share nothing. A new C harness (`tests/c/two_cores.c`) opens two copies of
the fixture library as two images, sends a panic through each table (status 2 on the synchronous and the callback
path), then shuts core A down while core B has a stream open and a port call in flight: A's calls in flight are
answered as cancelled before its shutdown returns, A's callbacks never run again (the test aborts if one does), A's late
`port_reply` goes nowhere, A's `stats_json` reports no runtime and `runtime_threads: 0` while B's threads run, and B
answers its port call, streams on credit, logs to its own host and keeps computing; A then starts again beside B. A
negative control (one image opened twice) fails at B's `init`, so the test would see sharing. Every piece of
process-global state in `undra-ffi` and `undra-runtime` (the embedder slot, `PORTS`, the generation counter, the write
checker's thread-local runtime list, the thread counter behind `runtime_threads`, timers, the blocking pool) is a
static of the image, so "per process" became "per core" for all of them by construction; the test is the proof. On the
iOS simulator the two-core app now also panics inside each **prelinked** core and gets a reply back, in Debug
(`-all_load`) and Release (`-u`, fat LTO) — unwinding survives `ld -r`. The linked app binaries, Debug and Release, of
the two-core app and of a fresh `undra init` app export exactly one `_<ns>_undra_api` per core and no Rust symbol; the
Android libraries export `<ns>_undra_api`, `JNI_OnLoad` and `JNI_OnUnload` only; the wasm exports are unchanged.

Nothing High. Two Mediums, both fixed with a test that fails without the fix: a namespace whose generated entry is a name the
runtimes already declare (`core` → `UndraCore`, `ids`, `store`, `log`, `core_native`, ...) was accepted and produced
bindings that do not compile (M1); and the sibling-namespace refusal also refused **another checkout of the same
project** — a `git worktree` or a copy beside it — so `undra build` failed in both (M2; the refusal had no test at all).
Four Lows, fixed: the CLI's schema loader could dereference a null namespace or hold a null function pointer of a
damaged library as a Rust `fn` (L1); `export_core!`'s compile-time check accepted uppercase and a leading `_` against
the lowercase rule (L2); an overstated `// SAFETY:` comment (L3); the Node two-core runner ran a stale wasm against
newer bindings (L4). ADR-044 is flipped to Accepted with the implementer's deviations and the integrator's
Amendment A (per-namespace default storage, follow-up `ns-storage`).

## Findings

### M1 — a namespace whose entry is a runtime name generated uncompilable bindings (fixed, `a4ed73e`)

`crates/undra-cli/src/config.rs:602` (`check_namespace`) checked only the characters. The entry `Undra<Pascal(ns)>` is
declared in the same scope as the runtime's types and the generated `UndraIds`/`UndraCoreNative`, so `namespace =
"core"` generated `public enum UndraCore` beside `import UndraRuntime` (the enum shadows the runtime's type and
`load() -> UndraCore` no longer type-checks), `export const UndraCore` beside `import { UndraCore }` (TS2440), and
`object UndraCore` beside the import in Kotlin; `ids`, `core_native`, `core_entry`, `store`, `log`, `object`, `runtime`,
`app` (the Android template's `UndraApp`) likewise. The user met it as a compiler error inside generated code, not as an
`undra` error (R8). Fix: `undra_bindgen::naming::RESERVED_ENTRIES` (every `Undra…` name the Swift, Kotlin, TypeScript
and React Native runtimes, the generated bindings and the app templates declare) and `CoreNames::entry_is_reserved`;
`check_namespace` refuses such a namespace with the entry it would have produced (C0002, for a configured and for a
derived namespace). A test scans the runtimes' and templates' sources for `Undra…` declarations and fails when one is
missing from the list (mutation-checked by deleting `UndraWriter`), so the list cannot drift as the runtimes grow.

### M2 — the sibling refusal blocked a second checkout of the same project (fixed, `9ba4174`)

`crates/undra-cli/src/session.rs:200` (`sibling_with_namespace`) refused any directory next to the project with an
`undra.toml` claiming the same namespace. A `git worktree add ../app-feature` or `cp -r app app-old` of a project whose
`undra.toml` is at its root (what `undra init` creates) is exactly that, so `undra build` refused in **both** checkouts.
A sibling with this project's own `[project] id` is another checkout of the same app, not a second core of it, and is
now skipped. The refusal had no test; two now pin it (a set and a derived namespace are found; worktrees and copies are
not; the second fails without the fix). SPEC 13 states both rules. What the scan reads, for the security question: the
parent's listing, each sibling's `undra.toml`, and one `Cargo.toml` under the sibling's own `core.path` (which may be
absolute or `..`, so a crafted `undra.toml` can make it read one file named `Cargo.toml` anywhere). Only its
`[package] name` is compared and nothing of it is printed beyond naming the sibling directory in a refusal: accepted.

### L1 — the schema loader trusted a damaged table (fixed, `655c434`)

`crates/undra-cli/src/schema.rs` read the table through a `#[repr(C)]` struct whose `schema_json` and `buf_free` were
non-nullable `fn` pointers and passed `name_space` to `CStr::from_ptr` unchecked: a library named on the command line
with a null namespace or entry was undefined behaviour rather than C0006. The reading moved to `checked_table` (null
table, `abi_version` first and nothing else of another version, `size >= sizeof`, namespace not null and equal, the two
entries `Option`s and present), with a unit test over in-memory tables of each defect. Swift (`CoreTable.swift`) and
React Native (`copyTable`) already checked every entry and the namespace.

### L2 — `export_core!` accepted namespaces `undra.toml` refuses (fixed, `a4ed73e`)

`crates/undra-ffi/src/table.rs:149` (`is_valid_namespace`) accepted `A1_b2` and `_x` (its own test listed them as
good), while the record and the ADR deviation say lowercase only. A hand-written shim could export a core no generated
binding can name. It now enforces `[a-z][a-z0-9_]*`, at most 32, and the compile-time message says so; keywords (`fn`,
`self`, `gen`, `type`, `match`) are verified to work as namespaces (`$ns:ident` matches them).

### L3 — `unsafe impl Sync for UndraApi` claimed more than it guarantees (fixed, `655c434`)

`table.rs:99`: `UndraApi`'s fields are public, so a table built by hand may hold any `name_space`; the comment said it
always points at a `'static` string. The impls are sound regardless (no safe code dereferences the pointer); the comment
now says that.

### L4 — the Node two-core runner skipped the build when a wasm existed (fixed, `0213b8f`)

`examples/two-cores/node/run.sh` built a core only when its `.wasm` was missing, so after a core change (this review's
merge of `main` moved the schema hash) it ran the old module against the regenerated bindings and failed with a schema
mismatch. CI always starts clean, so only local runs were affected. It now runs `undra build` every time (incremental).

## The attack, per surface

**1. R2 and the table's soundness.** Every `unsafe` block and `unsafe impl` in the diff has a `// SAFETY:` comment; read
each against the code: `table.rs` (Send/Sync, L3), the `export_core!`/`__export_jni!` expansions (`#[unsafe(export_name)]`,
`#[unsafe(no_mangle)] JNI_OnLoad`, whose body's `unsafe` call is the one the JVM contract makes sound; both functions are
declared inside `const _: () = { .. }` so no safe Rust code can name and call them with a bogus `JavaVM *`),
`jni_shim::on_load`, `native.rs` and `registry.rs` (moved, unchanged), the CLI loader (L1). The macro's tokens are
authored in `undra-ffi`; a shim crate with `#![forbid(unsafe_code)]` compiles (checked), so the generated shim holds no
`unsafe` of its own (R2). **Version and size:** every host reads `abi_version` first and refuses any other value
(CLI, Swift, React Native; Kotlin through `abiVersion()`), then requires `size >= sizeof(UndraApi)` as it knows it — so
`abi_version` is the breaking version and appended fields under version 2 are accepted by older hosts (tested in the
CLI loader with a 64-byte-larger table). A v1 library: the CLI names it as v1 (`undra_abi_version` resolves); Swift's
generated entry fails at link time; React Native's `dlsym`/class lookup fails with a message; Kotlin's v1 `JNI_OnLoad`
registers on a class that no longer exists and returns `JNI_ERR`, which `NativeLibrary.load` turns into
`isAvailable == false` and a clear load error. **`name_space`:** a `&'static CStr` of the image (static string data);
Swift repairs non-UTF-8 (`String(cString:)`) and compares, React Native compares bytes, the CLI compares lossily (a
mismatch is a refusal); an unterminated string is outside the contract (a library that lies can run arbitrary code at
load anyway). **R6:** all 17 entries are `extern "C"`; each body runs under `guarded` (directly or in `api.rs`), the JNI
natives likewise; a panic through the table is a status 2 reply in the C harness (both images, both paths), the Swift
harness, the JNI harness, contract S17 on three columns, and now the iOS two-core app on prelinked objects.
**Two cores:** enumerated above; each is an image static (proved by `two_cores.c`). JNI: each library's `JNI_OnLoad`
registers on its own class through the class loader of the runtime's `NativeLibrary` (documented); two cores with one
Kotlin package cannot coexist in one class loader anyway (a duplicate `UndraCoreNative` fails the app's dex/compile);
a failed `RegisterNatives` clears the exception and returns `JNI_ERR` (→ `UnsatisfiedLinkError` → "could not be
loaded"); the `JavaVM` is taken per `init` from the calling env; worker threads attach as daemons and the `jni` crate
detaches them from a thread-local guard when they exit (each image has its own).

**2. Dead-strip and prelink (ADR-029).** Real `xcodebuild`s on the iPhone 17 Pro simulator: the two-core app, Debug
and Release (both `two-cores ios: passed`, now including the panic check), and a fresh `undra init` app, Debug and
Release (builds, launches, the to-do screen renders). `nm -gU` of each linked binary: the two-core app exports exactly
`_playground_a_undra_api` and `_playground_b_undra_api` (plus `_main`, the Mach-O header and Xcode's debug-dylib
stubs), the init app exactly `_fresh_app_core_undra_api`; no `__R…`/`_ZN`/`rust_*` symbol is global in any of them;
each core's `undra_ffi` monomorphs are present twice as locals (two images in one binary). Android (`nm -D` of every
`lib<ns>.so`, both ABIs): `JNI_OnLoad`, `JNI_OnUnload`, `<ns>_undra_api`, nothing else. Host dylib: the same three.
wasm: the unchanged `undra_*` exports of SPEC 7 (`schema_retention` covers the host library and passes).

**3. Namespace validation.** Fuzzed (`config.rs` test): empty, 33 bytes, `-`, `.`, `/`, `../x`, uppercase, leading
`_` or digit, non-ASCII lowercase (`été`), space, NUL — all refused; keywords accepted and compile. M1 (runtime names),
M2 (sibling), L2 (the macro's rule). Remaining, Low: two namespaces can share a **bundle** name (`acme_pay` and
`acme_pay_core` both give `AcmePayCore`, so the XCFramework, the pod and the `AcmePayCoreFFI` module collide in one
app); React Native's `validNamespace` accepts a superset (uppercase, leading `_`), which is safe because only
`[A-Za-z0-9_]` ever reaches a class name or a library path.

**4. Generated code and hashes.** The Swift (`Core.swift`, `public enum Undra<Ns>` over `UndraCoreEntry`), Kotlin
(`Core.kt`, `internal object UndraCoreNative` with `override external fun`s, `object Undra<Ns>`) and TypeScript
(`core.ts`) entries read as native code, as do the defaults (`ctx: UndraCore = UndraPlaygroundCore.core`) and
`UndraIds.namespace`; no Rust-ism found in the goldens or the playground's packages. `UndraCore.unloaded` (TS) is the
closed placeholder, documented, every call rejecting with `UndraCallError.Unavailable` (tested). Hashes: before the
merge the playground and both two-core packages were `0xefd907be3070520a` (the record's, unchanged by the piece); after
merging `main` (derived-lists changes the playground core) all three are `0xc5f05c376fde398c`, regenerated and
`undra bindgen --check --docs` clean; `schema_docs --ignored` passes.

**5. Performance (R9).** `boundary/call_sync/add`: main's bench binary (`6c71cca`, built from an
archive of `main`) and this branch's (table, pre-merge), alternating within each pair, 1 s warm-up and 4 s measurement
each. **No quiet moment came** in the review window: other agents' Gradle and rustc builds kept the load average
between 23 and 74 for over an hour, so the brief's "load < 6" could not be met. Ten pairs were taken anyway with the
load at 50 → 23: main 59.5–125.1 ns, table 63.2–92.0 ns, per-pair difference (table − main) −42.8, −16.2, −14.2,
−13.8, −7.0, −3.1, +4.5, +5.5, +8.7, +32.5 ns, **median −5.1 ns**. At this load the spread (±40 ns) is far wider
than ADR-044's 2 ns budget, so this run can only say that no regression shows above the noise; it neither confirms
nor refutes the implementer's +0.6 ns (eight pairs from "quiet rounds" whose load the record does not state). The structure says the same:
one extra load of a function pointer from a table already in cache per call. `sync_alloc` and `commit_alloc`
(exact Rust allocation counts per call and per commit) pass unchanged, so the table added no allocation. The JNI
path (instance natives of the generated `object` instead of `@JvmStatic` natives of `UndraNative`) has no host
benchmark; the device bench (`scripts/bench-device.sh --device android`) was not re-run. Both are `RegisterNatives`
-bound natives called with one receiver argument; the change is an `invokeinterface` through `NativeApi` to a
single implementation per core, which a JIT can devirtualise: not measured, stated. **Re-measure the host pairs when
the machine is quiet before the number goes into `bench/RESULTS.md`.**

**6. The matrix**, once, on the merged tree: see "Suites".

## Suites (merged tree)

Every suite below ran once on the merged tree (`main` `76f9364` with derived-lists). The final merge of `main`
`6db6749` brought the state files and one bindgen change (a derived list's generated doc comment, no hash moves):
after it, `cargo test -p undra-bindgen` (139) and `-p undra-cli --lib` (297), `undra bindgen --check --docs` for the
three packages, and the site build and link check were re-run, all clean.

| Suite | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -D warnings`; `cargo clippy -p undra-ffi --target wasm32-unknown-unknown -D warnings`; `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps`; the wasm32 builds of the core crates | clean |
| `cargo test --workspace --no-fail-fast` | 2,718 passed, 0 failed, 14 ignored (doc tests included; the 19 TypeScript-compiling bindgen tests needed `tsc` on `PATH` here and passed when given the runtime's) |
| `write_context` / `write_checker` (release); `sync_alloc` / `commit_alloc` (release) | 9 + 8; 2 + 5 |
| `schema_retention` + `schema_docs` `--ignored`; `typecheck_swift` | pass; 2 |
| Miri: `-p undra-ffi --lib`; the `abi` subset CI runs plus the two table tests | 35; 4 |
| Rust AddressSanitizer (`-Z sanitizer=address`, `--lib --test abi --test host_contract`) | **does not link on this Mac** (Apple `ld`: "initializer pointer has no target" for the `inventory` constructors under ASan); CI's Linux job runs it |
| C harness under clang ASan: `smoke.c`, `lifetime.c`, **`two_cores.c`** (new) | ok, ok, ok |
| Swift over the fixture's table; wasm harness; JNI end to end (Kotlin 2.4.20 and 2.0.21) | 6; 19 + 24; 16 + 16 |
| Swift runtime `swift test` | 527, 0 failures |
| Kotlin runtime `test-local.sh` (Kotlin 2.4.20, and 2.0.21 in its own build directory) | 630 cases, 0 failed, 1 skipped, both |
| TypeScript runtime `npm test` | 1,133 |
| React Native: `npm test`; typecheck; `test:contract`; `cpp/test/run.sh` (`UNDRA_RN_REQUIRE_JSI=1`); `android/test/run.sh` | 65; clean; 18 + S17 skipped; 15 store + 29 + 29 host checks, JSI, TurboModule and the Apple platform compile; 4 |
| `contract-tests/run-all.sh` | **60/60** (S01–S19 and S26 × TypeScript, Kotlin, Swift; the Kotlin column also under 2.0.21) |
| interop (`crates/undra-transport/interop/run.sh`) | pass |
| `undra bindgen --check --docs` (playground, two-cores a and b) | up to date, `0xc5f05c376fde398c` |
| bench budgets (`undra-bench --test budgets --release`) | 6 pass, 1 ignored |
| `scripts/wasm-size.sh` (ADR-052) | hello wasm 102,971 B gzipped (record 102,722, +0.2%, ceiling 107,858); hello runtime JS 24,914 (ceiling 26,000): within the gates |
| site `build-all.mjs`, `check-links.mjs --words` | clean (reference pages and search index rebuilt for the merged bindings; landing prose 342/350 words) |
| `two-cores.yml` | parses (3 jobs: ios 7 steps, android 8, jvm-and-node 9); its JVM and Node steps run in a clean clone of the branch: `passed`, `passed` |
| Two-core apps: iOS simulator Debug and Release (with the new panic check), Android `undra` emulator, JVM, Node | `passed` × 5 |
| Fresh `undra init` app: `xcodebuild` Debug and Release for the simulator; Release launched | builds; one global `_fresh_app_core_undra_api`; to-do screen renders |
| `scripts/rn-device-checks.sh`: iOS simulator; Android (`undra-rn` emulator) | 19/19; 20/20 |

Merging `main` brought two breaks of the merge's own making, fixed in `939e1a1`: the Swift contract runner exported the
derived-list recording from `$PROJECT`, which this branch's runner no longer defines (every Swift scenario reported
MISSING), and the Kotlin column declared `onMain` twice (S18's, made `internal` here for S26, and S19's private
copy; S19 now uses S18's with a timeout parameter). The playground's schema hash moved with derived-lists; all three
packages were regenerated (`f07515b`).

## Open items

* **Amendment A, `ns-storage`:** default `Kv`/`Fs`/`SecureStore` locations per namespace on every platform, no
  legacy path (recorded in ADR-044; not implemented here).
* **JNI forward compatibility of appended entries.** Appending a field to the C table is compatible (hosts accept a
  larger `size`), but appending a JNI native is not by itself: `RegisterNatives` fails when the class lacks a method,
  so a core with an 18th native would fail to load under bindings generated without it. Today bindings and core come
  from one `undra` version; when an entry is appended, register it separately (and tolerate its absence) or bump the
  version.
* **Bundle-name collisions** between namespaces (`acme_pay` / `acme_pay_core`), surface 3; refuse in the sibling check
  or suffix differently.
* **Thread names** are the same in every core (`undra-core`, …): two cores' threads are indistinguishable in a
  debugger or a trace. Cosmetic; a namespace suffix would help ADR-046's tooling.
* From the record, still open: a React Native app with two cores on a device and S26 in the React Native contract
  column; Metro reload sequences after the table; Gradle `:runtime:test` offline.
* **Re-measure `boundary/call_sync/add`** (main vs table, alternating, load < 6) when the machine is quiet; this
  review could not (surface 5).
* Rust ASan was not run on this machine (it does not link on macOS); the Linux CI job covers it.
