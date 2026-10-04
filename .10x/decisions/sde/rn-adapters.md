# SDE - React Native's default ports (wt/rn-adapters, G1b, 2026-10-01)

ADR-038 left a React Native app to supply its own `Kv`, `SecureStore`, `Fs` and `Connectivity` adapters ("React Native's
core has none"). This piece removes that limit, as the Swift (`Adapters.platformDefault`), Kotlin
(`AndroidPlatformDefaults.install`) and web (`browserAdapters()`) runtimes already did: `loadNative({ expectedSchemaHash })`
gives the core all ten standard ports. The design is **ADR-038 amendment B** (written before the code, accepted by the
coordinator): a hybrid, with one portable C++ `Kv`/`Fs` for both phones and per platform only what C++ cannot reach.

## The port table

| Port | Implemented in | iOS | Android | Tests |
|---|---|---|---|---|
| `Kv` | C++ (`cpp/UndraStores.cpp`), both phones | files in `<Application Support>/<bundle id>/Undra/kv`, `<fnv1a64>-<fnv1a32>` names (the Swift `KvAdapter`'s) | files in `<filesDir>/undra/kv`, SHA-256 names (`android-adapters`') | `stores_test` (layouts against the Swift runtime's own name vectors and FIPS SHA-256 vectors, collisions, damaged entries, junk skipped, 1 MiB value, 0600, no temp left); `host_test` through the real core (`kv_put`/`kv_get`/`kv_keys`, file bytes checked, no JS port call); device RN11 + RN18 |
| `SecureStore` | iOS: Objective-C++ (`ios/UndraPlatformApple.mm`); Android: the C++ store over values sealed by Java (`UndraPlatform.seal`/`open`) | Keychain generic passwords, service `dev.undra.securestore`, `AfterFirstUnlockThisDeviceOnly` (the Swift adapter's items) | AES-256-GCM under the `AndroidKeyStore` key `dev.undra.securestore`, `format, iv, ct+tag`, AAD `undra.secure:<key>`, files in `<noBackupFilesDir>/undra/secure` (`android-adapters`' alias, layout, lock file, directory) | `host_test` with a test store (round trip through the core; a failing store answers unavailable and logs, never "missing"; a worker mid-job at shutdown); `android/test/run.sh` (the seal against an AES-GCM vector computed with Node; `android-adapters`' own `SecureSeal.open` opens it too, checked in a scratch run); device RN12 + RN17 |
| `Fs` | C++, both phones | `<Application Support>/<bundle id>/Undra/fs` | `<filesDir>/undra/fs` | `stores_test` (any `..` Denied; links in, out, dangling, in the middle and at the end Denied; delete removes a link without following it; a tree with a link out deleted without touching the target; 300-deep tree; root never deleted; NUL Io; a file as a directory; a refused permission Denied); `host_test` through the core (`../` is the typed `FsError::Denied`, nothing written outside); device RN13 |
| `Connectivity` | native event source | `nw_path_monitor` on its own serial queue | `ConnectivityManager.registerDefaultNetworkCallback` on a handler thread (Java), into C++ through a `RegisterNatives` callback | `host_test` with a scripted source (first report reaches the core before any store exists; repeats sent once; source stopped before the slot is released); `android/test/run.sh` (classification); device RN15 + RN20 (airplane mode, Android) |
| `Http` | TypeScript, `reactNativeHttp()` | React Native's `fetch` (`NSURLSession`) | React Native's `fetch` (OkHttp) | `npm test` with a scripted `fetch` (status, repeated headers, exact body bytes from a view, error status is a response, offline `TypeError` is `Network`, timeout over the exchange, abort `Cancelled`, eight URLs React Native's `URL` would accept are `InvalidUrl`); device RN14 (loopback GET through the core, the app's override header present; `127.0.0.1:1` is `HttpError.Network`) |
| `Lifecycle` | TypeScript, `appStateLifecycle()` (unchanged) | `AppState` | `AppState` | `npm test` (the default reports AppState to the core); device RN16 + RN19 |
| `Clock`, `Rng`, `Log` | C++, unchanged | | | unchanged |
| `Timer` | the core's own | | | unchanged |

Overrides: `nativeDefaultPorts()` keeps a port native only when the app did not override it (`adapters.<name>` set to an
adapter or `null`, or `ports[<id>]`); the JS side passes the chosen ids to `start` (`nativePorts`), the module keeps those the
platform offers (`platformDefaults()`), registers a native `port_cb` for them before `undra_init`, and starts the
`Connectivity` source after it. `nativePlatformDefaults()` reports the ports and the directories.

## What was built

* **C++** (`cpp/`): `UndraStores.{h,cpp}` (wire reader/writer, UTF-8 validation, FNV-1a, SHA-256, `KvStore`, `FsRoot`:
  `openat` + `O_NOFOLLOW` from the root's descriptor, `fsync` + `renameat`, a delete walk that keeps a stack of names so a
  tree of any depth needs a bounded number of descriptors); `UndraDefaults.{h,cpp}` (`Platform`, `SecretStore`,
  `ConnectivitySource`, `Worker` (one serial, named thread per port, started on first use), `NativeDefaults`: queue on the
  port's worker and answer with `undra_port_reply`; `Connectivity` with `undra_event`, deduplicated);
  `UndraPlatformAndroid.cpp` (JNI: the class through the JS thread's context class loader, `RegisterNatives`, workers attached
  once); `Host` takes `StartOptions` and stops the source and joins the workers **before** releasing the process's slot
  (amendment B, B3); the JSI binding gains `platformDefaults()` and `start`'s sixth argument.
* **iOS**: `ios/UndraPlatformApple.mm` (Objective-C++ over Foundation, Security and Network.framework; no Swift); the podspec
  links Security and Network.
* **Android**: `android/` is now a Java library (no Kotlin, no dependency): `UndraPlatform`, `SecureSeal`,
  `NetworkClassifier`, `NetworkMonitor`, `UndraContextProvider` (the application context before `Application.onCreate`),
  `UndraReactNativePackage` (empty: autolinking links a library with a Gradle project only when it has a `ReactPackage`); the
  manifest merges `INTERNET`, `ACCESS_NETWORK_STATE` and the provider; consumer R8 rules keep the JNI-named classes. The C++
  module is still autolinked through `cxxModuleCMakeListsPath` (React Native's Gradle plugin registers C++ modules of
  libraries with a Gradle project too; checked in `GenerateAutolinkingNewArchitecturesFileTask`), and the library applies
  `com.facebook.react` so the package's codegen runs for it.
* **TypeScript**: `reactNativeHttp()`, `nativeDefaultPorts()`, `nativePlatformDefaults()`; `loadNative` installs `Http` and
  `Lifecycle` and picks the native ports; `NativeTransportOptions.nativePorts`.
* **Playground core**: `examples/playground/core/src/platform.rs` (one file, one line in `lib.rs`): `kv_*`, `secret_*`,
  `file_*`, `http_get`, and a `Device` store of the `Connectivity` and `Lifecycle` reports, fed by an `InitHook` from the
  moment the runtime exists (the platforms send the first reports before any store can be created), every signal
  `no_coalesce`. Six Rust tests with `undra::ports::fakes`. Bindings regenerated (schema hash `0xefd907be3070520a`).
* **Playground RN app**: the in-memory `Kv` is gone; one override kept as the sample (an `Http` wrapper adding
  `x-undra-playground: rn` around `reactNativeHttp()`); RN11..RN16; the restart token and the `CONNECTIVITY`/`LIFECYCLE`
  lines; cleartext only to the loopback in a network security config.
* **`scripts/rn-device-checks.sh`**: a loopback server for RN14 (`adb reverse` on Android), then RN17 (scan of the app's
  files: the simulator's data container, `/data/data/<package>` through `su` on Android), RN18 (kill, relaunch), RN19 (Home or
  Settings, then back), RN20 (Android airplane mode, always switched off on exit), and the total line.
* **CI**: `ci.yml`'s React Native job also runs `android/test/run.sh` (JDK 17); `run.sh` gained the stores test (step 0)
  and the Apple compile check (step 4, macOS only).
* **Docs**: `docs/REACT_NATIVE.md` (the port table, what each default does, replacing a default, the `registerPort` limit and
  why, why Java not Kotlin, the test table, troubleshooting); `site/docs/ports.html` says what React Native installs; the
  site's generated pages follow the playground's bindings.

## Evidence (this Mac, 2026-10-01, shared with other agents' builds and the `undra` emulator)

iPhone 17 Pro simulator (iOS 26.5), release core, Release app, `scripts/rn-device-checks.sh ios`:

```
UNDRA-RN CHECK RN11 PASS Kv default: a round trip through the core, answered natively: put, get, keys, remove through the core; 5 native port calls
UNDRA-RN CHECK RN12 PASS SecureStore default: ... stored and read back (marker nonce mupzywke2ck5m); Keychain service dev.undra.securestore
UNDRA-RN CHECK RN13 PASS Fs default: write, read, list, delete inside the root; a path outside is Denied: ... /Library/Application Support/dev.undra.playground.rn/Undra/fs; ../ Denied
UNDRA-RN CHECK RN14 PASS Http default: ... GET http://127.0.0.1:8737/undra-rn-check -> 200 with the override's header; 127.0.0.1:1 -> HttpError.Network
UNDRA-RN CHECK RN15 PASS Connectivity default: the core received the native source's report: online=true kind=wifi after 1 report(s)
UNDRA-RN CHECK RN16 PASS Lifecycle default: the core received AppState: state=active after 1 report(s)
UNDRA-RN CHECKS 16/16 passed
UNDRA-RN CHECK RN17 PASS SecureStore is not plain text in the app's files: UNDRA-SECRET-mupzywke2ck5m in no file; the Kv marker next to it in 1 (the scan sees the files)
UNDRA-RN CHECK RN18 PASS Kv survives the process: the first launch wrote mupzywke2ck5m; after the kill the second read mupzywke2ck5m
UNDRA-RN CHECK RN19 PASS Lifecycle reaches the core: Settings in front: 'UNDRA-RN LIFECYCLE state=background reports=4'; back: 'UNDRA-RN LIFECYCLE state=active reports=5'
== UNDRA-RN CHECKS 19/19 passed
```

`undra-rn` emulator (arm64, API 35, `emulator-5556`), release core, release APK, `scripts/rn-device-checks.sh android`:

```
UNDRA-RN CHECK RN12 PASS SecureStore default: ... (marker nonce mupzzxeg8u6zb); Android Keystore key dev.undra.securestore, sealed files in /data/user/0/dev.undra.playground.rn/no_backup/undra/secure
UNDRA-RN CHECK RN13 PASS Fs default: ... under /data/user/0/dev.undra.playground.rn/files/undra/fs; ../ Denied
UNDRA-RN CHECK RN14 PASS Http default: ... -> 200 with the override's header; 127.0.0.1:1 -> HttpError.Network
UNDRA-RN CHECKS 16/16 passed
UNDRA-RN CHECK RN17 PASS SecureStore is not plain text in the app's files: UNDRA-SECRET-mupzzxeg8u6zb in no file; the Kv marker next to it in 1 (the scan sees the files)
UNDRA-RN CHECK RN18 PASS Kv survives the process: the first launch wrote mupzzxeg8u6zb; after force-stop the second read mupzzxeg8u6zb
UNDRA-RN CHECK RN19 PASS Lifecycle reaches the core: Home: 'UNDRA-RN LIFECYCLE state=background reports=4'; back: 'UNDRA-RN LIFECYCLE state=active reports=5'
UNDRA-RN CHECK RN20 PASS Connectivity follows airplane mode: on: 'UNDRA-RN CONNECTIVITY online=false kind=none reports=2'; off: 'UNDRA-RN CONNECTIVITY online=true kind=wifi reports=3'
== UNDRA-RN CHECKS 20/20 passed
```

## Findings

1. **The Swift runtime's `FsAdapter` follows symbolic links out of its root (open item, not fixed here: the Swift runtime is not
   this piece's).** SPEC 8: "one that would leave it, through `..` or a symbolic link, is `Denied`". `FsAdapter.resolve` only
   refuses `..` components. Repro (a scratch copy of `runtimes/swift/UndraRuntime` with one `@testable` test, `swift test`,
   Xcode 26.6): root `<tmp>/root`, a link `<tmp>/root/out -> <tmp>/outside`, a file `<tmp>/outside/secret.txt` = `outside`.
   * `FsAdapter.read("out/secret.txt", root:)`: **expected** `FsError.denied`; **observed** returns 7 bytes, `outside`.
   * `FsAdapter.write("out/new.txt", data:, root:)`: **expected** `FsError.denied`; **observed** succeeds, and
     `<tmp>/outside/new.txt` exists.
   A follow-up can start from that test: the fix is an `lstat` walk (or `open(O_NOFOLLOW)`) per component, as the C++ `FsRoot`
   here and the JVM `FsAdapter` (`toRealPath` check) do.
2. **React Native on Android pauses the JS timers of a backgrounded app** (`JavaTimerManager` on `onHostPause`), so the
   mirror's drain does not run until the app is back, and coalesced delivery (ADR-031) kept only the last of the
   `background` and `active` reports the core had received in between: the first RN19 run on Android saw the counter jump
   from 2 to 4. The core was right; the UI view of it lost a value. The `Device` store's signals are `no_coalesce` now (each
   report is applied, in order), and RN19 checks the background report once the app is back. Apps whose UI must show every
   transition of an event-driven signal want the same attribute.
3. **React Native's `URL` is a regex shim that never throws** (`new URL("not a url")` succeeds, `protocol` is `""`): the web's
   `fetchHttp` would rely on it. `reactNativeHttp()` checks the URL shape itself before any `fetch`.
4. **`AppState` announces a state more than once while an app starts** (a cold start: `background` from the module, which
   is created before the activity resumes, then `active`; after a force-stop on Android, `active` three times: the module's
   initial value, its `getCurrentAppState` answer and `onHostResume`). Each became a `Lifecycle` report, and every `Active`
   report makes the query layer look for stale queries. `appStateLifecycle()` now reports a state only when it differs from
   the last one it reported (tested). The cold start's `background` is React Native's own answer and is kept.
5. **`pipefail` and a scan that must find nothing**: the first iOS run's script aborted at RN17 because `grep` found no secret
   (exit 1). Fixed with `|| true` inside the pipelines.

## Deviations from the brief

* **"The TS fakes already exist in `@undra/runtime`"**: they do not (only the test-support `FakeCoreTransport`). The RN
  package's tests use its fake module and a mocked React Native surface, plus an in-test `memoryKv`; the core-side logic of
  the checks uses the Rust `undra::ports::fakes` (coordinator: noted, proceed).
* **"The loopback demo server the playground already has"**: the React Native app has none (that is the Kotlin app's
  `DemoServer.kt`). The device script runs a small loopback server on `127.0.0.1:8737` (the simulator shares the Mac's
  loopback; Android reaches it with `adb reverse`), which is also what the CI jobs get.
* **`android-adapters` is not a dependency** (amendment B, option (a) rejected): sharing is by format and test, not linking.
* The playground core gained the `platform` module (the only way to exercise each default **through the core**); its schema and
  the checked-in bindings changed. At a cross-merge with `abi-table` / `persistence-v2` / `dev-reload`, regenerate the bindings
  (`undra bindgen -C examples/playground --docs`) and the site (`node site/scripts/build-all.mjs`) rather than hand-merging.
* Airplane mode is Android-only (the iOS simulator has none): iOS reports 19 checks, Android 20.

## Suites (final, at the merge of `main`)

Run at `806c1c1` (main `3e8a304` merged: tooling, wasm-size) on this Mac:

| Suite | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | 2,562 passed, 0 failed, 11 ignored (playground core 75, of which `platform` 6) |
| `bash contract-tests/run-all.sh` | 18 x 3 = 54/54 |
| `cpp/test/run.sh` (ASan + UBSan) | stores 13; host 20 (linked shim) + 20 (dlopen shim); `UndraJsi.cpp` and `UndraPlatformApple.mm` compile |
| `android/test/run.sh` | 4 |
| RN `npm test` / `npm run typecheck` / `npm run test:contract` | 59 / clean / 17 pass + S17 skipped (app-tested) |
| TS runtime `npm test` (unchanged) | 1,128 |
| `undra bindgen -C examples/playground --docs --check`; `node site/scripts/build-all.mjs` | up to date (schema hash `0xefd907be3070520a`); site up to date |
| Devices, full builds after the merge | iPhone 17 Pro simulator `UNDRA-RN CHECKS 19/19 passed`; `undra-rn` emulator `UNDRA-RN CHECKS 20/20 passed` (the `undra-rn` emulator was shut down afterwards; `emulator-5554` untouched) |

## What the integrator owns

* `.10x/status.md` / `handoff.md`: G1b landed; the Swift symlink finding as an open item for the Swift runtime.
* `rn-devices.yml` was not run on a GitHub runner by this piece: the Android job's RN17 needs `su` (the x86_64 `google_apis`
  emulator images have it) and RN20 toggles airplane mode on the runner's emulator.
* Maven Central for the Kotlin runtime would make option (a) possible on Android later; the amendment says why it would still
  not be preferred.
