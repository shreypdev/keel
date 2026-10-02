// The Android project of fieldbook. Open this directory in Android Studio, or `./gradlew :app:assembleDebug`.

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

rootProject.name = "fieldbook"

include(":app")

// The generated Kotlin bindings (`undra bindgen`): a plain JVM module the app depends on.
include(":core-bindings")
project(":core-bindings").projectDir = file("../generated/kotlin")

// The Kotlin runtime, built from the Undra checkout (a composite build: `dev.undra:runtime` below
// resolves to it, so runtime edits show up without publishing).
includeBuild("../../../runtimes/kotlin/undra-runtime")

