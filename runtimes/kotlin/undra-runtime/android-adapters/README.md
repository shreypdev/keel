# android-adapters

The `:android-adapters` Gradle module of the Kotlin runtime (SPEC section 11): an Android library that depends on
`:runtime` (never the reverse), so `:runtime` stays plain JVM, stdlib + kotlinx-coroutines only. It holds everything
that needs an Android API: the platform adapters of the ten standard ports (SPEC section 8) and the Choreographer
frame pacer. **No third-party dependency**: the Android SDK and `:runtime` are all it uses.

`settings.gradle.kts` includes it only when an Android SDK is found (`ANDROID_HOME`, `ANDROID_SDK_ROOT`, or `sdk.dir`
in `local.properties`), so a JVM-only checkout still builds `:runtime`. Its coordinates are
`dev.undra:android-adapters:0.1.0-SNAPSHOT` (a composite build resolves them to this module, as the playground app
does). minSdk 26.

## The one call

```kotlin
class MyApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // UndraPlaygroundCore: the generated entry of your core's bindings, `Undra<Namespace>` (ADR-044).
        val core = UndraPlaygroundCore.load(
            LoadOptions(mirror = MirrorOptions(framePacer = ChoreographerFramePacer())), // drains at the display's frames (ADR-031)
        )
        AndroidPlatformDefaults.install(core, this)
    }
}
```

`AndroidPlatformDefaults.install(core, context)` registers the adapter of every standard port with the core and starts
reporting `Connectivity` and `Lifecycle` events, mirroring the Swift `Adapters.platformDefault`. Call it once, in
`Application.onCreate`, right after the core's `load` and before any store is created. (The core reads its persisted
query cache and offline queue through `Kv` while it starts and waits up to five seconds for the adapter to appear, which
is why installing after `load` is enough.) It returns an `AndroidPlatform` holding the adapters; `close()` stops the event
sources. To change one port, register another implementation afterwards: `core.registerPort(StandardPorts.Http.PORT_ID, impl)`
replaces what `install` registered. `install(core, context, http = AndroidHttpAdapter(connectTimeoutMs = ...))` changes
the HTTP limits; `requireValidatedNetwork = true` makes the Connectivity adapter wait for Android's own reachability check.

Install before the first activity starts: in `Application.onCreate`, or in the first activity's `onCreate` when the core is
loaded there (the playground and the `undra init` template do, so that a debug build can choose between the in-process core
and `undra dev`). The Lifecycle adapter counts started and resumed activities from the moment it is installed; installed
later (a Retry button, say) it starts from the process importance and counts from zero, so a dialog over the app can read as
`Background` instead of `Inactive` until the next activity change.

