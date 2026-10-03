plugins {
    alias(libs.plugins.kotlin.jvm) apply false
    // Declared here, not in :android-adapters alone, so the Kotlin plugin (loaded by this root project) and the
    // Android plugin share one class loader. Only resolved, never applied, when no Android SDK is around.
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
}

allprojects {
    // The repository's own identity: what a checkout's composite build substitutes (`includeBuild`), and the Maven Central
    // name for later. A release is published by JitPack from its tag with the group and version JitPack asks for
    // (`com.github.shreypdev.undra`, `v1.0.0`): jitpack.yml at the repository root runs scripts/jitpack-install.sh, which
    // passes them as -PundraGroup and -PundraVersion (ADR-063).
    group = providers.gradleProperty("undraGroup").getOrElse("dev.undra")
    version = providers.gradleProperty("undraVersion").getOrElse("0.1.0-SNAPSHOT")
}
