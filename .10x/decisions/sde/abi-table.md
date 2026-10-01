# SDE — abi-table: one C ABI table per core, cores side by side (ADR-044, wt/abi-table, 2026-10-01)

ADR-044's implementation brief, items 1–8. Before this piece a process could hold one Undra core: the C ABI was
19 global `undra_*` symbols exported by `undra-ffi` itself, the JNI natives were registered on one runtime class,
the iOS library needed `-force_load` and every artefact was called `undra_core`. Now each core exports one
symbol named after its namespace, `<namespace>_undra_api()`, returning its `UndraApi` table (C ABI version 2);
its libraries, its JNI class and its bindings' entry are named after the namespace too, and two cores load into
one app on iOS, Android, the JVM, Node and React Native. **No wire change, no schema change**: this piece moves no
hash (the playground's was `0xddcdea47fa95a8d4` before and after it; `0xefd907be3070520a` once main's playground
`platform` module was merged in, again identical with and without the namespacing), asserted by
`schema_retention.rs`, `abi.rs` and `undra bindgen --check`. **No wasm ABI change** (`WASM_ABI_VERSION` stays 1).

## What landed, per item

1. **`undra-ffi`** (`65bfcd3`). `src/table.rs`: `UndraApi` (`#[repr(C)]`: `abi_version u32 = 2`, `size u32`,
   `schema_hash u64`, `name_space *const c_char`, then 17 entry points in SPEC 6 order), built once per image in a
   `OnceLock`; `export_core!(ns, jni_class = "a/b/UndraCoreNative")` (`macro_rules!`) emits
   `#[unsafe(export_name = "<ns>_undra_api")]` and, with the `jni` feature, `JNI_OnLoad`/`JNI_OnUnload` that register
   the 17 natives on the named class; the namespace is checked at compile time (`__namespace`, a `const fn`: a C
   identifier of at most 32 lowercase characters). The entry points lost `no_mangle` (`pub(crate)`), the crate is an
   rlib only, `undra_abi_version`/`undra_schema_hash` became table data. JNI: no `Java_*` exports; callbacks go to
   `dev.undra.runtime.NativeCallbacks`. Tests: `tests/common/table.rs` (the fixture namespace `undra_test`), every
   ABI test through the table, the layout offsets, the namespace rules, the header's field order against the Rust
   struct; `tests/fixture` is a core like any other (`undra_fixture`), and the C, Swift and JNI harnesses run
   against it. Miri: `--lib` 35 pass (the table tests included) and the `abi` subset CI runs plus the two table
   tests, 4 pass.
2. **`undra.h`** — the table, no functions, `UNDRA_ABI_VERSION 2u`; the host contract text unchanged except that
   `undra_<name>` now names the table's `<name>` entry. The Swift runtime's copy and React Native's are
   byte-identical (`undra_h_copies_are_identical` and the RN host test).
3. **CLI** (`6757777`, `ec3f803`). `[core] namespace` (validated; default: the core's package name in snake case;
   C0002 BadConfig when a sibling project already uses it). The shim calls `export_core!`. Artefacts:
   `build/ios/<Pascal>Core.xcframework` whose slices are each **prelinked** (`xcrun ld -r -arch <a> -platform_version
   ios|ios-simulator <min> <sdk> -exported_symbol _<ns>_undra_api`, `-u _<ns>_undra_api` in release and
   `-all_load` in debug, then `libtool -static`) into `lib<ns>.a` with `Headers/<ns>_undra.h`;
   `jniLibs/<abi>/lib<ns>.so`; `build/web/<ns>.wasm`; `build/host/lib<ns>.{dylib,so}`; for `rn` a per-core
   `<Pascal>Core.podspec` and `<Pascal>CoreTable.m`. `schema.rs` `dlsym`s the table, checks version 2, `size` and
   `name_space`, and names a v1 core as such. Templates and `adopt` load through `Undra<Ns>.load`, link the library by
   path with no `-force_load`; `schema_retention.rs` checks `<ns>_undra_api` and `JNI_OnLoad` only, and that no v1
   symbol is left.
4. **bindgen** (`1cd7b9a`). `CoreNames` (`naming.rs`): entry `Undra<Pascal(ns)>`, bundle `<Pascal>Core` (no
   `CoreCore`), FFI module `<Bundle>FFI`, header `<ns>_undra.h`, JNI class `<package>/UndraCoreNative`. Swift:
   `Core.swift` (`public enum Undra<Ns>` over `UndraCoreEntry`) and a C target `<Bundle>FFI` in the generated
   package; Kotlin: `Core.kt` (`internal object UndraCoreNative : NativeApi` with 17 `override external fun`s, and
   `object Undra<Ns>` over `CoreEntry`) plus `META-INF/proguard/undra-<ns>.pro`; TypeScript: `core.ts`
   (`export const Undra<Ns> = { namespace, schemaHash, async load, async attach, get core() }`). Every generated
   default is `Undra<Ns>.core`; `UndraIds.namespace` in all three. Goldens regenerated and read as native; the
   playground's and both two-core packages' bindings are committed and `--check` clean.
