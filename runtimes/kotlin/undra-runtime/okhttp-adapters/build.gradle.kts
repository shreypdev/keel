import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// The optional OkHttp half of the Kotlin runtime (ADR-060): the Http, WebSocket and Sse ports over the app's own OkHttpClient, so
// that its interceptors, authenticator (token refresh), event listeners (tracing) and certificate pinner apply to the core's
// traffic with nothing Undra-specific to configure. OkHttp is a dependency :runtime and :android-adapters must not carry (an app
// that does not use OkHttp would get a second HTTP stack), so it lives here, and an app that wants its stack inside adds this
// module. Kotlin stdlib, kotlinx-coroutines and okhttp3 are all it uses, besides :android-adapters (and :runtime through it).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    `maven-publish`
}

android {
    namespace = "dev.undra.okhttp"
    compileSdk = 35

    defaultConfig {
        minSdk = 26
        // The instrumented tests (src/androidTest) run on an emulator or device: ./gradlew :okhttp-adapters:connectedAndroidTest
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }

    // The variant a release publishes, with its sources (ADR-063).
    publishing {
        singleVariant("release") {
            withSourcesJar()
        }
    }

    sourceSets {
        // Tests that need no Android API run twice: as JVM unit tests (fast) and on the device. They live in src/sharedTest.
        getByName("test").java.srcDir("src/sharedTest/kotlin")
        getByName("androidTest").java.srcDir("src/sharedTest/kotlin")
        // The suites every adapter of the Kotlin runtime meets, written once and run here again on OkHttp (ADR-060): the Http contract
        // and the loopback server it runs against (../adapter-contracts, shared with :android-adapters), and the Suite runner, the
        // realtime test server and the WebSocket/Sse contract (../test-support, shared with :runtime). Each runs on the desktop JVM
        // (unit tests) and on the device (instrumented tests).
        getByName("test").java.srcDir("../adapter-contracts/kotlin")
        getByName("androidTest").java.srcDir("../adapter-contracts/kotlin")
        getByName("test").java.srcDir("../test-support/kotlin")
        getByName("androidTest").java.srcDir("../test-support/kotlin")
        // What needs the instrumentation registry: the contract's device half (the realtime server on the host).
        getByName("androidTest").java.srcDir("../adapter-contracts/device")
    }

    testOptions {
        // The JVM unit tests (src/test) test the adapters against loopback servers; a stray call into android.jar returns a default.
        unitTests.isReturnDefaultValues = true
    }
}

kotlin {
    // As in :runtime: every public declaration states its visibility and type.
    explicitApi()
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_11)
        allWarningsAsErrors.set(true)
        // SseStreamReader and ReadAheadSource are the runtime's building blocks for adapter authors (ADR-060): an adapter over a
        // blocking stream reads at the pace of the binding's window with them instead of writing the parser and the gate again.
        freeCompilerArgs.add("-opt-in=dev.undra.runtime.UndraEmbeddingApi")
    }
}

dependencies {
    // AndroidPlatformDefaults is what installWithOkHttp extends, and its AndroidPlatform is part of this module's API.
    api(project(":android-adapters"))
    // OkHttpClient is in every public signature, so apps see it.
    api(libs.okhttp)

    testImplementation(libs.junit4)

    androidTestImplementation(libs.junit4)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.core)
    androidTestImplementation(libs.androidx.test.ext.junit)
}

// ADR-063: the release variant is published at every release (JitPack builds the tag; scripts/jitpack-install.sh). The
// component exists once the Android plugin has configured the variants, hence afterEvaluate.
afterEvaluate {
    publishing {
        publications {
            create<MavenPublication>("release") {
                from(components["release"])
            }
        }
    }
}
