// The Android project of @@NAME@@. Open this directory in Android Studio, or `./gradlew :app:assembleDebug`.

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

rootProject.name = "@@NAME@@"

include(":app")

// The generated Kotlin bindings (`keel bindgen`): a plain JVM module the app depends on.
include(":core-bindings")
project(":core-bindings").projectDir = file("@@GENERATED_KOTLIN_PATH@@")
@@KOTLIN_RUNTIME_BUILD@@
