# SDE - ns-storage: default storage per core namespace (ADR-044 amendment A, wt/ns-storage, 2026-10-02)

ADR-044's amendment A, implemented. Two cores of one app shared nothing in memory but the default `Kv`, `Fs`,
`SecureStore` and `Db` of every runtime still used one location per app, so two cores that both used the defaults read
and overwrote each other's keys and files. **Now every default store lives under `…/undra/<namespace>/<store>`** (`Undra` on Apple) on every
platform; an adapter the app supplies is untouched. No legacy path, no migration (nothing is released). **No wire change, no
schema change, no ABI change**: contract scenarios and the schema hash do not move. The rule is in SPEC 8 (the table), the
as-built record in ADR-044 amendment A.

## Paths per platform

| | `Kv` | `Fs` | `SecureStore` | `Db` |
|---|---|---|---|---|
| Swift, RN iOS | `<Application Support>/<bundle id>/Undra/<ns>/kv` | `…/Undra/<ns>/fs` | Keychain service `<ns>.dev.undra.securestore` | `…/Undra/<ns>/db/<name>.sqlite` |
| `android-adapters`, RN Android | `<filesDir>/undra/<ns>/kv` | `<filesDir>/undra/<ns>/fs` | Keystore alias `<ns>.dev.undra.securestore`, files `<noBackupFilesDir>/undra/<ns>/secure` | `getDatabasePath("undra-<ns>-<name>.sqlite")` |
| Kotlin JVM | `<dataDir>/<ns>/kv` | `<dataDir>/<ns>/fs` | `<dataDir>/<ns>/secure` | `<dataDir>/<ns>/db/<name>.sqlite` |
| TS browser | IndexedDB `undra.<ns>.kv` | OPFS `undra/<ns>/fs` | IndexedDB `undra.<ns>.secure`, `undra.<ns>.secure-keys` | wa-sqlite pool, OPFS `undra/<ns>/db` |

`<dataDir>` = `undra.data.dir` or `~/.undra/data`.

## What landed

