// The two-core test app on Android (ADR-044): one APK with two Undra cores, `libplayground_a.so` and
// `libplayground_b.so` (the playground core under two namespaces, ../a and ../b), each with its own
// generated bindings. `./run.sh` builds both cores, installs the app and reads its log lines.

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

rootProject.name = "two-cores"

include(":app")

// The generated Kotlin bindings of each core (`undra bindgen`), plain JVM modules the app depends on.
include(":core-a")
project(":core-a").projectDir = file("../a/generated/kotlin")
include(":core-b")
project(":core-b").projectDir = file("../b/generated/kotlin")

// The Kotlin runtime, built from this checkout (a composite build).
includeBuild("../../../runtimes/kotlin/undra-runtime")
