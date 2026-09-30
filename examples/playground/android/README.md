# Playground, Android

The Compose app of the playground: four screens over the one Rust core (`../core`), through the Kotlin
bindings `keel bindgen` generated (`../generated/kotlin`, package `dev.keel.playground.core`).

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
keel build -C .. --platform android     # ../build/android/jniLibs/<abi>/libkeel_core.so
./gradlew :app:assembleDebug            # app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.keel.playground/.MainActivity --es tab remote   # todos | counter | biglist | remote
```

The Gradle project includes the Kotlin runtime from this checkout (`includeBuild`) and the generated bindings
as the `:core-bindings` module, so a change to either shows up in the next build.

## How the app is wired

* `KeelApp` loads the core once (`KeelCore.load`, which checks the schema hash of the bindings against the
  library's), supplies the two ports Android does not default (`Http`, `Kv`), and calls `configureRemote`.
* `remote/DemoServer.kt` is the server of the Remote tab: in memory, three seeded items, 300 ms of latency,
  JSON by hand with `org.json`. With the Offline switch on it fails every request with `HttpError.Network` and the
  screen tells the core through `ConnectivityEvents`; switching back sends "online", which replays the queue.
* Controls carry test tags (`todo-add`, `counter-inc`, `biglist-count`, `remote-offline`, ...) that are also
  resource ids (`testTagsAsResourceId`), so `uiautomator dump` and UI tests can find them.