1. **Swift** (`f53d32c`). `UndraCore.namespace` (public; `_` when attached without one), `LoadOptions.namespace` (the generated
   `UndraCoreEntry.load` fills it in; an in-process load without it takes the table's), `StorageLocations`, and the default
   `KvAdapter()`, `FsAdapter()`, `SecureStoreAdapter()` (`service: String? = nil`), `SQLiteDbAdapter()` resolving their location
   in `makePortImpl(core:)`, so one adapter value can serve two cores. `DbPortAdapter` over a default `SQLiteDbAdapter` scopes it
   too (`NamespaceScopedDb`). Apple keeps the capital `Undra` (see Deviations: the device checks found that `undra` breaks on a container that holds the older directory).
2. **Kotlin** (`4e6c4b3`). `UndraCore.namespace`, `UNNAMED_NAMESPACE`, `LoadOptions.namespace` (`CoreEntry` fills it in, an
   in-process load takes the natives'), `JvmAdapters.defaults(namespace, …)` / `defaultDataDir(namespace)`; `android-adapters`
   constructors `(context, namespace)`, `keyAliasOf`, `directoryOf`, `rootOf`, `fileNameOf`; `AndroidPlatformDefaults.install` reads
   `core.namespace`. Drive-by: the `LoadOptions` copy that fills the schema hash in dropped `onDevNotice`; it now keeps it.
3. **TypeScript** (`8326715`). `browserAdapters({ namespace })`, `indexedDbKv/webCryptoSecureStore/opfsFs({ namespace })`,
   `AttachOptions.namespace`, `UndraCore.namespace`, `storeName/storePath/UNNAMED_NAMESPACE`, `PortImpl.bind({ namespace })`
   (called when a core registers a port) which `dbPort` hands to its adapter as `DbAdapter.open(name, { namespace })`, the worker
   protocol carrying it, `waSqliteAdapter` choosing `undra/<ns>/db` from it. **One generated-output change**: the entry's `load`
   and `attach` pass `namespace: UndraIds.namespace` (`undra-bindgen/src/ts.rs`, the ten TS goldens, the five committed example
   trees; `undra bindgen --check --docs` is clean on all five).
4. **React Native** (`e371b75`). `makePlatform(name_space, error)` (the table's `name_space`): the Apple platform's directories and
   Keychain service, the Android platform's directories, the Keystore alias (Java keeps one key per alias), the database file
   (`UndraDatabase.open(namespace, name)`); `StoreNames.java` holds the names (pure Java, tested on the JVM).
5. **two-cores, docs** (`801c27b`). See below; SPEC 8 and 11, ADR-044, REACT_NATIVE, the READMEs, the site's `ports` and
   `several-cores`, the playground's `smoke.sh` (`files/undra/playground_core/kv`).

## Tests

* Swift `NamespaceStorageTests` (7: the paths, the Keychain service, `core.namespace`, two cores' Kv, Fs and Db never see each
  other's data through one adapter value, an adapter with a directory or service is untouched) and `UndraCoreEntryTests` (+1).
* Kotlin runtime `AdapterTests` (two cores' JVM defaults: Kv, SecureStore, Fs in two directories, none at the old place, an explicit
  directory used as is) and `CoreEntryTests` (the namespace of an entry's core, a direct load, the placeholder, the options copy);
  `android-adapters`: `NamespaceNamesTest` (JVM), `PlatformLocationsTest` (two cores on a device: Kv, Fs, the real Keystore with two
  aliases, an explicit directory), the existing instrumented tests moved to a test namespace.
* TS `namespace-storage.test.ts` (10: the names, Kv/SecureStore databases of two namespaces, Fs directories in a fake OPFS, the core
  and `browserAdapters`, two cores' Kv through the real port plumbing, `bind` reaching the Db adapter, the worker protocol).
* RN: `cpp/test/apple_platform_test.mm` (the Apple platform on a Mac: where each core's Kv, Fs, Keychain service and Db are; step 5 of
  `cpp/test/run.sh`), `PureTest.java` (the store names).
* **two-cores** (`examples/two-cores`, all four platforms): each core writes `two-cores.key` through its default `Kv` and reads its own
  value back, a key only A wrote is not B's, and the two stores are in two places. The JVM runs with `-Dundra.data.dir` on a
  temporary directory; Node uses `fake-indexeddb` (a `package.json` and lockfile in `node/`, `npm ci` from `run.sh`) because the web's
  default `Kv` is IndexedDB; iOS' app became `async` (`.task`) for it and loads with the default adapters.

## Deviations

* Apple spells the root `Undra`, not `undra` as the rule says: Apple's file systems are case-insensitive, and under the iOS
  simulator `mkdir <…>/undra/<ns>` is ENOENT while `<…>/Undra` exists (found by `scripts/rn-device-checks.sh ios` on a container
  that held the older directory: `UndraError: storage I/O error: cannot create …/undra/playground_core`), so the casing stays the
  one already on disk. The JVM is `<dataDir>/<ns>/…` (no second `undra` segment); Android's database file carries the namespace in
  its name (`getDatabasePath` takes no separator): see ADR-044 amendment A, "as built".
* `SQLiteDbAdapter.directory` is optional, `AndroidKvAdapter(context)`, `AndroidFsAdapter(context)`, `AndroidSecureStoreAdapter(context)`
  and `AndroidDbAdapter(context)` no longer exist (they take the namespace): public-API changes of a library nothing has released.
* `PortImpl.bind` is the one new TypeScript runtime member; a wa-sqlite worker serves one core's directory and refuses another
  namespace (typed `DbError.Unavailable`).

## Verification (on the Mac, the `undra` AVD for the shared emulator, `undra-rn` for the RN device checks)

| Suite | Result |
|---|---|
| Swift `swift test` | 676 pass (`NamespaceStorageTests` 7, `UndraCoreEntryTests` 8) |
| Kotlin runtime `scripts/test-local.sh`, brew's 2.4.20 and CI's 2.0.21 | 755 cases, 0 failed, 2 skipped (the native smoke needs the fixture library); testkit 30 |
| `android-adapters` `:android-adapters:test` (debug and release) | 288 results, 0 failed, 2 skipped |
| `android-adapters` `:android-adapters:connectedAndroidTest` on the `undra` AVD | 151 tests, BUILD SUCCESSFUL (the first run found the Keystore test still reading the old alias; fixed) |
| TypeScript runtime `vitest run` | 1,442 pass (10 new in `namespace-storage.test.ts`), `tsc` clean |
| React Native: `vitest run` / `cpp/test/run.sh` / `android/test/run.sh` | 88 pass (1 new) / exit 0 (stores 15, Db 15, shims 33 each, the Apple platform 13 on a Mac) / 8 checks |
| bindgen `cargo test -p undra-bindgen`, `undra bindgen --check --docs` on playground, cookbook, fieldbook, two-cores a and b | pass, clean |
| Contract `contract-tests/run-all.sh` | 74/74 (S01–S26, S21/S22 TypeScript only) |
| `examples/two-cores` iOS (Debug), Android, JVM, Node | `passed`, the `Kv:` lines included |
| `scripts/rn-device-checks.sh ios` / `android --target emulator-5556` | 24/24 / 25/25 (RN12 and RN13 name the per-namespace Keystore alias, directories and database file) |

The iOS device checks failed once, usefully: with the lowercase `undra` they stopped at the first `Kv` write
(`cannot create …/undra/playground_core: No such file or directory`) on a simulator container that held the older `Undra`.
Apple went back to `Undra`; the same checks then passed.
