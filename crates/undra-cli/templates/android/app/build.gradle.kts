import java.io.File
import javax.inject.Inject
import org.gradle.process.ExecOperations

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

/**
 * Builds the Rust core for Android: `undra build --platform android` (add `--release` for a release variant), which
 * writes `<abi>/lib@@NAMESPACE@@.so` where the `sourceSets` block below packages them. Gradle runs it before anything
 * else (`preBuild` depends on it, see `undraBuild` below) and skips it while the core's sources and the libraries it
 * wrote are unchanged, so there is no manual `undra build` step.
 */
abstract class UndraBuild @Inject constructor(private val execOps: ExecOperations) : DefaultTask() {
    /** A release core (what a release variant packages: tens of megabytes smaller) instead of a debug one. */
    @get:Input
    abstract val release: Property<Boolean>

    /** The directory of undra.toml. */
    @get:Internal
    abstract val projectRoot: DirectoryProperty

    /** What the core is built from. Add more with `undraBuild { sources.from("../../shared/src") }`. */
    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    /** Where `undra build` writes the libraries, one directory per ABI. */
    @get:OutputDirectory
    abstract val libraries: DirectoryProperty

    /** The Android SDK this build uses, told to `undra` when ANDROID_HOME is not set (Android Studio does not set it). */
    @get:Internal
    abstract val sdk: Property<String>

    @TaskAction
    fun build() {
        val undra = findUndra() ?: throw GradleException(undraNotFound())
        if (!File(undra).canExecute()) throw GradleException(undraNotFound("`$undra` (UNDRA_BIN) is not an executable file"))
        val command = mutableListOf(undra, "-C", projectRoot.get().asFile.absolutePath, "build", "--platform", "android")
        if (release.get()) command.add("--release")
        logger.lifecycle("undra: " + command.drop(3).joinToString(" "))
        try {
            execOps.exec {
                commandLine(command)
                val hasSdk = System.getenv("ANDROID_HOME") != null || System.getenv("ANDROID_SDK_ROOT") != null
                if (!hasSdk && sdk.isPresent) environment("ANDROID_HOME", sdk.get())
            }
        } catch (e: GradleException) {
            throw GradleException("`" + command.drop(3).joinToString(" ") + "` failed; its output is above. `undra doctor` checks the toolchain.", e)
        }
    }

    /** `UNDRA_BIN`, else `undra` on PATH or where the installers put it (a GUI-launched Gradle has a short PATH). */
    private fun findUndra(): String? {
        System.getenv("UNDRA_BIN")?.takeIf { it.isNotBlank() }?.let { return it }
        val home = System.getProperty("user.home")
        val dirs = System.getenv("PATH").orEmpty().split(File.pathSeparator) +
            listOf("$home/.undra/bin", "$home/.cargo/bin", "/opt/homebrew/bin", "/usr/local/bin")
        return dirs.filter { it.isNotBlank() }.map { File(it, "undra") }.firstOrNull { it.isFile && it.canExecute() }?.absolutePath
    }

    private fun undraNotFound(why: String = "`undra` is not on PATH or in ~/.undra/bin, ~/.cargo/bin or Homebrew's directories"): String = """
        error[undra::C0003]: `undra` was not found
          = note: the `undraBuild` task of android/app/build.gradle.kts runs `undra build --platform android` to compile the Rust core into lib@@NAMESPACE@@.so, and $why
          = help: curl -fsSL https://shreypdev.github.io/undra/install.sh | sh
                  then stop the Gradle daemon (./gradlew --stop) and restart Android Studio so they see the new PATH, or set UNDRA_BIN to the executable; `undra doctor` checks the rest of the toolchain
          = docs: https://shreypdev.github.io/undra/docs/errors.html#C0003
    """.trimIndent()
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
            // The ABIs `undra build --platform android` produces (undra.toml [android] abis).
            abiFilters += listOf(@@ABI_FILTERS@@)
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
            // Release builds never talk to a dev server: no URL, and no cleartext traffic (src/debug/AndroidManifest.xml
            // is not part of them).
            buildConfigField("String", "UNDRA_DEV_URL", "\"\"")
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
        buildConfig = true
    }

    sourceSets {
        // `undra build --platform android --release` writes lib@@NAMESPACE@@.so for every ABI here. The
        // bindings load it with System.loadLibrary("@@NAMESPACE@@"). Like any path in this file
        // it is relative to this module (android/app), not to android/; Gradle ignores a
        // directory that does not exist, so `undra build` checks this line after building.
        getByName("main").jniLibs.srcDir("@@JNI_LIBS_PATH@@")
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_11)
    }
}

// The core is part of this build. Every Gradle build runs `undraBuild` first (`preBuild` depends on it); it does the work
// only when the core's sources or manifests changed, or the libraries are missing. A release variant gets a release core
// (`assembleRelease`, `bundleRelease`, ...); anything else a debug one; `-PundraRelease=true|false` overrides. To build the
// core yourself (CI that builds it in an earlier step, say) skip the task with `-PundraSkipBuild=true` or UNDRA_SKIP_BUILD=1.
val undraBuild = tasks.register<UndraBuild>("undraBuild") {
    group = "undra"
    description = "Builds the Rust core for Android: undra build --platform android."
    projectRoot.set(layout.projectDirectory.dir("@@PROJECT_ROOT_FROM_APP@@"))
    sources.from(
        fileTree(layout.projectDirectory.dir("@@CORE_FROM_APP@@")) {
            include("src/**", "Cargo.toml", "Cargo.lock", "build.rs")
        },
        layout.projectDirectory.dir("@@PROJECT_ROOT_FROM_APP@@").file("undra.toml"),
        layout.projectDirectory.dir("@@PROJECT_ROOT_FROM_APP@@").file("Cargo.toml"),
        layout.projectDirectory.dir("@@PROJECT_ROOT_FROM_APP@@").file("Cargo.lock"),
    )
    libraries.set(layout.projectDirectory.dir("@@JNI_LIBS_PATH@@"))
    sdk.set(androidComponents.sdkComponents.sdkDirectory.map { it.asFile.absolutePath })
    release.convention(false)
    val skip = providers.gradleProperty("undraSkipBuild").map { it.toBoolean() }
        .orElse(providers.environmentVariable("UNDRA_SKIP_BUILD").map { it == "1" })
        .orElse(false)
    onlyIf { !skip.get() }
}

tasks.named("preBuild") { dependsOn(undraBuild) }

gradle.taskGraph.whenReady {
    val requested = providers.gradleProperty("undraRelease").map { it.toBoolean() }.orNull
    val releaseBuild = requested ?: allTasks.any { it.project == project && it.name.contains("Release") }
    undraBuild.configure { release.set(releaseBuild) }
}

dependencies {
    // The Undra runtime and the bindings generated from the core.
    implementation("dev.undra:runtime:@@KOTLIN_RUNTIME_VERSION@@")
    // The Android half of the runtime: the adapters of the standard ports (Http, Kv, SecureStore, Fs, Connectivity,
    // Lifecycle) and the Choreographer frame pacer. Its manifest declares INTERNET and ACCESS_NETWORK_STATE.
    implementation("dev.undra:android-adapters:@@KOTLIN_RUNTIME_VERSION@@")
    implementation(project(":core-bindings"))

    implementation(platform("androidx.compose:compose-bom:2024.10.01"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.3")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
