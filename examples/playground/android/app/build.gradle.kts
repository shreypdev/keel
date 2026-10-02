plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dev.undra.playground"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.undra.playground"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "1.0"
        // The device benchmark (`scripts/bench-device.sh --device android`) runs as an instrumented test.
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            // The ABIs `undra build --platform android` produces (undra.toml [android] abis).
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        debug {
            // The `undra dev` server a debug build runs against instead of the in-process core, chosen when the app is
            // built: `./gradlew -PundraDevUrl=ws://10.0.2.2:7443 :app:installDebug` (10.0.2.2 is the emulator's name for
            // this machine; a USB device uses `adb reverse tcp:7443 tcp:7443` and ws://127.0.0.1:7443). Empty, the
            // default, keeps the in-process core. A launch extra (`--es undra_dev_url ...`) overrides it at run time.
            buildConfigField("String", "UNDRA_DEV_URL", "\"${providers.gradleProperty("undraDevUrl").getOrElse("")}\"")
        }
        release {
            // Release builds never talk to a dev server: no URL, and no cleartext traffic beyond the loopback demo server
            // (src/debug/AndroidManifest.xml is not part of them). The INTERNET permission is in the main manifest.
            buildConfigField("String", "UNDRA_DEV_URL", "\"\"")
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
        // What the device benchmark measures: the release build (not debuggable, optimised) signed with the debug key so
        // that it installs. `./gradlew :app:assembleBenchmark :app:assembleBenchmarkAndroidTest`.
        create("benchmark") {
            initWith(getByName("release"))
            signingConfig = signingConfigs.getByName("debug")
            matchingFallbacks += listOf("release")
            isDebuggable = false
        }
    }
    // The instrumented tests (the device benchmark) run against that build, not the debuggable one.
    testBuildType = "benchmark"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    sourceSets {
        // The recordings the Compose previews play (testkit/fixtures at the root of the repository; debug builds only).
        getByName("debug").assets.srcDir("../../../../testkit/fixtures")
        // `undra build --platform android --release` writes libplayground_core.so for every ABI here.
        // The bindings load it with System.loadLibrary("playground_core"). Like any path in this file
        // it is relative to this module (android/app), not to android/; Gradle ignores a
        // directory that does not exist, so `undra build` checks this line after building.
        getByName("main").jniLibs.srcDir("../../build/android/jniLibs")
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_11)
    }
}

dependencies {
    // The Undra runtime and the bindings generated from the core.
    implementation("dev.undra:runtime:0.1.0-SNAPSHOT")
    // The Android half of the runtime: the Choreographer frame pacer (ADR-031).
    implementation("dev.undra:android-adapters:0.1.0-SNAPSHOT")
    // The optional WorkManager module (ADR-046): replays the offline queue while the app is in the background.
    implementation("dev.undra:android-work:0.1.0-SNAPSHOT")
    // The optional Compose half (ADR-043): `items(list)` for a lazily paged list and `LoadMoreWhenNearEnd` for an infinite query.
    implementation("dev.undra:undra-compose:0.1.0-SNAPSHOT")
    implementation(project(":core-bindings"))

    implementation(platform("androidx.compose:compose-bom:2024.10.01"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    debugImplementation("androidx.compose.ui:ui-tooling")
    // The testing kit (docs/TESTING.md): the Compose previews play a recorded session. Debug builds only.
    debugImplementation("dev.undra:testkit:0.1.0-SNAPSHOT")

    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
}
