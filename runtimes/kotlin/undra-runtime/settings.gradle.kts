// Undra Kotlin runtime (SPEC §11). Kotlin stdlib + kotlinx-coroutines only.
//
//   :runtime           pure-JVM runtime: wire layer, transports, mirror, codecs. No Android APIs.
//   :android-adapters  Android-specific code (an Android library depending on :runtime, never the reverse):
//                      the adapters of the ten standard ports and the Choreographer frame pacer of ADR-031 (see
//                      android-adapters/README.md). Included only when an
//                      Android SDK is found (ANDROID_HOME, ANDROID_SDK_ROOT, or sdk.dir in local.properties of
//                      this build or of the build that includes it), so a JVM-only checkout builds :runtime.
//   :undra-compose     The optional Compose helpers (ADR-043: `LazyListScope.items(list)` for a lazily paged list and
//                      `LazyListState.LoadMoreWhenNearEnd` for an infinite query). The only module that depends on Compose;
//                      included under the same condition as :android-adapters (see undra-compose/README.md).
//   :android-work      the optional WorkManager module (ADR-046): UndraWorker + UndraWork run the core's background
//                      tasks in a window the OS grants. Its own module because WorkManager is a dependency
//                      :runtime and :android-adapters must not carry; included under the same condition.
//   :okhttp-adapters   the optional OkHttp module (ADR-060): the Http, WebSocket and Sse ports over the app's own OkHttpClient, so its
//                      interceptors, authenticator, event listeners and certificate pinner apply to the core's traffic. Its own module
//                      because OkHttp is a dependency :runtime and :android-adapters must not carry; included under the same condition.

pluginManagement {
    repositories {
        google()
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "undra-runtime"

include(":runtime")
include(":testkit")

/** The Android SDK directory, if this machine has one Gradle can be pointed at. */
fun androidSdk(): File? {
    val fromEnv = listOf("ANDROID_HOME", "ANDROID_SDK_ROOT").mapNotNull { System.getenv(it) }.map(::File)
    // `local.properties` of this build, and of the build that includes this one (Android Studio writes sdk.dir
    // only into the project it opened, for example examples/playground/android).
    val propertyFiles = listOfNotNull(settingsDir, gradle.parent?.startParameter?.currentDir).map { File(it, "local.properties") }
    val fromProperties = propertyFiles.filter { it.isFile }.mapNotNull { file ->
        java.util.Properties().apply { file.inputStream().use { load(it) } }.getProperty("sdk.dir")?.let(::File)
    }
    return (fromEnv + fromProperties).firstOrNull { it.isDirectory }
}

if (androidSdk() != null) {
    include(":android-adapters")
    include(":undra-compose")
    include(":android-work")
    include(":okhttp-adapters")
}
