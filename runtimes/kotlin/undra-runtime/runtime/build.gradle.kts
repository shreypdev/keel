import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import org.jetbrains.kotlin.gradle.tasks.KotlinCompile

plugins {
    alias(libs.plugins.kotlin.jvm)
}

// Android consumers get java.time through core-library desugaring; the runtime itself only needs
// JVM 11 bytecode and never touches Android APIs (those live in the :android-adapters module).
java {
    sourceCompatibility = JavaVersion.VERSION_11
    targetCompatibility = JavaVersion.VERSION_11
}

kotlin {
    // Every public declaration needs an explicit visibility and type: this is a library other teams
    // trust blindly, so accidental API surface is a build error.
    explicitApi()
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_11)
    }
}

tasks.named<KotlinCompile>("compileKotlin") {
    compilerOptions {
        allWarningsAsErrors.set(true)
    }
}

dependencies {
    api(libs.kotlinx.coroutines.core)

    testImplementation(platform(libs.junit.bom))
    testImplementation(libs.junit.jupiter)
    testRuntimeOnly(libs.junit.platform.launcher)
}

// Test support shared with android-adapters' tests (FaultyFileSystem: a file system that fails on demand, ADR-049).
kotlin.sourceSets.named("test") {
    kotlin.srcDir(rootProject.projectDir.resolve("test-support/kotlin"))
}

// The generated Kotlin of bindgen's `full` golden case and its execution test are compiled and run against this
// runtime as part of the tests (GoldenFullTests). Skipped when the repository layout is not around.
val bindgenTests = rootProject.projectDir.resolve("../../../crates/undra-bindgen/tests")
val goldenFull = bindgenTests.resolve("golden/full/kotlin/src/main/kotlin")
val goldenFullRun = bindgenTests.resolve("fixtures/kotlin-run/full")
if (goldenFull.isDirectory && goldenFullRun.isDirectory) {
    kotlin.sourceSets.named("test") {
        kotlin.srcDir(goldenFull)
        kotlin.srcDir(goldenFullRun)
    }
}

tasks.test {
    useJUnitPlatform()
    // For the JNI smoke test: -Pundra.native.dir=target/debug [-Pundra.native.name=undra_ffi]
    (findProperty("undra.native.dir") as String?)?.let { systemProperty("java.library.path", it) }
    (findProperty("undra.native.name") as String?)?.let { systemProperty("undra.native.name", it) }
    (findProperty("undra.native.path") as String?)?.let { systemProperty("undra.native.path", it) }
    testLogging {
        events("failed")
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
        showStandardStreams = true
    }
}
