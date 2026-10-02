plugins {
    alias(libs.plugins.kotlin.jvm) apply false
    // Declared here, not in :android-adapters alone, so the Kotlin plugin (loaded by this root project) and the
    // Android plugin share one class loader. Only resolved, never applied, when no Android SDK is around.
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
}

allprojects {
    group = "dev.undra"
    version = "0.1.0-SNAPSHOT"
}
