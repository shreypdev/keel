# android-adapters (not written yet)

This directory reserves the place of the `:android-adapters` Gradle module (SPEC section 11). It will depend on `:runtime`; `:runtime`
never depends on it. Nothing here compiles yet: `settings.gradle.kts` does not include it.

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

* **A one-call installer** (`UndraAndroid.load(context, expectedSchemaHash)`) that builds the adapters from an `Application` context and calls
  `UndraCore.load`. Main-thread delivery needs nothing here: `UndraDispatchers.main` already uses `Dispatchers.Main.immediate` when
  it finds Android's `Looper`.

## What the runtime already does for Android

* No Android API is referenced at compile time; `android.os.Looper` is found by reflection.
* `java.lang.ref.Cleaner` (Android 13+) is optional: a phantom-reference queue with one daemon thread stands in below API 33.
* With `LoadOptions.defaultAdapters` on Android, only the portable adapters (Clock, Rng, Log, Timer) are installed; the classes that need
  `java.net.http` are never loaded. `Mode.REMOTE` fails with an `UndraModeException` because the JDK WebSocket is missing.