5. **Runtimes.** Swift (`1480140`): `CoreTable` reads `abi_version` first, then size, namespace and the 17 entries;
   `LoadOptions.inproc(api:)`; per-namespace claim; `UndraCoreEntry`. Kotlin (`ffe0405`): `NativeApi` +
   `NativeCallbacks` public, `UndraNative` gone, `CoreEntry`, `NativeLibrary.load(namespace)`, per-namespace claim,
   `UndraCore.load(options)` refuses in-process with a pointer to the entry. TypeScript: the generated entry plus
   `UndraCore.unloaded` (the placeholder entries return). React Native (`101a829`, `5da37e3`): the C++ module finds a
   core by namespace (iOS: the class `UndraCoreTable_<ns>` its pod compiles; Android: `dlopen`/`dlsym`), one host and
   one `__undraNative[ns]` per namespace; `loadNative(entry, options)`.
6. **Two cores.** `examples/two-cores/{a,b}` build the playground core as `playground_a` and `playground_b`; a test
   app per platform (`ios/`, `android/`, `jvm/`, `node/`, each with `run.sh`) loads both, makes a call and an
   observed change on each, compares statistics, shuts one down and checks the other keeps answering;
   `.github/workflows/two-cores.yml` runs the four (iOS Debug and Release). Contract scenario **S26 "two cores"**
   (`contract-tests/scenarios.md`) in the Swift, Kotlin and TypeScript columns.
7. **Bench.** `boundary/call_sync/add` goes through `(api.call_sync)` / `(api.buf_free)`. Numbers below.
8. **Docs.** SPEC 6, 6.1, 6.2, 7, 10, 11.2, 13, 17 (`b251033`); ONBOARDING (suites, the fixture harnesses, the
   gotcha); the site: a guide "One core or several" (`site/docs/several-cores.html`), every docs page loads through
   the entry and names artefacts after the namespace, the architecture page shows the table, the reference pages
   gain the entry as a section; `docs/REACT_NATIVE.md`, the Swift and Kotlin runtime READMEs, `crates/undra-ffi/README.md`.

## Measurements

* `boundary/call_sync/add`, host (M-series, 18 cores, shared with five agents), main's bench binary and this
  branch's run alternately, 4 s each: in the quiet rounds the per-pair difference has a median of **+0.6 ns**
  (pairs: −0.1, +5.4, +0.5, +1.1, +2.7, −0.7, −0.5, +0.7; main 47.7–55.5 ns, table 48.8–54.8 ns). Within ADR-044's
  2 ns. Four rounds taken while the load average went to 18 were discarded (both sides 55–150 ns).
* Release iOS core prelinked: `libplayground_a.a` 3.05 MB per slice; the two-core Release app binary 6.2 MB. Android
  `lib<ns>.so` about 1.6 MB per ABI for the playground core.

## Verification on the final tree (main `0aa98a4` merged)

