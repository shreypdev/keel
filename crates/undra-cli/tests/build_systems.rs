//! The generated apps build the core by themselves: `undra build` is never a manual step.
//!
//! Each test makes a project with `undra init`, then runs the app's own build system without any
//! earlier `undra build` and checks that it built the core: Gradle's `undraBuild` task, Xcode's
//! "Build the Undra core" Run Script phase and the `undra()` Vite plugin. They run the real tools,
//! so they take minutes and need the toolchains (and, for the web app, the npm registry):
//!
//! | Environment | What happens |
//! |---|---|
//! | `UNDRA_REQUIRE_TOOLCHAINS=1` (CI sets it) | the test runs, and a missing toolchain fails it |
//! | `UNDRA_TEST_BUILD_SYSTEMS=1` | the test runs when the toolchain is there and is skipped, with the reason, when it is not |
//! | neither | skipped, with the reason |
//!
//! What a toolchain needs is decided by `undra doctor --json` on the project, so these tests use the
//! same list a person does.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Project, flag, init_project, path_with_undra, serial};

/// Whether the test should run; says why on stderr when it should not.
fn gate(test: &str, project: &Project, platform: &str) -> bool {
    let require = flag("UNDRA_REQUIRE_TOOLCHAINS");
    if !require && !flag("UNDRA_TEST_BUILD_SYSTEMS") {
        eprintln!(
            "skipped {test}: it runs the real build system (minutes); set UNDRA_TEST_BUILD_SYSTEMS=1 to run it where the toolchain is installed, or UNDRA_REQUIRE_TOOLCHAINS=1 to require it"
        );
        return false;
    }
    let blockers = blockers(project, platform);
    if blockers.is_empty() {
        return true;
    }
    let reason = format!(
        "{test} needs the {platform} toolchain, and `undra doctor` reports: {}",
        blockers.join("; ")
    );
    assert!(!require, "{reason} (UNDRA_REQUIRE_TOOLCHAINS=1)");
    eprintln!("skipped {reason}");
    false
}

/// The failures `undra doctor --platform <platform>` reports for the project.
fn blockers(project: &Project, platform: &str) -> Vec<String> {
    let out = project
        .undra()
        .args(["doctor", "--json", "--platform", platform])
        .output()
        .expect("undra doctor runs");
    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("doctor --json prints JSON");
    doc["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["findings"].as_array().into_iter().flatten())
        .filter(|f| f["severity"] == "fail")
        .map(|f| {
            format!(
                "{}: {}",
                f["id"].as_str().unwrap_or("?"),
                f["message"].as_str().unwrap_or("?")
            )
        })
        .collect()
}

/// The observed value of a doctor finding of the project.
fn observed(project: &Project, platform: &str, id: &str) -> Option<String> {
    let out = project
        .undra()
        .args(["doctor", "--json", "--platform", platform])
        .output()
        .ok()?;
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    doc["sections"]
        .as_array()?
        .iter()
        .flat_map(|s| s["findings"].as_array().into_iter().flatten())
        .find(|f| f["id"] == id)?["observed"]
        .as_str()
        .map(ToOwned::to_owned)
}

/// `PATH` without any directory that has an `undra` in it.
fn path_without_undra() -> std::ffi::OsString {
    let dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|d| !d.join("undra").exists())
        .collect();
    std::env::join_paths(dirs).expect("a PATH")
}

/// An `undra` installed where the build systems look besides `PATH` (they would find it, so the
/// "not installed" tests cannot run on this machine).
fn undra_installed_elsewhere() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [
        home.join(".undra/bin/undra"),
        home.join(".cargo/bin/undra"),
        PathBuf::from("/opt/homebrew/bin/undra"),
        PathBuf::from("/usr/local/bin/undra"),
    ]
    .into_iter()
    .find(|p| p.exists())
}

