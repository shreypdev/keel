plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.undra.twocores"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.undra.twocores"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "1.0"
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }

    sourceSets {
        // Each core's libraries, side by side in one APK: lib/<abi>/libplayground_a.so and libplayground_b.so.
        getByName("main").jniLibs.srcDirs("../../a/build/android/jniLibs", "../../b/build/android/jniLibs")
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_11)
    }
}

dependencies {
    implementation("dev.undra:runtime:0.1.0-SNAPSHOT")
    // The Android half of the runtime: the main thread is Android's (read-your-writes on it) and the
    // Choreographer frame pacer (ADR-031).
    implementation("dev.undra:android-adapters:0.1.0-SNAPSHOT")
    // Dispatchers.Main: what makes the Android main thread the runtime's main thread (a Compose or
    // lifecycle app has it already, through AndroidX).
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
    implementation(project(":core-a"))
    implementation(project(":core-b"))
}
