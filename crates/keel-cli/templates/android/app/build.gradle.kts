plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "@@APP_ID@@"
    compileSdk = 35

    defaultConfig {
        applicationId = "@@APP_ID@@"
        minSdk = @@MIN_SDK@@
        targetSdk = 35
        versionCode = 1
        versionName = "1.0"
        ndk {
            // The ABIs `keel build --platform android` produces (keel.toml [android] abis).
            abiFilters += listOf(@@ABI_FILTERS@@)
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }

    buildFeatures {
        compose = true
    }

    sourceSets {
        // `keel build --platform android` writes libkeel_core.so for every ABI here. The Kotlin
        // runtime loads it with System.loadLibrary("keel_core").
        getByName("main").jniLibs.srcDir("@@JNI_LIBS_PATH@@")
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_11)
    }
}

dependencies {
    // The Keel runtime and the bindings generated from the core.
    implementation("dev.keel:runtime:@@KOTLIN_RUNTIME_VERSION@@")
    implementation(project(":core-bindings"))

    implementation(platform("androidx.compose:compose-bom:2024.10.01"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.3")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