| Suite | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -D warnings`; `cargo clippy -p undra-ffi --target wasm32-unknown-unknown -D warnings`; `cargo doc --no-deps -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` | 2,620 passed, 0 failed, 13 ignored (doc tests included) |
| Miri: `-p undra-ffi --lib`; the `abi` subset CI runs plus the table tests | 35 pass; 4 pass |
| C harness (plain and ASan), wasm harness, Swift over the fixture's table, JNI end-to-end (Kotlin 2.4.20 and 2.0.21) | ok; 19 + 24; 6; 16 + 16 |
| Swift runtime `swift test` | 497, 0 failures |
| Kotlin runtime `test-local.sh`, Kotlin 2.4.20 and 2.0.21 (own build directory) | 629 cases, 0 failed, 1 skipped, both |
| TypeScript runtime `npm test` | 1,133 |
| React Native: `npm test`, typecheck, `test:contract`; `cpp/test/run.sh` | 65; clean; 17 + S17 skipped; 15 store + 29 + 29 host checks, JSI, TurboModule and the Apple platform compile |
| `contract-tests/run-all.sh` | 57/57 (S01–S18 and S26 × TS, Kotlin, Swift) |
| interop, `bindgen --check --docs` (playground, two-cores a and b), `schema_docs --ignored`, `schema_retention --include-ignored`, `sync_alloc`/`commit_alloc`, `undra-bench` budgets | pass |
| site: `build-all.mjs`, `check-links.mjs --words` | clean |

Devices (screenshots and logs in the session scratchpad): two-core app on the iPhone 17 Pro simulator, Debug and
Release (`ios-two-cores-debug.log`, `ios-two-cores.log`, `ios-two-cores.png`), on the `undra` emulator
(`android-two-cores.log`, `.png`), JVM and Node (`jvm-two-cores.log`, `node-two-cores.log`): every line `ok`, then
`passed`. The playground on iOS (four screens alive, no fault lines, the XCUITest tour 6 tests / 1 skipped / 0
failed; `ios-playground-*.png`) and Android (four tabs alive; `android-playground-*.png`), its web build. A fresh
`undra init` app in process and on `undra dev` with the dev bar, on iOS (`ios-init-app.png`, `ios-init-app-dev.png`),
Android (`android-init-app.png`, `android-init-app-dev.png`) and the web (`web-init-app.jpg`,
`web-init-app-dev.jpg`). `scripts/rn-device-checks.sh`: iOS 19/19, Android 20/20.

## Merging main (dev-reload, rn-adapters)

* rn-adapters' default ports sit on top of the per-namespace host unchanged: the native defaults are created per
  host (so per core) and registered through the table's `port_register`. `nativePlatformDefaults()` became
  `nativePlatformDefaults(namespace)` (the module object is per namespace now; the answer is the device's).
  Main's default-port tests were moved to the per-namespace fakes and entries.
* The native defaults of React Native, like Swift's and Kotlin's platform defaults, keep one `Kv` directory, one
  `Fs` root and one secure store per app: two cores of one app share them (documented in REACT_NATIVE.md; see Open).
* Gotcha found on the device run: React Native's autolinking cache
  (`android/build/generated/autolinking/autolinking.json`) is keyed by the app's package files, not the
  dependency's `react-native.config.cjs`, so an app built before the package gained its Java library still links
  it as a pure C++ dependency (`ClassNotFoundException: dev.undra.reactnative.UndraPlatform`) until that file is
  deleted. Not this piece's, but anyone updating `@undra/react-native` meets it.

## Deviations from ADR-044 (each with why)

* **17 function pointers and 2 data fields, not 19 pointers.** `abi_version` and `schema_hash` are fields: a host
  reads the version before it trusts any pointer, and reading data cannot run code of another ABI.
* **No module map in the XCFramework; the FFI module lives in the generated Swift package** (`<Bundle>FFI`: the
  header, a module map, an empty `.c`). Xcode copies the `Headers` of every linked XCFramework into one directory,
  so two cores' `module.modulemap` files collide; the package's own target also lets `swift build` and
  `typecheck_swift` compile the bindings without an XCFramework.
* **Namespaces are lowercase only** (`[a-z][a-z0-9_]*`, ≤ 32): one spelling for the symbol, the library file names
  on case-insensitive file systems, the JNI class path and the Pascal-case entry.
* **Swift `LoadOptions.inproc(api: UnsafeRawPointer?)`**, not `UnsafePointer<UndraApi>`: the generated header
  returns `const void *` so `undra.h` stays the one owner of the type; the runtime checks the table before it binds it.
* **Kotlin natives are instance methods of the generated object** (`override external fun` of `NativeApi`), not
  `@JvmStatic`: the runtime calls them through the interface with no reflection, and `RegisterNatives` binds them on
  the object's class the same way.
* **TypeScript gained one runtime member**, `UndraCore.unloaded`, the closed placeholder a generated entry returns
  while its core is not loaded (the ADR said "unchanged except the entry").
* **`undra build` refuses a sibling project with the same namespace** (not in the ADR): two cores of one app usually
  sit next to each other, and the clash would otherwise surface only at load.
* **React Native on iOS finds a core through an Objective-C class** (`UndraCoreTable_<ns>`, `+api`), not by
  calling the symbol from the core's header: the module is compiled once for all cores and names none. ADR-038 now
  says so.
* **S26 keeps its provisional number.**

## Found on the way

* **Handles are per core.** Two cores with the same history issue the same handle numbers. Generated objects carry
  their core, so this is correct, but a value-vs-handle mix-up across cores cannot be caught by the number alone;
  S26 step 4 and the guide say so. (ADR-040's object parameters will want to check the core.)
* **SwiftPM names a local package after its directory**: two generated packages both in `swift/` collide, so the
  iOS two-core app and the Swift contract column reach them through `Packages/PlaygroundA|B` symlinks.
* **Android two-core app needs `Dispatchers.Main`** (kotlinx-coroutines-android) and the adapters' frame pacer, like
  any Compose app; the first version saw no change-sets.
* **Pre-existing, not this piece:** the web playground's stress smoke (`Stress.stop failed: stale handle`) fails on
  main `59f807d` too; the `undra init` Android app dies when `undra dev` already serves another client (it reproduced
  with the iOS app connected).

## Open

* The default storage adapters (`Kv`, `Fs`, `SecureStore`) are per app on every platform, so two cores of one app
  share them unless the app passes its own; a per-namespace default directory is a follow-up decision.

* An on-device React Native app with two cores and S26 in the React Native contract column (the C++ host test does
  load two real cores in one process).
* JavaScript reload sequences on Metro were not re-run after the table (the host's reload race test passes).
* Gradle `:runtime:test` (JUnit) could not run offline; the same tests run through `test-local.sh` under both
  Kotlin compilers.
* ADR-044 is still `Proposed`; the integrator accepts it with the merge.
