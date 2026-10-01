# android-adapters

The `:android-adapters` Gradle module of the Kotlin runtime (SPEC section 11): an Android library that
depends on `:runtime` (never the reverse), so `:runtime` stays plain JVM, stdlib + kotlinx-coroutines only.
`settings.gradle.kts` includes it only when an Android SDK is found (`ANDROID_HOME`, `ANDROID_SDK_ROOT`, or
`sdk.dir` in `local.properties`), so a JVM-only checkout still builds `:runtime`; its coordinates are
`dev.undra:android-adapters:0.1.0-SNAPSHOT` (a composite build resolves them to this module, as the
playground app does).

## What it holds

* **`ChoreographerFramePacer`** (`dev.undra.android`): paces the mirror's drains by the display
  (ADR-031). Pass it when loading the core:

  ```kotlin
  UndraCore.load(LoadOptions(expectedSchemaHash = UndraIds.SCHEMA_HASH, mirror = MirrorOptions(framePacer = ChoreographerFramePacer())))
  ```

  Without it the runtime drains on a 60 Hz grid of its own (`undra-frame`), not aligned with the display.

## What it will hold

* **Loading the core.** `System.loadLibrary("undra_core")` from `jniLibs/<abi>/` (arm64-v8a, x86_64; 16 KB page alignment, NDK r27), and
  the ProGuard / R8 consumer rules that keep `dev.undra.runtime.UndraNative` and `UndraNative$Callbacks` with their method names (the
  native library finds them by name and descriptor).
* **Port adapters** implementing the standard ports of SPEC section 8 over Android APIs, each as a `PortImpl` for
  `LoadOptions.adapters`, reusing the codecs in `dev.undra.runtime.adapters` (`HttpRequest`, `FsError`, ...):

  | Port | Adapter |
  |---|---|
  | `Http` | OkHttp (optional dependency) or `HttpURLConnection` (`java.net.http` does not exist on Android) |
  | `Kv` | files or `SharedPreferences` under `Context.filesDir`, same key scheme as `FileKv` |
  | `SecureStore` | `EncryptedFile` with an Android Keystore key |
  | `Fs` | `Context.filesDir`, confined like `FsAdapter` |
  | `Connectivity` | `ConnectivityManager` network callbacks feeding `ConnectivityEvents` |
  | `Lifecycle` | `ProcessLifecycleOwner` feeding `LifecycleEvents` |
  | `Timer` | `Handler` on a `HandlerThread`, calling `UndraCore.timerFired` |
  | `Clock`, `Rng`, `Log` | the portable ones from `JvmAdapters.portable`; `Log` may go to `android.util.Log` instead of `java.util.logging` |

* **A one-call installer** (`UndraAndroid.load(context, expectedSchemaHash)`) that builds the adapters from an `Application` context,
  installs `ChoreographerFramePacer` and calls `UndraCore.load`.

## What the runtime already does for Android

* No Android API is referenced at compile time; `android.os.Looper` is found by reflection (once, for the main thread).
* `java.lang.ref.Cleaner` (Android 13+) is optional: a phantom-reference queue with one daemon thread stands in below API 33.
* With `LoadOptions.defaultAdapters` on Android, only the portable adapters (Clock, Rng, Log, Timer) are installed; the classes that need
  `java.net.http` are never loaded. `Mode.REMOTE` fails with an `UndraModeException` because the JDK WebSocket is missing.
