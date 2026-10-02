import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// The optional WorkManager half of the Kotlin runtime (ADR-046, decision 3.4): UndraWorker runs the core's background
// tasks in a window the OS grants, and UndraWork schedules it. WorkManager is a dependency the base runtime and
// :android-adapters must not carry, so it lives here, and an app that wants background drains adds this module.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "dev.undra.work"
    compileSdk = 35

    defaultConfig {
        minSdk = 26
        // The instrumented tests (src/androidTest) run on an emulator or device: ./gradlew :android-work:connectedAndroidTest
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }

    testOptions {
        // The JVM unit tests (src/test) test the pure logic of a background window; WorkManager itself is tested on the device.
        unitTests.isReturnDefaultValues = true
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
    // UndraCore.runInBackground and UndraStats.background are part of the runtime's API: apps of this module see them.
    api(project(":runtime"))
    api(libs.androidx.work.runtime.ktx)

    testImplementation(libs.junit4)

    androidTestImplementation(libs.junit4)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.core)
    androidTestImplementation(libs.androidx.test.ext.junit)
    androidTestImplementation(libs.androidx.work.testing)
}
