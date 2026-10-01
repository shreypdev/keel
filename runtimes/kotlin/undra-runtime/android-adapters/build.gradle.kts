import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// The Android half of the Kotlin runtime: code that needs Android APIs, which :runtime must not reference
// (it stays stdlib + kotlinx-coroutines). Today the Choreographer frame pacer of ADR-031.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "dev.undra.android"
    compileSdk = 35

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }
}

kotlin {
    // As in :runtime: every public declaration states its visibility and type.
    explicitApi()
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_11)
        allWarningsAsErrors.set(true)
    }
}

dependencies {
    api(project(":runtime"))
}