**Against `undra dev` (`Mode.REMOTE`)** the adapters stay on the device and the core, on your computer, calls them over the
connection; install after the core's `load` as usual. A core that `undra dev` replaced (`Closed(SESSION_LOST)`) is a new core
that needs its own `install`: `close()` the previous `AndroidPlatform` first, so its callbacks stop reporting to a core that is
gone (the playground's `UndraApp.load` does). `Connectivity` and `Lifecycle` reports made while the connection is down are
dropped with a log line, so the dev server's core can hold an older state until the next change.

Without `install`, an Android core has only Clock, Rng, Log and Timer: no network, no storage, no connectivity. `Http`
calls fail with `Network("the Http port has no adapter registered")` and the query layer's persistence and offline queue do
nothing.

## What is installed

| Port | Adapter | Android API | Notes |
|---|---|---|---|
| `Http` | `AndroidHttpAdapter` | `HttpURLConnection` on `Dispatchers.IO` | Never on the calling thread. Typed outcomes (ADR-025): `InvalidUrl`, `Timeout`, `Cancelled`, `Network`. `timeoutMs` bounds the whole exchange, body included. **Cancelling the caller closes the socket.** Redirects are followed by the adapter (the rules of `java.net.http`, never https to http, no credentials to another origin). Response bodies are read in chunks and capped (64 MiB by default); no cookie jar, no HTTP cache. |
| `Kv` | `AndroidKvAdapter` | files under `filesDir/undra/kv` | One file per key (named by the SHA-256 of the key, holding `key, value`), written atomically, so a killed process leaves the old or the new value. Backed up with the app's data. |
| `SecureStore` | `AndroidSecureStoreAdapter` | `AndroidKeyStore` AES-256-GCM, files under `noBackupFilesDir/undra/secure` | The key never leaves the Keystore (hardware-backed where available). Values are sealed as `format, iv, ciphertext+tag` with the key name as authenticated data (the web adapter's layout). Not backed up (a ciphertext could not be opened on another device). A failing Keystore or a damaged file is an error, never "missing". |
| `Fs` | `AndroidFsAdapter` | files under `filesDir/undra/fs` | Confined to its root: `..` and symbolic links out are `Denied`. `write` creates directories and is atomic; `delete` removes a directory with its contents (like Swift and the web), never the root, and removes a symbolic link without following it. A NUL byte in a path is `Io`. |
| `Connectivity` | `AndroidConnectivityAdapter` | `ConnectivityManager.registerDefaultNetworkCallback` | Reports the current state at once, then every change (Wi-Fi to cellular, loss, return). Online = the default network provides internet (`NET_CAPABILITY_INTERNET`), like `NWPath.satisfied`; kind = Wi-Fi, cellular, wired, unknown or none. Reports come from a handler thread, never the main thread. |
| `Lifecycle` | `AndroidLifecycleAdapter` | `Application.ActivityLifecycleCallbacks` | `Active` (an activity resumed), `Inactive` (visible, no focus), `Background` (none started), on the main thread. A move to a less active state is reported after 700 ms if it lasted, so a rotation or one activity giving way to another is not a trip to the background. There is no "terminate" event: `AppState` has none and Android ends a process without notice; the queue and the cache are persisted as they change. `Active` makes the core refetch stale queries (SPEC section 9). |
| `Log` | `AndroidLogAdapter` | `android.util.Log` | The record's target is the tag; levels trace..fatal map to verbose..assert (fatal is written with `Log.println`, never `Log.wtf`). Unlike `java.util.logging`, which the runtime's default adapter uses, it keeps trace and debug records. |
| `Clock`, `Rng`, `Timer` | the runtime's own (`ClockAdapter`, `RngAdapter`, `TimerAdapter`) | `System`, `SecureRandom`, a scheduled executor | Plain JVM; `install` registers them too, so one call covers all ten. |

Each adapter is usable alone: `AndroidKvAdapter(context).portImpl()` is a `PortImpl` for `LoadOptions.adapters` or
`core.registerPort`; `AndroidConnectivityAdapter(context).attach(core)` starts one event source.

**Why no `androidx` dependency.** `androidx.security:security-crypto` (`EncryptedFile`) is deprecated and pulls in a large
cryptography library for what is about eighty lines of `javax.crypto` and `AndroidKeyStore` here. `ProcessLifecycleOwner`
(`lifecycle-process`) would add AndroidX to every app for a dozen lines of activity counting. OkHttp would be a second
HTTP stack next to the platform's; `HttpURLConnection` on Android *is* OkHttp.

## Permissions

| Permission | Needed by | Declared by |
|---|---|---|
| `android.permission.INTERNET` | `AndroidHttpAdapter` (opening sockets) | this library's manifest (it merges into the app's) and the `undra init` Android template, explicitly |
| `android.permission.ACCESS_NETWORK_STATE` | `AndroidConnectivityAdapter` (`ConnectivityManager`) | this library's manifest and the template, explicitly |

Without `ACCESS_NETWORK_STATE` the Connectivity adapter logs a warning and reports nothing (the core assumes the network is
up); without `INTERNET` requests fail with a `Network` error (its message suggests declaring the permission when the platform reports a permission failure).

**Cleartext.** Since Android 9 `http://` traffic is blocked unless the app's network security config allows the host; the
failure is a typed `Network` error naming the policy. The playground allows only its own loopback server
(`res/xml/network_security_config.xml`). `https://` needs nothing.

## Tests

* **JVM unit tests** (`./gradlew :android-adapters:test`): the pure logic (URL and header rules, error mapping, redirect rules,
  secret sealing, network classification, the lifecycle state machine, log levels), plus the Http, Kv, Fs and SecureStore
  adapters against a local server and temporary directories (`src/test`, `src/sharedTest`).
* **Instrumented tests** (`./gradlew :android-adapters:connectedAndroidTest`, on a booted emulator or device; set
  `ANDROID_SERIAL` when several are attached): the shared tests again on Android's own `HttpURLConnection`, file system and
  Keystore, and the device-only ones in `src/androidTest`: no plaintext secret in any file or `SharedPreferences`, a secret
  and a Kv entry that survive a process killed with SIGKILL (the service in `:writer`), a Kv write interrupted by that kill,
  the default network read from `ConnectivityManager`, lifecycle states from real activities (`ActivityScenario`, Home),
  every port of `AndroidPlatformDefaults.install` answering through its port methods, the Http adapter off the main thread.
  `-Pandroid.testInstrumentationRunnerArguments.undra.networkToggle=true` also runs a test that switches the device's
  Wi-Fi and data off and on to see the Connectivity events; it changes the whole device, so it is off by default.
* **The playground** (`examples/playground/android/smoke.sh`): the app on the real adapters, including a queued mutation that
  survives a killed process and is replayed after real airplane mode ends.

## What the runtime already does for Android

* No Android API is referenced at compile time in `:runtime`; `android.os.Looper` is found by reflection (once, for the main thread).
* `java.lang.ref.Cleaner` (Android 13+) is optional: a phantom-reference queue with one daemon thread stands in below API 33.
* With `LoadOptions.defaultAdapters` on Android, only the portable adapters (Clock, Rng, Log, Timer) are installed; the classes that
  need `java.net.http` are never loaded. `Mode.REMOTE` (`undra dev`) works: the runtime has its own WebSocket client.

## Still to come

* **Packaging the core.** `lib<namespace>.so` in `jniLibs/<abi>/` stays with the app's Gradle project (16 KB page alignment,
  NDK r27). Loading it is not this module's job: the generated `UndraCoreNative` of the core's bindings loads it
  (`NativeLibrary.load(namespace)`), and the R8 rules that keep what JNI finds by name ship with the code that declares it:
  `META-INF/proguard/undra-runtime.pro` in the runtime jar (`NativeCallbacks`), `META-INF/proguard/undra-<namespace>.pro` with
  the bindings (`UndraCoreNative`) (ADR-044).
* A background-execution integration (`WorkManager`) for draining the offline queue while the app is not running.
