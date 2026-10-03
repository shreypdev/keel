import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// The optional Compose half of the Kotlin runtime (ADR-043): `LazyListScope.items(list)` draws a lazily paged
// UndraLazyList in a LazyColumn, and `LazyListState.LoadMoreWhenNearEnd(query)` fetches the next page of an
// infinite query when the list is scrolled near its end. The one module that depends on Compose: :runtime stays
// stdlib + kotlinx-coroutines, and an app that does not draw lists with Compose never pulls it in. Depends on
// compose-foundation (LazyListScope, public API) and compose-runtime (snapshot state); no other Compose artifact.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    `maven-publish`
}

android {
    namespace = "dev.undra.compose"
    compileSdk = 35

    defaultConfig {
        minSdk = 26
        // The instrumented tests (src/androidTest) draw real lazy lists on an emulator or device:
        // ./gradlew :undra-compose:connectedDebugAndroidTest
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
    // UndraLazyList and InfiniteQuery are in the public signatures.
    api(project(":runtime"))
    // The BOM picks the matching versions of the two Compose artifacts; an app's own BOM overrides it.
    api(platform(libs.androidx.compose.bom))
    api(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.runtime)

    testImplementation(libs.junit4)

    androidTestImplementation(platform(libs.androidx.compose.bom))
    androidTestImplementation(libs.junit4)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.ext.junit)
    androidTestImplementation(libs.androidx.compose.ui.test.junit4)
    // The activity the Compose test rule launches.
    debugImplementation(libs.androidx.compose.ui.test.manifest)
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
