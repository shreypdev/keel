# android-work

The optional `:android-work` Gradle module of the Kotlin runtime (ADR-046, decision 3.4): background drains of the core on Android,
through WorkManager. It is a module of its own because **WorkManager is a dependency the base runtime and `:android-adapters` must
not carry**; an app that does not replay its offline queue in the background never adds it. Coordinates
`dev.undra:android-work:0.1.0-SNAPSHOT` (a composite build resolves them to this module, as the playground app does), and
`com.github.shreypdev.undra:android-work:v<version>` from JitPack at a release (ADR-063); minSdk 26;
`api` dependencies on `:runtime` and `androidx.work:work-runtime-ktx`. `settings.gradle.kts` includes it under the same condition as
`:android-adapters` (an Android SDK is found).

## What it does

`UndraWorker` is a `CoroutineWorker`: it loads the core with the application context, calls `core.runInBackground(deadlineMs)` (the
standard function `run_background`: replay the offline queue, refetch stale persisted queries, flush persistence, all together)
for 9 minutes less a 15 second margin (WorkManager stops a worker at 10), and answers WorkManager:

| The core... | The worker returns |
|---|---|
| finished every task before the deadline | `Result.success()` |
| did not finish (the deadline passed, or something still waits for the network) | `Result.retry()` |
| could not be loaded, or the run failed (closed core, refused or cancelled call) | `Result.retry()` |
| no loader is registered | `Result.failure()` (retrying cannot help) |

When WorkManager stops the worker the coroutine is cancelled and so is the call into the core: what the run already did is kept (the
offline queue persists per item, ADR-037) and WorkManager runs the job again. Nothing is thrown into WorkManager (R6).

`UndraWork.schedule(context)` enqueues the worker as **unique** work (`UndraWork.UNIQUE_WORK_NAME`) with `NetworkType.CONNECTED` and
`ExistingWorkPolicy.KEEP`: however often it is called, one job waits or runs. `UndraWork.scheduleIfPending(context, core)` does it only
when `core.stats().background.pending > 0`.

## Wiring

```kotlin
class MyApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // How a process that WorkManager starts on its own (no activity, no earlier load) gets the core. Idempotent: return the core
        // that is loaded, or load it (the generated `Undra<Namespace>.load` refuses a second load).
        UndraWork.configure(loader = { context -> (context.applicationContext as MyApp).loadedCore() })
    }

    fun install(core: UndraCore) {
        // Asks for a window when the app goes to the background with work pending (queued offline mutations, stale persisted queries).
        AndroidPlatformDefaults.install(core, this, onBackgroundWorkPending = { UndraWork.schedule(this) })
    }
}
```

When a mutation is queued offline and you want a window even if the app stays in the foreground for now, call
`UndraWork.scheduleIfPending(context)` after it. The loader is called on a WorkManager thread, once per run; a loader that throws makes
the run end with `retry`.

## Tests

* **JVM unit tests** (`./gradlew :android-work:testDebugUnitTest`): the pure logic behind the worker, `BackgroundWindow`: the deadline
  (9 minutes less a margin, less what loading used, never under a second), the mapping of the core's answer onto a result, failures never
  thrown, cancellation propagating after cancelling the call, the registered loader.
* **Instrumented tests** (`./gradlew :android-work:connectedDebugAndroidTest`, on a booted emulator or device; set `ANDROID_SERIAL`
  when several are attached): `doWork` over a fake core through WorkManager's `TestListenableWorkerBuilder` (success, retry, no loader,
  a throwing loader, a failing core, a stopped worker cancelling the run), and `UndraWork.schedule` against WorkManager's test driver:
  one unique job with the network constraint, run once its constraints are met, ended on `finished` and retried otherwise.
