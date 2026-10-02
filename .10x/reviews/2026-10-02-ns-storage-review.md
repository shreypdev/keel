# ns-storage (ADR-044 amendment A: default storage per namespace) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/ns-storage`
at `bc6061d` (`main` `3fc8b7f` merged), reviewed with `main` at `d1b35b5` (ios-floor) merged once at the end · **Scope (four
surfaces only):** the namespace as a path component; byte compatibility across hosts; migration of nothing and the
playground's `smoke.sh`; the matrix · **Read:** ADR-044 amendment A "as built", `.10x/decisions/sde/ns-storage.md`, SPEC 8's rule
table and the diff of the TypeScript, Swift, Kotlin, `android-adapters` and React Native (C++, Objective-C++, Java) halves ·
**Fixes:** `44fc93a` (F1, F2, F5, F6), `8804894` (F4, regenerated after `main` was merged), `b058bf8` (the TypeScript check made smaller, F3); `main` merged at `ca93c1d`.

## Verdict

**Merge after one decision (F3, the web JavaScript gate), which is the integrator's, not the reviewer's.** The design is right
and the implementation is careful: every default store is under the namespace, nothing of an app's own adapter moves, the
Apple casing deviation is correct and found the hard way, and the byte layouts of the React Native module and the native
shells agree. What the review found is at the edges, not in the middle:

* **F1 (Medium, fixed).** The namespace was a path component that nobody validated where it enters from outside the table
  (TypeScript `AttachOptions.namespace`, Swift and Kotlin `LoadOptions.namespace`, the in-process table's name, the
  `android-adapters` constructors, React Native's `makePlatform`): `..`, `a/b`, an empty string or 5,000 characters became an
  IndexedDB name, an OPFS directory, a directory under `Application Support`, a Keystore alias or a file name. Each host now
  refuses it typed, by the rule of `undra.toml`, before anything is created.
* **F2 (High for CI, fixed).** `cargo test -p undra-bindgen` with `tsc` on `PATH` and `UNDRA_REQUIRE_TOOLCHAINS=1` (what CI
  does) failed **19 tests** (`run_ts` 8, `typecheck_ts` 11): the generated TypeScript passes `namespace`, the hand-written
  `tests/fixtures/ts-base/index.d.ts` did not declare it, and `ts-run/objects.mjs` still expected the old `attach` options.
  The implementer's "bindgen passes" was a run in which those toolchain tests skip.
* **F3 (Medium, open).** The hello-world JavaScript is over its gate: see "The JavaScript number".
* **F4 (Medium, fixed).** The site's generated files were stale (`reference/typescript.html`, `llms-full.txt`,
  `search-index.json`): CI's "Generated files are up to date" step would have failed.
