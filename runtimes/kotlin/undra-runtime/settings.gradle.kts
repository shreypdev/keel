// Undra Kotlin runtime (SPEC §11). Kotlin stdlib + kotlinx-coroutines only.
//
//   :runtime           pure-JVM runtime: wire layer, transports, mirror, codecs. No Android APIs.
//   :android-adapters  (future, not created yet) Android-specific code: Dispatchers.Main wiring, the
//                      Application-context Kv / SecureStore / Http / Connectivity / Lifecycle port
//                      adapters, and the JNI loader for libundra. Depends on :runtime, never the reverse.

pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        mavenCentral()
    }
}

rootProject.name = "undra-runtime"

include(":runtime")
