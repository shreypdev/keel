# Playground, Android

The Compose app of the playground: four screens over the one Rust core (`../core`), through the Kotlin
bindings `undra bindgen` generated (`../generated/kotlin`, package `dev.undra.playground.core`).

| Tab | Store | What it shows |
|---|---|---|
| Todos | `Todos` | a keyed list, a filter chip row and two computed values (`visible`, `remaining`); `add` fails with the typed `TodoError.EmptyTitle` |
| Counter | `Counter` | one tap is one transaction: `count`, `changes` and the computed `parity` arrive in one change-set |
| 10k list | `BigList` | 10,000 keyed rows in a `LazyColumn`; insert, update, move and remove cross as one-operation patches; "Stream updates" sends ten a second |
| Remote | `RemoteTodosQueryHandle` | a cached server list (`inbox`) with status, fetching, updated-at and error; optimistic add and toggle; an Offline switch that queues your additions and replays them |

Every screen reads its store with `collectAsState()` on the store's `StateFlow`s. A `ViewModel` owns each
store, so the state (which lives in the core) survives switching tabs and rotating the device.

## Build and run

```sh
undra build -C .. --platform android --release   # ../build/android/jniLibs/<abi>/libundra_core.so (1.5 MB each)
./gradlew :app:assembleDebug                    # app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.undra.playground/.MainActivity --es tab remote   # todos | counter | biglist | remote
```

`--release` is the packaging path: without it `undra build` makes a debug core, 42 MB per ABI, which is right for
the dev loop and makes a 96 MB APK (13 MB with the release core). `app/build.gradle.kts` names the directory with
a path relative to the module (`../../build/android/jniLibs`); after every Android build `undra` checks that
line and says so if the app would not package what it just built.

The **device benchmark** (`scripts/bench-device.sh --device android`) is an instrumented test (`app/src/androidTest`) over the
`benchmark` build type (`./gradlew :app:assembleBenchmark :app:assembleBenchmarkAndroidTest`: release, not debuggable, signed
with the debug key); its runner is `app/src/main/kotlin/dev/undra/playground/bench/BenchRunner.kt`, and `UndraApp` records how
long the first `UndraCore.load` took for the cold-start row.

The Gradle project includes the Kotlin runtime from this checkout (`includeBuild`) and the generated bindings
as the `:core-bindings` module, so a change to either shows up in the next build.

## How the app is wired

* `UndraApp` loads the core once (`UndraCore.load`, which checks the schema hash of the bindings against the
  library's), supplies the two ports Android does not default (`Http`, `Kv`), and calls `configureRemote`.
* `remote/DemoServer.kt` is the server of the Remote tab: in memory, three seeded items, 300 ms of latency,
  JSON by hand with `org.json`. With the Offline switch on it fails every request with `HttpError.Network` and the
  screen tells the core through `ConnectivityEvents`; switching back sends "online", which replays the queue.
* Controls carry test tags (`todo-add`, `counter-inc`, `biglist-count`, `remote-offline`, ...) that are also
  resource ids (`testTagsAsResourceId`), so `uiautomator dump` and UI tests can find them.