* **F5 (Low, fixed).** `examples/playground/android/smoke.sh` still looked for `undra-playground.sqlite`; the file is
  `undra-playground_core-playground.sqlite` (as are the README and `NotesScreen.kt`'s comments).
* **F6 (Low, fixed).** SPEC 8's `Db` paragraph contradicted the new rule: it said two cores that open one name "share it by
  design" and listed the old paths (`undra/db/<name>`, `getDatabasePath("undra-<name>.sqlite")`).

No data-loss, wire, ABI or schema finding: the contract grid is unchanged and passes.

## Findings

| # | Sev | What | Where | State |
|---|---|---|---|---|
| F1 | Medium | namespace from outside the table not validated | `runtimes/ts/@undra/runtime/src/adapters/names.ts:35` (`checkNamespace`, called by `storeName`/`storePath`, so every IndexedDB/OPFS name), `core.ts` (`attach` async); `runtimes/swift/.../Adapters/StorageLocations.swift:49` (`CoreNamespace`), `Core/UndraCore.swift` (`load`, `connect`), `Core/Errors.swift` (`invalidNamespace`); `runtimes/kotlin/.../runtime/CoreNamespace.kt`, `UndraCore.kt` (`start`, `attach`), `adapters/JvmAdapters.kt` (`defaultDataDir`), `android-adapters/.../Android{Kv,Fs,SecureStore,Db}Adapter.kt`; `runtimes/rn/.../cpp/UndraDefaults.h` (`validStorageNamespace`, both `makePlatform`s), `android/.../StoreNames.java` | fixed, tests per host |
| F2 | High (CI) | 19 bindgen tests fail with toolchains required | `crates/undra-bindgen/tests/fixtures/ts-base/index.d.ts`, `tests/fixtures/ts-run/objects.mjs:93` | fixed (148 pass) |
| F3 | Medium | web hello-world JS over its gate | `bench/budgets.toml:707` | open, decision |
| F4 | Medium | site's generated files stale | `site/reference/typescript.html`, `site/llms-full.txt`, `site/search-index.json` | fixed |
| F5 | Low | `smoke.sh` expects the old database file name | `examples/playground/android/smoke.sh:186` | fixed (not run: it toggles airplane mode on the shared AVD) |
| F6 | Low | SPEC 8 `Db` paragraph says the old paths and "share by design" | `docs/SPEC.md:678` | fixed |
| F7 | Low | TypeScript accepts the explicit namespace `_` (the unnamed core's), Swift and Kotlin refuse it as an option | `names.ts`, `StorageLocations.swift`, `CoreNamespace.kt` | documented in SPEC 8; harmless (`_` is what none means) |
| F8 | Info | React Native's `loadApi` accepts any C identifier (`Foo`, `_x`) as a namespace; Apple's file systems are case-insensitive, so `Foo` and `foo` would share a directory | `cpp/UndraApi.cpp:12` | the storage rule in `makePlatform` is the stricter, `undra.toml` one; `loadApi` unchanged (it names a class and a library, not a path) |

## 1. The namespace as a path component

**Attacks.** Sixteen hostile values per host: `..`, `.`, `a/b`, `a\b`, `../escape`, empty, 33 characters, `Upper`, `1abc`, `_hidden`,
`café`, `a\0b`, `with space`, `with-dash` (the Android database file's separator), `a.b`, and for TypeScript non-strings (`42`,
`null`, `{}`, `["a"]`, `1n`) and 5,000 characters (message stays under 600). Each at every entry that takes the string.

| Host | Entries attacked | Result |
|---|---|---|
| TypeScript | `UndraCore.attach`, `UndraCore.load`, `storeName`, `storePath`, `indexedDbKv`, `webCryptoSecureStore`, `opfsFs`, `browserAdapters`, `waSqliteAdapter.open` (the worker is told the namespace by message) | before the fix 51 of 62 tests failed (every one accepted `..` and created an IndexedDB database called `undra...kv`); after: `attach` and `load` reject `UndraError("options")` with the transport untouched and no database created, the factories throw the same, the worker adapter answers a typed `DbError.Unavailable` |
| Swift | `UndraCore.connect` (the choke point), `UndraCore.load`, an in-process table whose `name_space` is hostile | `UndraLoadError.invalidNamespace` before `start`, before `init`, before the in-process claim (`isClaimed` false) |
| Kotlin | `CoreEntry.load`, `UndraCore.load(options, native)`, `attachTransport`, `JvmAdapters.defaultDataDir`, `AndroidSecureStoreAdapter.keyAliasOf`, `AndroidDbAdapter.fileNameOf`, the four adapters' `(context, namespace)` constructors | `UndraModeException` for the options, `IllegalArgumentException` for the adapters; nothing started (`UndraCore.current` unchanged) |
| React Native | `makePlatform(name_space)` on Apple and Android, Java `StoreNames` (`keyAlias`, `securePath`, `databaseFileName`) | refused with a message before a path, a service or an alias is made; `UndraDatabase.open` turns the Java refusal into its `failure` bytes (it catches `RuntimeException`) |

`loadApi` already bounds the string the JavaScript `install` passes to a C identifier of at most 32 characters, and the table's
`name_space` must equal it, so no `..` reached C++; what it admitted was uppercase and a leading `_` (F8), which matters on
Apple. The check is in `makePlatform`, where the directory is made, and tested directly on a Mac (17 more checks in
`apple_platform_test.mm`, 30 in all).

**The `_` fallback.** A core attached without a namespace has `_` (`UndraCore.namespace`); two such cores share the stores of
`_`; a generated entry always sets one (it passes `UndraIds.namespace` / the table's `name_space`). Now said in SPEC 8 and on the
public declaration in TypeScript, Swift and Kotlin.

## 2. Byte compatibility across hosts

* **RN iOS ↔ Swift.** Same directory: `<Application Support>/<bundle id>/Undra/<namespace>/{kv,fs,db}`, same fallbacks (`app`,
  the temporary directory), same Keychain service, same `kSecAttrAccessible`. The Kv file names are the Swift runtime's FNV
  vectors on both sides (`stores_test.cpp`, `AdapterTests.swift`) and the header layout is shared. What was missing was the
  *same literal* for the namespace segment on both sides: `apple_platform_test.mm` asserts `/Undra/playground_a/kv`, `fs`, `db` and
  the service `playground_a.dev.undra.securestore`, while Swift's test compared `StorageLocations` with itself. Added
  `testTheLayoutOfOneNamespaceIsTheLiteralTheReactNativeModuleChecks`, which asserts the same strings on `StorageLocations`,
  `KvAdapter.defaultDirectory` and `SQLiteDbAdapter.defaultDirectory`. **Limit:** there is no in-process Swift-writes,
  C++-reads round trip (the two cannot share a test binary); parity rests on the vectors, the literals and the device checks.
* **RN Android ↔ `android-adapters`.** `files/undra/<ns>/{kv,fs}`, `noBackupFiles/undra/<ns>/secure`, Keystore alias
  `<ns>.dev.undra.securestore`, `undra-<ns>-<name>.sqlite`: `PureTest.java` and `NamespaceNamesTest.kt` assert the same alias
  and file-name literals; the directories are asserted by `PlatformLocationsTest` and RN12/RN13 of `rn-device-checks.sh` on a
  device (the implementer's run). A namespace has no `-`, so the file name parses unambiguously, now enforced (a `-` is refused).
* **`Undra` on Apple, `undra` elsewhere.** In SPEC 8 (the table and the sentence), in the C++ (`UndraPlatformApple.mm`,
  `UndraPlatformAndroid.cpp`) and in Swift (`StorageLocations.root()`), with the reason (case-insensitive file systems, ENOENT under
  the simulator on a container that holds the older `Undra`). Correct, and the right place to keep it.

## 3. Migration of nothing; `smoke.sh`

* **Migration notice: not added, recorded.** Nothing is released, and the only old data a dev machine's tooling can see is the
  JVM's `~/.undra/data/{kv,secure,fs,db}`; the iOS, Android and browser data are in simulators, emulators and browsers that
  `undra doctor` cannot reach. A doctor check needs a `Check` id, a `Sys` fake, a docs anchor under ONBOARDING's headings and the
  report/JSON tests, and the JVM directory names (`kv`, `fs`, `db`, `secure`) are also valid namespaces, so a correct "old data
  is here" test must look inside for files rather than directories. That is over the 30 minutes for a notice that reaches only the
  JVM; left as an open item.
* **`smoke.sh`** (Android): the Kv directory (`files/undra/playground_core/kv`) was right; the database file was not (F5). Fixed
  statically (`undra-playground_core-playground.sqlite`, `grep -F`) and the README and `NotesScreen.kt` comments. **Not run:** it
  switches the shared AVD's airplane mode on and off. The instrumented tests ran instead on the `undra` AVD (147 tests, 0 failed).
  The iOS `smoke.sh` names no storage path.

## 4. The matrix

All on the Mac (`source scripts/env.sh`, `DEVELOPER_DIR` set, `UNDRA_REQUIRE_TOOLCHAINS=1`, `tsc` on `PATH`). "Pre" is before `main`
was merged, "post" after.

| Suite | Result |
|---|---|
| `cargo test -p undra-bindgen -p undra-cli` (pre, first run) | 516 pass, **19 fail** (F2), 2 ignored |
| `cargo test -p undra-bindgen` (post, after F2) | 148 pass, 0 fail |
| `undra bindgen --check --docs` playground, cookbook, fieldbook, two-cores a and b (post) | all five "up to date" |
| TypeScript runtime `vitest run` + `tsc --noEmit` (post) | 1,495 pass (1,442 + 53: 52 hostile-namespace cases, 1 for `_`), clean |
| Swift `swift test` | 680 pass pre (676 + 4), 686 post (ios-floor added 6), 0 fail |
| Kotlin `scripts/test-local.sh`, brew 2.4.20 and the 2.0.21 compiler of Gradle 8.14.3 | 756 cases, 0 failed, 2 skipped, under both (755 + 1); testkit 30 |
| `android-adapters` `:android-adapters:test` | 145 per variant, debug and release, 0 failed, 1 skipped each |
| `android-adapters` `:android-adapters:connectedAndroidTest`, the `undra` AVD | 147 tests, 0 failed, 5 skipped (the network-off test and the four `RealtimeOnDeviceTest`s, which need the host test server: the implementer's 151 ran them); the new `a_namespace_that_is_not_one...` ran on the device |
| React Native `vitest run` | 88 pass |
| React Native `cpp/test/run.sh` | exit 0 (stores, Db, shims, the Apple platform now 30 checks) |
| React Native `android/test/run.sh` | pass (the new store-name checks included) |
| `bash contract-tests/run-all.sh` (post; the TypeScript column again after the last runtime edit) | **74/74** (S01–S26, S21/S22 TypeScript only) |
| `examples/two-cores` JVM and Node (post) | `passed`, with the `Kv:` lines (own value back, a key only A wrote is not B's, two stores) |
| `scripts/wasm-size.sh` | wasm **116,857** B gz, gate 120,000: ok; hello JS **26,221** B gz, gate 26,000: **OVER by 221** (F3) |
| site `build-all` + `check-links --words` | three generated files regenerated and committed (F4); links ok, landing prose 342 of 350 words |

Not run, said so: `scripts/rn-device-checks.sh` (the implementer's 24/24 and 25/25 stand; my changes touch only the namespace refusal
before the paths those checks exercise), `examples/two-cores` iOS and Android (same), Fieldbook's `npm test` and RN's
`npm run typecheck` (need `npm ci`; this checkout's RN `node_modules` does not link `@undra/runtime`).

## The JavaScript number (F3)

What a hello-world app ships of `@undra/runtime` (`scripts/web-size-runtime.mjs`, gzip level 9), measured per commit with the same
Vite:

| Tree | gz bytes | against the 26,000 gate |
|---|---|---|
| `main` before the piece (`3fc8b7f`, the recorded 25,996) | 25,996 | 4 under |
| the piece as delivered (`bc6061d`) | 26,138 | **138 over** (the piece had not run the gate) |
| the review's first validation (long messages, helper) | 26,503 | 503 over |
| **this branch** (one regex in `storeName`/`storePath`, async `attach`, no `optional()` helper) | **26,221** | **221 over** |

The namespace costs the main entry 225 bytes in all (`names.ts`, the `bind` calls in `core.ts`, the OPFS directory walk, the options
plumbing): the piece's 142, and the review's validation net of its slimming, 83. `bench/budgets.toml` says "the next change to the hello runtime pays
for itself, or `ts-runtime-size` lands", and the gate is R9, so it is **not** raised here. What is left to decide: (a) a restatement
of ADR-052's decision 2 (it was 8 KB, then 24, then 26) to 26,300 with `scripts/wasm-size.sh --record` in the same commit, or (b)
landing `ts-runtime-size` first and merging this after. Cheaper levers exist inside the piece (drop `UndraCore.namespace` from the
hot path, fold the two `bind` call sites into one helper, flatten the OPFS walk to one `getDirectoryHandle` chain) but they are
tens of bytes each, not 221.

## Open items

1. **F3**, the web JavaScript gate (above): a decision, one commit either way.
2. The migration notice (section 3): recorded, not built.
3. `smoke.sh` on the AVD (airplane mode) and `rn-device-checks.sh`/two-cores iOS and Android were not re-run by the reviewer.
4. No in-process Swift ↔ C++ Kv round trip; parity is by vectors, literals and the device checks.
5. Two cores loaded without a namespace share `_`: documented, deliberately not an error (a scripted test transport is the
   case that has none).
