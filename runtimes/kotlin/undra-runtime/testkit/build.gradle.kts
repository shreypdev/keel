import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import org.jetbrains.kotlin.gradle.tasks.KotlinCompile

// dev.undra.testkit: the testing kit for Kotlin apps (docs/TESTING.md): PreviewCore (the app's own core with the
// deterministic fakes and a manual clock), RecordedCore (a recording played under the generated stores), and port
// record/replay. Kotlin stdlib + kotlinx-coroutines + :runtime only; JVM and Android (a Compose @Preview runs on the JVM).
plugins {
    alias(libs.plugins.kotlin.jvm)
    `maven-publish`
}

java {
    sourceCompatibility = JavaVersion.VERSION_11
    targetCompatibility = JavaVersion.VERSION_11
    withSourcesJar()
}

kotlin {
    explicitApi()
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_11)
        // RecordedCore is built on the runtime's embedding API.
        freeCompilerArgs.add("-opt-in=dev.undra.runtime.UndraEmbeddingApi")
    }
}

tasks.named<KotlinCompile>("compileKotlin") {
    compilerOptions {
        allWarningsAsErrors.set(true)
    }
}

dependencies {
    api(project(":runtime"))
    api(libs.kotlinx.coroutines.core)

    testImplementation(platform(libs.junit.bom))
    testImplementation(libs.junit.jupiter)
    testRuntimeOnly(libs.junit.platform.launcher)
}

tasks.test {
    useJUnitPlatform()
    // The tests read the checked-in fixtures (testkit/fixtures, testkit/conformance) from the repository.
    systemProperty("undra.testkit.dir", rootProject.projectDir.resolve("../../../testkit").absolutePath)
    testLogging {
        events("failed")
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
        showStandardStreams = true
    }
}

// ADR-063: published at every release (JitPack builds the tag; scripts/jitpack-install.sh), with its sources.
publishing {
    publications {
        create<MavenPublication>("maven") {
            from(components["java"])
        }
    }
}
