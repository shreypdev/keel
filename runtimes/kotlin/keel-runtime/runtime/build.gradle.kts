import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import org.jetbrains.kotlin.gradle.tasks.KotlinCompile

plugins {
    alias(libs.plugins.kotlin.jvm)
}

// Android consumers get java.time through core-library desugaring; the runtime itself only needs
// JVM 11 bytecode and never touches Android APIs (those live in the future :android-adapters module).
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

tasks.test {
    useJUnitPlatform()
    testLogging {
        events("failed")
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
        showStandardStreams = true
    }
}