/// Runs `cmd` and returns whether it succeeded and everything it printed.
fn logged(cmd: &mut Command) -> (bool, String) {
    let out = cmd.output().expect("the command starts");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// Prints the lines of `log` that mention `needles`: the evidence a run leaves in the test output.
fn evidence(what: &str, log: &str, needles: &[&str]) {
    eprintln!("--- {what}");
    for line in log
        .lines()
        .filter(|l| needles.iter().any(|n| l.contains(n)))
    {
        eprintln!("    {}", line.chars().take(160).collect::<String>());
    }
}

fn append(path: &Path, text: &str) {
    let mut contents = std::fs::read_to_string(path).unwrap();
    contents.push_str(text);
    std::fs::write(path, contents).unwrap();
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[test]
fn gradle_builds_the_core_before_the_app_and_skips_it_while_nothing_changed() {
    let _serial = serial();
    let project = init_project("gradleproof", "android");
    if !gate("the Gradle build", &project, "android") {
        return;
    }
    let android = project.root.join("android");
    let jni = project.root.join("build/android/jniLibs");
    assert!(
        !project.root.join("build").exists(),
        "no earlier `undra build`"
    );
    let gradle = |args: &[&str], path: std::ffi::OsString| {
        let mut cmd = Command::new(android.join("gradlew"));
        cmd.args(args)
            .arg("--console=plain")
            .current_dir(&android)
            .env("PATH", path)
            .env("CARGO_TARGET_DIR", common::shared_target())
            .env_remove("UNDRA_BIN")
            .env_remove("UNDRA_SKIP_BUILD");
        if std::env::var_os("ANDROID_HOME").is_none() {
            if let Some(sdk) = observed(&project, "android", "android.sdk") {
                cmd.env("ANDROID_HOME", sdk);
            }
        }
        logged(&mut cmd)
    };

    // 1. A clean project: the debug build makes the core, then the APK.
    let (ok, log) = gradle(&["assembleDebug"], path_with_undra());
    evidence(
        "gradle assembleDebug, no earlier undra build",
        &log,
        &[
            "Task :app:undraBuild",
            "undra: ",
            "==> Building the core",
            "Built:",
            "BUILD ",
        ],
    );
    assert!(ok, "{log}");
    assert!(
        log.contains("> Task :app:undraBuild\n") || log.contains("> Task :app:undraBuild "),
        "{log}"
    );
    assert!(
        log.contains("undra: build --platform android\n"),
        "a debug variant builds a debug core:\n{log}"
    );
    assert!(
        log.contains("==> Building the core for android (debug)"),
        "{log}"
    );
    for abi in ["arm64-v8a", "x86_64"] {
        let lib = jni.join(abi).join("libundra_core.so");
        assert!(lib.is_file(), "{abi}: no library\n{log}");
        eprintln!("android debug {abi}: {} bytes", size(&lib));
    }
    let apk = android.join("app/build/outputs/apk/debug/app-debug.apk");
    assert!(apk.is_file(), "no APK\n{log}");
    assert!(
        log.find("Task :app:undraBuild").unwrap()
            < log.find("Task :app:mergeDebugJniLibFolders").unwrap(),
        "the core is built before its libraries are merged"
    );

    // 2. Nothing changed: Gradle skips the task. (The first run may have created Cargo.lock, an
    // input, so the second run is allowed to do the work once more.)
    let _ = gradle(&["assembleDebug"], path_with_undra());
    let (ok, log) = gradle(&["assembleDebug"], path_with_undra());
    evidence(
        "gradle assembleDebug, nothing changed",
        &log,
        &["Task :app:undraBuild", "BUILD "],
    );
    assert!(ok, "{log}");
    assert!(log.contains("> Task :app:undraBuild UP-TO-DATE"), "{log}");

    // 3. A change of the core's sources builds it again.
    append(&project.root.join("core/src/lib.rs"), "\n// a change\n");
    let (ok, log) = gradle(&["assembleDebug"], path_with_undra());
    evidence(
        "gradle assembleDebug, core/src changed",
        &log,
        &[
            "Task :app:undraBuild",
            "undra: ",
            "==> Building the core",
            "BUILD ",
        ],
    );
    assert!(ok, "{log}");
    assert!(!log.contains("> Task :app:undraBuild UP-TO-DATE"), "{log}");
    assert!(
        log.contains("==> Building the core for android (debug)"),
        "{log}"
    );

    // 4. A release variant builds a release core, and the libraries are small.
    let (ok, log) = gradle(&["assembleRelease"], path_with_undra());
    evidence(
        "gradle assembleRelease",
        &log,
        &[
            "Task :app:undraBuild",
            "undra: ",
            "==> Building the core",
            "BUILD ",
        ],
    );
    assert!(ok, "{log}");
    assert!(
        log.contains("undra: build --platform android --release\n"),
        "{log}"
    );
    let release_size = size(&jni.join("arm64-v8a/libundra_core.so"));
    eprintln!("android release arm64-v8a: {release_size} bytes");
    assert!(
        release_size < 5_000_000,
        "a release core is megabytes, not tens of megabytes: {release_size}"
    );

    // 5. -PundraSkipBuild leaves the build to the caller.
    append(&project.root.join("core/src/lib.rs"), "// another change\n");
    let (ok, log) = gradle(
        &["assembleDebug", "-PundraSkipBuild=true"],
        path_with_undra(),
    );
    assert!(
        ok && log.contains("> Task :app:undraBuild SKIPPED"),
        "{log}"
    );
}

#[test]
fn gradle_without_undra_says_how_to_install_it() {
    let _serial = serial();
    if let Some(found) = undra_installed_elsewhere() {
        eprintln!(
            "skipped: undra is installed at {}, where the Gradle task finds it",
            found.display()
        );
        return;
    }
    let project = init_project("gradleabsent", "android");
    if !gate("the Gradle build without undra", &project, "android") {
        return;
    }
    let android = project.root.join("android");
    let mut cmd = Command::new(android.join("gradlew"));
    cmd.args(["assembleDebug", "--console=plain"])
        .current_dir(&android)
        .env("PATH", path_without_undra())
        .env_remove("UNDRA_BIN");
    if std::env::var_os("ANDROID_HOME").is_none() {
        if let Some(sdk) = observed(&project, "android", "android.sdk") {
            cmd.env("ANDROID_HOME", sdk);
        }
    }
    let (ok, log) = logged(&mut cmd);
    evidence(
        "gradle assembleDebug, no undra",
        &log,
        &["undraBuild", "error[undra", "= help", "= note", "BUILD "],
    );
    assert!(!ok, "{log}");
    assert!(
        log.contains("error[undra::C0003]: `undra` was not found")
            && log.contains("curl -fsSL https://shreypdev.github.io/undra/install.sh | sh")
            && log.contains("https://shreypdev.github.io/undra/docs/errors.html#C0003"),
        "{log}"
    );
}

#[test]
fn xcode_builds_the_core_in_a_build_phase_and_skips_it_while_nothing_changed() {
    let _serial = serial();
    let project = init_project("xcodeproof", "ios");
    if !gate("the Xcode build", &project, "ios") {
        return;
    }
    let derived = common::TempDir::new("xcode-dd");
    let build = |configuration: &str| {
        let mut cmd = Command::new("xcodebuild");
        cmd.args(["-project"])
            .arg(project.root.join("ios/Xcodeproof.xcodeproj"))
            .args([
                "-scheme",
                "Xcodeproof",
                "-sdk",
                "iphonesimulator",
                "-configuration",
                configuration,
            ])
            .arg("-derivedDataPath")
            .arg(derived.path())
            .arg("build")
            // The runtime package ships link-time stand-ins for the core; this switches them off.
            .env("UNDRA_LINK_CORE", "1")
            .env("PATH", path_with_undra())
            .env("CARGO_TARGET_DIR", common::shared_target());
        logged(&mut cmd)
    };
    let phase = "PhaseScriptExecution Build\\ the\\ Undra\\ core";
    assert!(
        !project.root.join("build").exists(),
        "no earlier `undra build`"
    );

    // 1. A clean project: the phase runs, before the app compiles, and the app links the core.
    let (ok, log) = build("Debug");
    evidence(
        "xcodebuild, no earlier undra build",
        &log,
        &[
            phase,
            "note: undra build",
            "==> Building the core",
            "Built:",
            "BUILD ",
        ],
    );
    assert!(
        ok,
        "{}",
        log.lines().rev().take(60).collect::<Vec<_>>().join("\n")
    );
    assert!(log.contains(phase), "the Run Script phase ran:\n{log}");
    assert!(
        log.contains("note: undra build --platform ios --configuration Debug"),
        "{log}"
    );
    assert!(
        log.contains("==> Building the core for ios (debug)"),
        "{log}"
    );
    assert!(log.contains("** BUILD SUCCEEDED **"), "{log}");
    assert!(
        log.find(phase).unwrap()
            < log
                .find("SwiftDriver Xcodeproof normal")
                .expect("the app was compiled"),
        "the core is built before the app is compiled"
    );
    let xcframework = project.root.join("build/ios/UndraCore.xcframework");
    assert!(xcframework.join("ios-arm64/libundra_core.a").is_file());
    assert!(
        xcframework
            .join("ios-arm64-simulator/libundra_core.a")
            .is_file()
    );
    assert!(
        project
            .root
            .join("build/ios/.undra-configuration-Debug")
            .is_file()
    );
    assert!(
        derived
            .path()
            .join("Build/Products/Debug-iphonesimulator/Xcodeproof.app")
            .is_dir()
    );

    // 2. Nothing changed: Xcode does not run the phase.
    let (ok, log) = build("Debug");
    evidence(
        "xcodebuild, nothing changed",
        &log,
        &[phase, "note: undra build", "BUILD "],
    );
    assert!(ok, "{log}");
    assert!(
        !log.contains(phase),
        "Xcode skips the phase while no input changed:\n{log}"
    );

    // 3. A change of a source of the core runs it again; so does a new file, and after it an edit of that file.
    append(&project.root.join("core/src/lib.rs"), "\n// a change\n");
    let (ok, log) = build("Debug");
    evidence(
        "xcodebuild, core/src/lib.rs changed",
        &log,
        &[phase, "note: undra build", "BUILD "],
    );
    assert!(ok && log.contains(phase), "{log}");
    std::fs::write(
        project.root.join("core/src/extra.rs"),
        "// not compiled yet\n",
    )
    .unwrap();
    let (ok, log) = build("Debug");
    assert!(
        ok && log.contains(phase),
        "a new file changes its directory:\n{log}"
    );
    let (ok, _) = build("Debug");
    assert!(ok);
    let inputs =
        std::fs::read_to_string(project.root.join("ios/Config/undra-core-inputs.xcfilelist"))
            .unwrap();
    assert!(
        inputs.contains("$(SRCROOT)/../core/src/extra.rs"),
        "the build refreshed the input list:\n{inputs}"
    );
    append(
        &project.root.join("core/src/extra.rs"),
        "// edited in place\n",
    );
    let (ok, log) = build("Debug");
    assert!(
        ok && log.contains(phase),
        "an edit of a file the list gained is seen:\n{log}"
    );

    // 4. Another configuration builds the other kind of core, and switching back runs the phase again.
    let (ok, log) = build("Release");
    evidence(
        "xcodebuild -configuration Release",
        &log,
        &[
            phase,
            "note: undra build",
            "==> Building the core",
            "BUILD ",
        ],
    );
    assert!(
        ok,
        "{}",
        log.lines().rev().take(60).collect::<Vec<_>>().join("\n")
    );
    assert!(
        log.contains("note: undra build --platform ios --configuration Release"),
        "{log}"
    );
    assert!(
        log.contains("==> Building the core for ios (release)"),
        "{log}"
    );
    assert!(
        project
            .root
            .join("build/ios/.undra-configuration-Release")
            .is_file()
    );
    assert!(
        !project
            .root
            .join("build/ios/.undra-configuration-Debug")
            .exists()
    );
    let (ok, log) = build("Release");
    assert!(ok && !log.contains(phase), "{log}");
    let (ok, log) = build("Debug");
    assert!(
        ok && log.contains(phase),
        "back to Debug rebuilds the debug core:\n{log}"
    );
}

#[test]
fn xcode_without_undra_says_how_to_install_it() {
    let _serial = serial();
    if let Some(found) = undra_installed_elsewhere() {
        eprintln!(
            "skipped: undra is installed at {}, where the build phase finds it",
            found.display()
        );
        return;
    }
    let project = init_project("xcodeabsent", "ios");
    if !gate("the Xcode build without undra", &project, "ios") {
        return;
    }
    let derived = common::TempDir::new("xcode-dd-absent");
    let mut cmd = Command::new("xcodebuild");
    cmd.args(["-project"])
        .arg(project.root.join("ios/Xcodeabsent.xcodeproj"))
        .args([
            "-scheme",
            "Xcodeabsent",
            "-sdk",
            "iphonesimulator",
            "-derivedDataPath",
        ])
        .arg(derived.path())
        .arg("build")
        .env("UNDRA_LINK_CORE", "1")
        .env("PATH", path_without_undra());
    let (ok, log) = logged(&mut cmd);
    evidence("xcodebuild, no undra", &log, &["error:", "BUILD "]);
    assert!(!ok, "{log}");
    assert!(
        log.contains("error: [undra::C0003] 'undra' was not found")
            && log.contains("curl -fsSL https://shreypdev.github.io/undra/install.sh | sh"),
        "{log}"
    );
}

#[test]
fn npm_run_build_builds_the_core_through_the_vite_plugin() {
    let _serial = serial();
    let project = init_project("viteproof", "web");
    if !gate("the web build", &project, "web") {
        return;
    }
    let web = project.root.join("web");
    let npm = |args: &[&str], path: std::ffi::OsString| {
        let mut cmd = Command::new("npm");
        cmd.args(args)
            .current_dir(&web)
            .env("PATH", path)
            .env("CARGO_TARGET_DIR", common::shared_target())
            .env_remove("UNDRA_BIN")
            .env_remove("UNDRA_SKIP_BUILD");
        logged(&mut cmd)
    };
    let (ok, log) = npm(
        &["install", "--no-audit", "--no-fund"],
        std::env::var_os("PATH").unwrap_or_default(),
    );
    assert!(ok, "npm install needs the npm registry:\n{log}");
    assert!(
        !project.root.join("build").exists(),
        "no earlier `undra build`"
    );

    // 1. `npm run build`: type-check, then Vite starts, the plugin builds the core, the bundle has it.
    let (ok, log) = npm(&["run", "build"], path_with_undra());
    evidence(
        "npm run build, no earlier undra build",
        &log,
        &[
            "vite v",
            "==> Building the core",
            "Built:",
            "web wasm",
            "built in",
        ],
    );
    assert!(ok, "{log}");
    assert!(
        log.contains("==> Building the core for web (release)"),
        "{log}"
    );
    assert!(log.contains("web wasm"), "{log}");
    let wasm = project.root.join("build/web/undra_core.wasm");
    assert!(wasm.is_file(), "{log}");
    eprintln!("web wasm: {} bytes", size(&wasm));
    let bundled = std::fs::read_dir(web.join("dist/assets"))
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().ends_with(".wasm"));
    assert!(bundled, "the wasm core is bundled");
    assert!(
        log.find("vite v").unwrap() < log.find("==> Building the core").unwrap(),
        "Vite started the build"
    );

    // 2. A second build runs the plugin again (it is cheap: Cargo has nothing to do).
    let (ok, log) = npm(&["run", "build"], path_with_undra());
    assert!(ok, "{log}");
    assert!(
        log.contains("==> Building the core for web (release)"),
        "{log}"
    );
    assert!(
        !log.contains("Compiling webapp") && !log.contains("Compiling viteproof-core"),
        "an unchanged core is not recompiled:\n{log}"
    );
}

#[test]
fn npm_run_build_without_undra_says_how_to_install_it() {
    let _serial = serial();
    if let Some(found) = undra_installed_elsewhere() {
        eprintln!(
            "skipped: undra is installed at {}, where the Vite plugin finds it",
            found.display()
        );
        return;
    }
    let project = init_project("viteabsent", "web");
    if !gate("the web build without undra", &project, "web") {
        return;
    }
    let web = project.root.join("web");
    let mut install = Command::new("npm");
    install
        .args(["install", "--no-audit", "--no-fund"])
        .current_dir(&web);
    let (ok, log) = logged(&mut install);
    assert!(ok, "npm install needs the npm registry:\n{log}");
    let mut build = Command::new("npm");
    build
        .args(["run", "build"])
        .current_dir(&web)
        .env("PATH", path_without_undra())
        .env_remove("UNDRA_BIN");
    let (ok, log) = logged(&mut build);
    evidence(
        "npm run build, no undra",
        &log,
        &["error[undra", "= help", "= note", "Build failed"],
    );
    assert!(!ok, "{log}");
    assert!(
        log.contains("error[undra::C0003]: `undra` was not found")
            && log.contains("curl -fsSL https://shreypdev.github.io/undra/install.sh | sh"),
        "{log}"
    );
}
