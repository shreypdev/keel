# Playground, Android

The Compose app of the playground: five screens over the one Rust core (`../core`), through the Kotlin
bindings `undra bindgen` generated (`../generated/kotlin`, package `dev.undra.playground.core`).

| Tab | Store | What it shows |
|---|---|---|
| Todos | `Todos` | a keyed list, a filter chip row and two computed values (`visible`, `remaining`); `add` fails with the typed `TodoError.EmptyTitle` |
| Counter | `Counter` | one tap is one transaction: `count`, `changes` and the computed `parity` arrive in one change-set |
| 10k list | `BigList` | 10,000 keyed rows in a `LazyColumn`; insert, update, move and remove cross as one-operation patches; "Stream updates" sends ten a second |
| Remote | `RemoteTodosQueryHandle` | a cached server list (`inbox`) with status, fetching, updated-at and error; optimistic add and toggle; an Offline switch that queues your additions and replays them |
| Notes | `Notes` | notes kept in SQLite through the opt-in `Db` port (ADR-048): the platform's `android.database.sqlite` (`AndroidDbAdapter`, file `undra-playground_core-playground.sqlite`, per core namespace), migrated on open; add, toggle and remove change the database first, then the keyed list; they survive a restart |

Every screen reads its store with `collectAsState()` on the store's `StateFlow`s. A `ViewModel` owns each
store, so the state (which lives in the core) survives switching tabs and rotating the device.

## Build and run

```sh
undra build -C .. --platform android --release   # ../build/android/jniLibs/<abi>/libplayground_core.so (1.5 MB each)
./gradlew :app:assembleDebug                    # app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.undra.playground/.MainActivity --es tab remote   # todos | counter | biglist | remote | notes
```

`--release` is the packaging path: without it `undra build` makes a debug core, 42 MB per ABI, which is right for
the dev loop and makes a 96 MB APK (13 MB with the release core). `app/build.gradle.kts` names the directory with
a path relative to the module (`../../build/android/jniLibs`); after every Android build `undra` checks that
line and says so if the app would not package what it just built.

The **device benchmark** (`scripts/bench-device.sh --device android`) is an instrumented test (`app/src/androidTest`) over the
`benchmark` build type (`./gradlew :app:assembleBenchmark :app:assembleBenchmarkAndroidTest`: release, not debuggable, signed
with the debug key); its runner is `app/src/main/kotlin/dev/undra/playground/bench/BenchRunner.kt`, and `UndraApp` records how
long the first `UndraPlaygroundCore.load` took for the cold-start row.

The Gradle project includes the Kotlin runtime from this checkout (`includeBuild`) and the generated bindings
as the `:core-bindings` module, so a change to either shows up in the next build.

## Against `undra dev`

Debug builds can run against the core `undra dev` serves instead of the one in the APK: a Rust change needs no
rebuild of the app, and the native build (`undra build --platform android`) is not needed at all.

```sh
undra dev -C .. --android                                              # prints the addresses, runs adb reverse for attached devices
./gradlew :app:installDebug
adb shell am start -n dev.undra.playground/.MainActivity --es undra_dev_url ws://10.0.2.2:7443    # emulator
adb shell am start -n dev.undra.playground/.MainActivity --es undra_dev_url ws://127.0.0.1:7443   # USB device, after adb reverse
./gradlew -PundraDevUrl=ws://10.0.2.2:7443 :app:installDebug           # or bake the URL into the debug build
```

The launch extra wins over the build property; neither does anything in a release build. `10.0.2.2` is the
emulator's name for your computer; a USB device uses `adb reverse` and `127.0.0.1`. The debug build type has what
the connection needs and release does not: `app/src/debug/AndroidManifest.xml` (`usesCleartextTraffic`) and the
`UNDRA_DEV_URL` `BuildConfig` field. A bar above the screens shows the connection
(`core.connectionState`, a `StateFlow`): green, amber while the runtime reconnects, red when it is over. If the dev
server cannot be reached at launch, the app says so with a Retry button.

Save a Rust change and the app is on the rebuilt core within a second, on the same screen with the same state:
`undra dev` restores the old core's state into the new one (ADR-053), the runtime reconnects and resumes its session,
and the bar says "Reloaded, state kept" (`UndraApp.devNotice`, fed by `LoadOptions.onDevNotice`). When the state could
not be carried (a schema change, a snapshot over 16 MiB) the runtime finds a new core with none of its objects
(`Closed(SESSION_LOST)`), and `UndraApp` loads it and restarts the activity on it. A dropped connection alone (the
emulator slept, adb restarted) is resumed with the same objects.

## How the app is wired

* `UndraApp` loads the core once per process (`UndraPlaygroundCore.load`, the bindings' entry, which loads
  `libplayground_core.so` and checks the schema hash of the bindings against the library's; `MainActivity` starts it, with the URL of `DevServer`, if any), calls `AndroidPlatformDefaults.install(core, this)`
  (module `android-adapters`: `Http`, `Kv`, `SecureStore`, `Fs`, `Connectivity` and `Lifecycle` are all the real ones, nothing is
  faked) and calls `configureRemote`. The persisted query cache and the offline queue live in the app's files, so they survive
  the process being killed. Against `undra dev` the same adapters stay on the device (the core is on the laptop and calls
  them over the connection), and a rebuilt core gets a fresh `install`.
* `remote/DemoServer.kt` is the server of the Remote tab: a small HTTP server on the device's loopback interface (the
  playground has no backend to ship), three seeded items, 300 ms of latency, JSON by hand with `org.json`. The core reaches
  it through the real `Http` adapter and a real socket. Like a host on the internet it is reachable only while the device
  has a network: while the Offline switch is on, or in airplane mode, it drops the connection, and the core sees
  `HttpError.Network`. The switch also tells the core through `ConnectivityEvents` (a simulated outage); airplane mode needs
  no switch, the Connectivity adapter reports it. Every request is logged under the tag `UndraDemoServer`.
* The manifest declares `INTERNET` and `ACCESS_NETWORK_STATE` (the adapters need them; `android-adapters` declares the same
  two) and `res/xml/network_security_config.xml` allows cleartext traffic to `127.0.0.1` and `localhost` only.
* Controls carry test tags (`todo-add`, `counter-inc`, `biglist-count`, `remote-offline`, ...) that are also
  resource ids (`testTagsAsResourceId`), so `uiautomator dump` and UI tests can find them.

## Smoke test

```sh
ANDROID_SERIAL=emulator-5554 ./smoke.sh      # SKIP_CORE=1 reuses build/android/jniLibs
```

Builds the core and the app, installs it, launches every tab and screenshots it, then drives the Remote tab's offline story
with `uiautomator`: fetch (persisted to `files/undra/playground_core/kv`), Offline switch on, add an item (queued), kill the process, real
airplane mode on, relaunch (the cached list comes from `Kv` while offline, the core holds the queue because the Connectivity
adapter says there is no network), airplane mode off (the core replays the queue; the server receives the
`Idempotency-Key` generated before the kill). Output: `.proof/android-adapters-smoke.log` and `.proof/android-adapters-*.png`.
