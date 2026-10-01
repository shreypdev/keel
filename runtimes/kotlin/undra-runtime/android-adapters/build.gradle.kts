import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// The Android half of the Kotlin runtime: code that needs Android APIs, which :runtime must not reference
// (it stays stdlib + kotlinx-coroutines). It holds the Choreographer frame pacer of ADR-031 and the platform
// adapters of the ten standard ports (SPEC section 8): Http, Kv, SecureStore, Fs, Connectivity and Lifecycle over
// Android APIs, installed together by AndroidPlatformDefaults.install. No third-party dependency: the Android SDK
// and :runtime are all it uses.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "dev.undra.android"
    compileSdk = 35

    defaultConfig {
        minSdk = 26
        // The instrumented tests (src/androidTest) run on an emulator or device: ./gradlew :android-adapters:connectedAndroidTest
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }

    sourceSets {
        // Tests that need no Android API run twice: as JVM unit tests (fast) and on the device (where HttpURLConnection,
        // the file system and the Keystore are Android's own). They live in src/sharedTest.
        getByName("test").java.srcDir("src/sharedTest/kotlin")
        getByName("androidTest").java.srcDir("src/sharedTest/kotlin")
    }

    testOptions {
        // The JVM unit tests (src/test) test pure logic; a stray call into android.jar returns a default instead of throwing.
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
    api(project(":runtime"))

    testImplementation(libs.junit4)

    androidTestImplementation(libs.junit4)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.core)
    androidTestImplementation(libs.androidx.test.ext.junit)
}
