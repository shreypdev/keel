// The Android project of the playground. Open this directory in Android Studio, or run
// `./gradlew :app:assembleDebug` (after `undra build -C .. --platform android`).

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "playground"

include(":app")

// The generated Kotlin bindings (`undra bindgen`): a plain JVM module the app depends on.
include(":core-bindings")
project(":core-bindings").projectDir = file("../generated/kotlin")

// The Kotlin runtime, built from this checkout (a composite build: `dev.undra:runtime` in
// app/build.gradle.kts resolves to it, so runtime edits show up without publishing).
includeBuild("../../../runtimes/kotlin/undra-runtime")
