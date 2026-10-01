//! `undra doctor`: what this machine has against what the platforms need.
//!
//! Every check is a function of a [`Sys`], so the tests describe machines (a Mac with Xcode but
//! no NDK, a Linux box with only Rust) and assert the findings, without depending on the one
//! they run on. Each gap names the one-line fix.

use std::path::{Path, PathBuf};

use crate::config::Platform;
use crate::error::Result;
use crate::project::Project;
use crate::sys::{Os, Sys};
use crate::toolchain::{Toolchain, ndk_major};
use crate::ui::Ui;

use super::Env;

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Present and fine.
    Ok,
    /// Something is off but builds still work.
    Warn,
    /// A build for a platform in scope cannot work.
    Fail,
    /// Not applicable on this machine.
    Skip,
}

/// One line of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The status.
    pub status: Status,
    /// What was checked and what was found.
    pub message: String,
    /// What to do about it (one line); empty when nothing needs doing.
    pub fix: String,
}

impl Finding {
    fn ok(message: impl Into<String>) -> Finding {
        Finding {
            status: Status::Ok,
            message: message.into(),
            fix: String::new(),
        }
    }

    fn warn(message: impl Into<String>, fix: impl Into<String>) -> Finding {
        Finding {
            status: Status::Warn,
            message: message.into(),
            fix: fix.into(),
        }
    }

    fn fail(message: impl Into<String>, fix: impl Into<String>) -> Finding {
        Finding {
            status: Status::Fail,
            message: message.into(),
            fix: fix.into(),
        }
    }

    fn skip(message: impl Into<String>) -> Finding {
        Finding {
            status: Status::Skip,
            message: message.into(),
            fix: String::new(),
        }
    }
}

/// A titled group of findings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// `Rust`, `iOS`, ...
    pub title: &'static str,
    /// The findings.
    pub findings: Vec<Finding>,
}

/// Runs `undra doctor`.
///
/// # Errors
///
/// `C0009` for an unknown platform name; a broken undra.toml is reported as such.
pub fn run(env: &Env<'_>, platform: Option<&str>) -> Result<bool> {
    let (scope, project) = scope(env, platform)?;
    let toolchain = Toolchain::detect(env.sys);
    let android = project
        .as_ref()
        .map(|p| p.config.android.abis.clone())
        .unwrap_or_else(|| vec!["arm64-v8a".into(), "x86_64".into()]);
    let sections = check(env.sys, &toolchain, &scope, &android);
    env.ui.line(&render(
        &sections,
        &env.ui,
        project.as_ref().map(|p| p.config.name.as_str()),
        &scope,
    ));
    let failed = sections
        .iter()
        .flat_map(|s| &s.findings)
        .any(|f| f.status == Status::Fail);
    Ok(!failed)
}

/// The platforms to check and the project, when there is one.
fn scope(env: &Env<'_>, platform: Option<&str>) -> Result<(Vec<Platform>, Option<Project>)> {
    let project = match Project::discover(&env.start_dir()?) {
        Ok(p) => Some(p),
        Err(e) if e.code == crate::error::Code::NoProject => None,
        Err(e) => return Err(e),
    };
    let platforms = match (platform, &project) {
        (Some(list), _) => Platform::parse_list(list)?,
        (None, Some(p)) => p.config.platforms.clone(),
        (None, None) => Platform::ALL.to_vec(),
    };
    Ok((platforms, project))
}

/// Runs every check for the platforms in `scope`.
#[must_use]
pub fn check(
    sys: &dyn Sys,
    toolchain: &Toolchain,
    scope: &[Platform],
    android_abis: &[String],
) -> Vec<Section> {
    let mut sections = vec![Section {
        title: "Rust",
        findings: rust(sys, toolchain),
    }];
    if scope.contains(&Platform::Ios) {
        sections.push(Section {
            title: "iOS",
            findings: ios(sys, toolchain),
        });
    }
    if scope.contains(&Platform::Android) {
        sections.push(Section {
            title: "Android",
            findings: android(sys, toolchain, android_abis),
        });
    }
    if scope.contains(&Platform::Web) {
        sections.push(Section {
            title: "Web",
            findings: web(sys, toolchain),
        });
    }
    sections
}

/// The first line of a tool's version output.
fn version_line(
    sys: &dyn Sys,
    toolchain: &Toolchain,
    program: &Path,
    args: &[&str],
) -> Option<String> {
    let out = sys.run(program, args, &toolchain.env_pairs())?;
    if !out.success {
        return None;
    }
    out.text().lines().next().map(|l| l.trim().to_owned())
}

/// Parses `rustc 1.98.1 (...)` into `(1, 98)`.
fn rust_version(line: &str) -> Option<(u32, u32)> {
    let version = line.split_whitespace().nth(1)?;
    let mut parts = version.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// `v22.3.0` into `22`.
fn node_major(line: &str) -> Option<u32> {
    line.trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn rust_target(sys: &dyn Sys, toolchain: &Toolchain, triple: &str) -> Option<bool> {
    let rustc = toolchain.which(sys, "rustc")?;
    let out = sys.run(&rustc, &["--print", "sysroot"], &toolchain.env_pairs())?;
    if !out.success {
        return None;
    }
    let sysroot = PathBuf::from(out.stdout.trim());
    Some(sys.is_dir(&sysroot.join("lib/rustlib").join(triple)))
}

fn target_finding(sys: &dyn Sys, toolchain: &Toolchain, triple: &str, needed_for: &str) -> Finding {
    match rust_target(sys, toolchain, triple) {
        Some(true) => Finding::ok(format!("Rust target {triple}")),
        Some(false) => Finding::fail(
            format!("Rust target {triple} is not installed ({needed_for})"),
            format!("rustup target add {triple}"),
        ),
        None => Finding::warn(
            format!("could not check the Rust target {triple}"),
            "check `rustc --print sysroot` works",
        ),
    }
}

fn rust(sys: &dyn Sys, toolchain: &Toolchain) -> Vec<Finding> {
    let mut out = Vec::new();
    match toolchain.which(sys, "rustc") {
        None => out.push(Finding::fail(
            "rustc was not found",
            "install Rust with rustup: https://rustup.rs (then open a new terminal)",
        )),
        Some(rustc) => match version_line(sys, toolchain, &rustc, &["--version"]) {
            Some(line) => match rust_version(&line) {
                Some(v) if v >= (1, 85) => out.push(Finding::ok(line)),
                Some(_) => out.push(Finding::fail(
                    format!("{line}: Undra needs Rust 1.85 or newer (edition 2024)"),
                    "rustup update stable",
                )),
                None => out.push(Finding::warn(
                    format!("rustc prints an unexpected version: {line}"),
                    "",
                )),
            },
            None => out.push(Finding::fail(
                "rustc is installed but does not run",
                "reinstall it: rustup self uninstall, then https://rustup.rs",
            )),
        },
    }
    match toolchain.which(sys, "cargo") {
        Some(cargo) => match version_line(sys, toolchain, &cargo, &["--version"]) {
            Some(line) => out.push(Finding::ok(line)),
            None => out.push(Finding::fail(
                "cargo is installed but does not run",
                "rustup update stable",
            )),
        },
        None => out.push(Finding::fail(
            "cargo was not found",
            "install Rust with rustup: https://rustup.rs",
        )),
    }
    out
}

fn ios(sys: &dyn Sys, toolchain: &Toolchain) -> Vec<Finding> {
    if sys.os() != Os::Macos {
        return vec![Finding::skip(
            "iOS builds need macOS (Xcode); build the other platforms here and iOS on a Mac or in CI",
        )];
    }
    let mut out = Vec::new();
    match toolchain.which(sys, "xcodebuild") {
        None => out.push(Finding::fail(
            "xcodebuild was not found",
            "install Xcode from the App Store, then `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`",
        )),
        Some(xcodebuild) => match version_line(sys, toolchain, &xcodebuild, &["-version"]) {
            Some(line) => out.push(Finding::ok(line)),
            None => out.push(Finding::fail(
                "xcodebuild does not work (only the command line tools are installed)",
                "install Xcode, then `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`",
            )),
        },
    }
    // Where xcode-select points, and what undra does about it.
    let selected = toolchain
        .which(sys, "xcode-select")
        .and_then(|tool| sys.run(&tool, &["-p"], &[]))
        .map(|o| o.stdout.trim().to_owned())
        .unwrap_or_default();
    let developer_dir = sys.env("DEVELOPER_DIR");
    if let Some(dir) = developer_dir {
        out.push(Finding::ok(format!("DEVELOPER_DIR is set to {dir}")));
    } else if selected.ends_with("CommandLineTools") {
        let xcode_installed = sys.is_dir(Path::new(crate::toolchain::XCODE_DEVELOPER_DIR));
        if xcode_installed {
            out.push(Finding::warn(
                "xcode-select points at the command line tools although Xcode is installed (undra uses DEVELOPER_DIR for its own builds)",
                format!("sudo xcode-select -s {}", crate::toolchain::XCODE_DEVELOPER_DIR),
            ));
        } else {
            out.push(Finding::fail(
                "xcode-select points at the command line tools, and Xcode is not installed",
                "install Xcode from the App Store",
            ));
        }
    } else if !selected.is_empty() {
        out.push(Finding::ok(format!("xcode-select: {selected}")));
    }
    if toolchain.which(sys, "lipo").is_none() {
        out.push(Finding::fail(
            "lipo was not found",
            "xcode-select --install",
        ));
    }
    out.push(target_finding(
        sys,
        toolchain,
        "aarch64-apple-ios",
        "iOS devices",
    ));
    out.push(target_finding(
        sys,
        toolchain,
        "aarch64-apple-ios-sim",
        "the simulator on Apple silicon",
    ));
    out
}

fn android(sys: &dyn Sys, toolchain: &Toolchain, abis: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    match &toolchain.android_sdk {
        Some(sdk) => {
            let env_set = sys.env("ANDROID_HOME").is_some() || sys.env("ANDROID_SDK_ROOT").is_some();
            if env_set {
                out.push(Finding::ok(format!("Android SDK at {}", sdk.display())));
            } else {
                out.push(Finding::warn(
                    format!("Android SDK found at {} but ANDROID_HOME is not set (undra finds it anyway; Gradle and Android Studio may not)", sdk.display()),
                    format!("export ANDROID_HOME={}", sdk.display()),
                ));
            }
            if sys.is_dir(&sdk.join("platforms/android-35")) || sys.is_dir(&sdk.join("platforms/android-36")) {
                out.push(Finding::ok("Android platform 35+ installed"));
            } else {
                out.push(Finding::warn(
                    "no Android platform 35 or newer in the SDK (the app shell compiles against 35)",
                    "sdkmanager \"platforms;android-35\" \"build-tools;35.0.0\"",
                ));
            }
        }
        None => out.push(Finding::fail(
            "no Android SDK found (ANDROID_HOME is not set and the usual locations are empty)",
            "install Android Studio, or the command line tools (`brew install --cask android-commandlinetools`), and set ANDROID_HOME",
        )),
    }
    match &toolchain.android_ndk {
        Some(ndk) => match ndk_major(ndk) {
            Some(major) if major >= 27 => out.push(Finding::ok(format!(
                "Android NDK r{major} at {}",
                ndk.display()
            ))),
            Some(major) => out.push(Finding::fail(
                format!(
                    "Android NDK r{major} is too old: libraries need r27+ for 16 KB page alignment"
                ),
                "sdkmanager \"ndk;27.2.12479018\"",
            )),
            None => out.push(Finding::ok(format!("Android NDK at {}", ndk.display()))),
        },
        None => out.push(Finding::fail(
            "no Android NDK found",
            "sdkmanager \"ndk;27.2.12479018\" (then ANDROID_NDK_HOME is found from ANDROID_HOME)",
        )),
    }
    match toolchain.which(sys, "cargo-ndk") {
        Some(_) => {
            // `cargo ndk --version` (the binary alone does not answer outside of cargo).
            let version = toolchain
                .which(sys, "cargo")
                .and_then(|cargo| version_line(sys, toolchain, &cargo, &["ndk", "--version"]))
                .unwrap_or_else(|| "cargo-ndk".to_owned());
            out.push(Finding::ok(version));
        }
        None => out.push(Finding::fail(
            "cargo-ndk was not found",
            "cargo install cargo-ndk",
        )),
    }
    for abi in abis {
        out.push(target_finding(
            sys,
            toolchain,
            crate::builds::android::triple_of(abi),
            &format!("the {abi} ABI"),
        ));
    }
    out.push(jdk(sys, toolchain));
    out
}

/// The JDK check: Gradle and the Kotlin tests need one (17+ for current Android Gradle plugins).
fn jdk(sys: &dyn Sys, toolchain: &Toolchain) -> Finding {
    // A JDK on PATH that runs.
    if let Some(java) = toolchain.which(sys, "java") {
        let out = sys.run(&java, &["-version"], &toolchain.env_pairs());
        if let Some(out) = out.filter(|o| o.success) {
            let text = out.text();
            let first = text.lines().next().unwrap_or("java").to_owned();
            return match java_major(&first) {
                Some(major) if major >= 17 => Finding::ok(first),
                Some(major) => Finding::warn(
                    format!("{first}: Android Gradle plugins need JDK 17 or newer"),
                    format!("install a newer JDK (Java {major} found): `brew install openjdk@17`"),
                ),
                None => Finding::ok(first),
            };
        }
    }
    // A JDK that is installed but not on PATH (Homebrew's openjdk is keg-only).
    for candidate in [
        "/opt/homebrew/opt/openjdk@17",
        "/opt/homebrew/opt/openjdk",
        "/usr/local/opt/openjdk@17",
        "/usr/local/opt/openjdk",
    ] {
        if sys.is_file(&Path::new(candidate).join("bin/java")) {
            return Finding::warn(
                format!("a JDK is installed at {candidate} but `java` is not on PATH"),
                format!("export JAVA_HOME={candidate} and add $JAVA_HOME/bin to PATH"),
            );
        }
    }
    Finding::warn(
        "no JDK found (Gradle, the Kotlin tests and Android Studio builds need one; `undra build` does not)",
        "brew install openjdk@17 (or install Android Studio, which bundles one)",
    )
}

/// `openjdk version "17.0.12" ...` / `java version "1.8.0"` into the major version.
fn java_major(line: &str) -> Option<u32> {
    let quoted = line.split('"').nth(1)?;
    let mut parts = quoted.split('.');
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

fn web(sys: &dyn Sys, toolchain: &Toolchain) -> Vec<Finding> {
    let mut out = vec![target_finding(
        sys,
        toolchain,
        "wasm32-unknown-unknown",
        "the web build",
    )];
    match toolchain.which(sys, "node") {
        None => out.push(Finding::fail(
            "node was not found (the web app shell and the TypeScript runtime need Node 20+)",
            "install Node 20 or newer: https://nodejs.org (or `brew install node`, `fnm install 22`)",
        )),
        Some(node) => match version_line(sys, toolchain, &node, &["--version"]) {
            Some(line) => match node_major(&line) {
                Some(major) if major >= 20 => out.push(Finding::ok(format!("node {line}"))),
                Some(_) => out.push(Finding::fail(format!("node {line} is too old (Undra needs 20+)"), "install Node 20 or newer")),
                None => out.push(Finding::ok(format!("node {line}"))),
            },
            None => out.push(Finding::fail("node is installed but does not run", "reinstall Node 20 or newer")),
        },
    }
    if toolchain.which(sys, "npm").is_none() {
        out.push(Finding::warn(
            "npm was not found (the web app shell installs its dependencies with it)",
            "it ships with Node: https://nodejs.org",
        ));
    }
    match toolchain.which(sys, "wasm-opt") {
        Some(tool) => {
            let version = version_line(sys, toolchain, &tool, &["--version"]).unwrap_or_else(|| "wasm-opt".to_owned());
            out.push(Finding::ok(version));
        }
        None => out.push(Finding::warn(
            "wasm-opt (binaryen) was not found: the wasm module is built but not shrunk further (10-20% larger)",
            "brew install binaryen (or `npm i -g wasm-opt`)",
        )),
    }
    out
}

/// Renders the report.
#[must_use]
pub fn render(sections: &[Section], ui: &Ui, project: Option<&str>, scope: &[Platform]) -> String {
    let mut out = String::new();
    let names: Vec<&str> = scope.iter().map(|p| p.name()).collect();
    match project {
        Some(name) => out.push_str(&format!(
            "{} checking {name} ({})\n",
            ui.bold_out("undra doctor:"),
            names.join(", ")
        )),
        None => out.push_str(&format!(
            "{} no project here, checking {}\n",
            ui.bold_out("undra doctor:"),
            names.join(", ")
        )),
    }
    let mut counts = [0_usize; 4];
    for section in sections {
        out.push_str(&format!("\n{}\n", ui.bold_out(section.title)));
        for f in &section.findings {
            let tag = match f.status {
                Status::Ok => ui.green_out("  ok   "),
                Status::Warn => ui.yellow_out("  warn "),
                Status::Fail => ui.red_out("  FAIL "),
                Status::Skip => ui.dim_out("  skip "),
            };
            counts[f.status as usize] += 1;
            out.push_str(&format!("{tag} {}\n", f.message));
            if !f.fix.is_empty() {
                out.push_str(&format!("         fix: {}\n", f.fix));
            }
        }
    }
    out.push_str(&format!(
        "\n{} ok, {} warning{}, {} failure{}\n",
        counts[Status::Ok as usize],
        counts[Status::Warn as usize],
        if counts[Status::Warn as usize] == 1 {
            ""
        } else {
            "s"
        },
        counts[Status::Fail as usize],
        if counts[Status::Fail as usize] == 1 {
            ""
        } else {
            "s"
        },
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    fn find<'a>(sections: &'a [Section], title: &str, needle: &str) -> &'a Finding {
        sections
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("no section {title}"))
            .findings
            .iter()
            .find(|f| f.message.contains(needle))
            .unwrap_or_else(|| panic!("no finding with {needle:?} in {title}: {sections:#?}"))
    }

    /// A machine with a working Rust toolchain.
    fn with_rust(sys: FakeSys, targets: &[&str]) -> FakeSys {
        let mut sys = sys
            .with_tool("rustc", "/home/dev/.cargo/bin/rustc")
            .with_tool("cargo", "/home/dev/.cargo/bin/cargo")
            .with_output(
                "rustc",
                "--version",
                "rustc 1.98.1 (48a229cea 2026-09-01)\n",
            )
            .with_output(
                "cargo",
                "--version",
                "cargo 1.98.1 (797e8a9bc 2026-08-05)\n",
            )
            .with_output(
                "rustc",
                "--print sysroot",
                "/home/dev/.rustup/toolchains/stable\n",
            );
        for t in targets {
            sys = sys.with_dir(&format!(
                "/home/dev/.rustup/toolchains/stable/lib/rustlib/{t}"
            ));
        }
        sys
    }

    #[test]
    fn a_bare_machine_fails_with_fixes() {
        let sys = FakeSys::linux();
        let tc = Toolchain::detect(&sys);
        let sections = check(&sys, &tc, &[Platform::Web], &[]);
        assert_eq!(find(&sections, "Rust", "rustc").status, Status::Fail);
        assert!(find(&sections, "Rust", "rustc").fix.contains("rustup.rs"));
        let node = find(&sections, "Web", "node");
        assert_eq!(node.status, Status::Fail);
        assert!(node.fix.contains("nodejs.org"));
        assert_eq!(find(&sections, "Web", "wasm-opt").status, Status::Warn);
        assert!(find(&sections, "Web", "wasm32-unknown-unknown").status != Status::Ok);
    }

    #[test]
    fn a_complete_web_machine_is_all_ok() {
        let sys = with_rust(FakeSys::macos(), &["wasm32-unknown-unknown"])
            .with_tool("node", "/usr/local/bin/node")
            .with_tool("npm", "/usr/local/bin/npm")
            .with_tool("wasm-opt", "/opt/homebrew/bin/wasm-opt")
            .with_output("node", "--version", "v22.3.0\n")
            .with_output("wasm-opt", "--version", "wasm-opt version 133\n");
        let tc = Toolchain::detect(&sys);
        let sections = check(&sys, &tc, &[Platform::Web], &[]);
        for f in sections.iter().flat_map(|s| &s.findings) {
            assert_eq!(f.status, Status::Ok, "{f:?}");
        }
    }

    #[test]
    fn old_node_and_old_rust_are_failures() {
        let sys = with_rust(FakeSys::linux(), &["wasm32-unknown-unknown"])
            .with_output("rustc", "--version", "rustc 1.80.0 (abc 2024-07-01)\n")
            .with_tool("node", "/usr/bin/node")
            .with_output("node", "--version", "v18.19.0\n");
        let tc = Toolchain::detect(&sys);
        let sections = check(&sys, &tc, &[Platform::Web], &[]);
        assert_eq!(find(&sections, "Rust", "1.80.0").status, Status::Fail);
        assert!(
            find(&sections, "Rust", "1.80.0")
                .fix
                .contains("rustup update")
        );
        assert_eq!(find(&sections, "Web", "v18").status, Status::Fail);
    }

    #[test]
    fn ios_needs_a_mac() {
        let sys = with_rust(FakeSys::linux(), &[]);
        let tc = Toolchain::detect(&sys);
        let sections = check(&sys, &tc, &[Platform::Ios], &[]);
        assert_eq!(find(&sections, "iOS", "macOS").status, Status::Skip);
    }

    #[test]
    fn xcode_select_on_the_command_line_tools_is_a_warning_with_the_fix() {
        let dev = crate::toolchain::XCODE_DEVELOPER_DIR;
        let sys = with_rust(
            FakeSys::macos(),
            &["aarch64-apple-ios", "aarch64-apple-ios-sim"],
        )
        .with_dir(dev)
        .with_tool("xcodebuild", "/usr/bin/xcodebuild")
        .with_tool("xcode-select", "/usr/bin/xcode-select")
        .with_tool("lipo", "/usr/bin/lipo")
        .with_output(
            "xcodebuild",
            "-version",
            "Xcode 26.6\nBuild version 17F113\n",
        )
        .with_output(
            "xcode-select",
            "-p",
            "/Library/Developer/CommandLineTools\n",
        );
        let tc = Toolchain::detect(&sys);
        let sections = check(&sys, &tc, &[Platform::Ios], &[]);
        let f = find(&sections, "iOS", "command line tools");
        assert_eq!(f.status, Status::Warn);
        assert!(
            f.fix
                .contains("sudo xcode-select -s /Applications/Xcode.app"),
            "{f:?}"
        );
        assert_eq!(find(&sections, "iOS", "Xcode 26.6").status, Status::Ok);
        assert_eq!(
            find(&sections, "iOS", "aarch64-apple-ios-sim").status,
            Status::Ok
        );
    }

    #[test]
    fn a_missing_ios_rust_target_says_how_to_add_it() {
        let sys = with_rust(FakeSys::macos(), &[]).with_tool("lipo", "/usr/bin/lipo");
        let tc = Toolchain::detect(&sys);
        let sections = check(&sys, &tc, &[Platform::Ios], &[]);
        let f = find(&sections, "iOS", "aarch64-apple-ios ");
        assert_eq!(f.status, Status::Fail);
        assert_eq!(f.fix, "rustup target add aarch64-apple-ios");
        assert_eq!(
            find(&sections, "iOS", "xcodebuild was not found").status,
            Status::Fail
        );
    }

    #[test]
    fn android_findings_cover_sdk_ndk_cargo_ndk_targets_and_jdk() {
        let sdk = "/opt/homebrew/share/android-commandlinetools";
        let sys = with_rust(FakeSys::macos(), &["aarch64-linux-android"])
            .with_dir(&format!("{sdk}/ndk/27.2.12479018"))
            .with_dir(&format!("{sdk}/platforms/android-35"))
            .with_file("/opt/homebrew/opt/openjdk@17/bin/java")
            .with_tool("cargo-ndk", "/home/dev/.cargo/bin/cargo-ndk")
            .with_output("cargo", "ndk --version", "cargo-ndk 4.1.2\n");
        let tc = Toolchain::detect(&sys);
        let abis = vec!["arm64-v8a".to_owned(), "x86_64".to_owned()];
        let sections = check(&sys, &tc, &[Platform::Android], &abis);
        assert_eq!(
            find(&sections, "Android", "ANDROID_HOME is not set").status,
            Status::Warn
        );
        assert_eq!(find(&sections, "Android", "NDK r27").status, Status::Ok);
        assert_eq!(
            find(&sections, "Android", "cargo-ndk 4.1.2").status,
            Status::Ok
        );
        assert_eq!(
            find(&sections, "Android", "aarch64-linux-android").status,
            Status::Ok
        );
        let missing = find(&sections, "Android", "x86_64-linux-android");
        assert_eq!(missing.status, Status::Fail);
        assert_eq!(missing.fix, "rustup target add x86_64-linux-android");
        let jdk = find(&sections, "Android", "JDK");
        assert_eq!(jdk.status, Status::Warn);
        assert!(
            jdk.fix.contains("JAVA_HOME=/opt/homebrew/opt/openjdk@17"),
            "{jdk:?}"
        );
    }

    #[test]
    fn android_without_anything_explains_each_piece() {
        let sys = with_rust(FakeSys::linux(), &[]);
        let tc = Toolchain::detect(&sys);
        let sections = check(&sys, &tc, &[Platform::Android], &["arm64-v8a".to_owned()]);
        assert!(
            find(&sections, "Android", "no Android SDK")
                .fix
                .contains("ANDROID_HOME")
        );
        assert!(
            find(&sections, "Android", "no Android NDK")
                .fix
                .contains("sdkmanager")
        );
        assert_eq!(
            find(&sections, "Android", "cargo-ndk").fix,
            "cargo install cargo-ndk"
        );
    }

    #[test]
    fn an_old_ndk_fails() {
        let sys = with_rust(FakeSys::linux(), &[])
            .with_dir("/sdk/ndk/25.2.9519653")
            .with_env("ANDROID_HOME", "/sdk");
        let tc = Toolchain::detect(&sys);
        let sections = check(&sys, &tc, &[Platform::Android], &[]);
        assert_eq!(find(&sections, "Android", "r25").status, Status::Fail);
    }

    #[test]
    fn java_versions_are_read() {
        assert_eq!(
            java_major("openjdk version \"17.0.12\" 2024-07-16"),
            Some(17)
        );
        assert_eq!(java_major("java version \"1.8.0_292\""), Some(8));
        assert_eq!(java_major("nonsense"), None);
    }

    #[test]
    fn the_report_counts_and_shows_fixes() {
        let sections = vec![Section {
            title: "Web",
            findings: vec![
                Finding::ok("node v22"),
                Finding::fail("wasm-opt missing", "brew install binaryen"),
            ],
        }];
        let text = render(&sections, &Ui::plain(), Some("todo"), &[Platform::Web]);
        assert!(text.contains("checking todo (web)"), "{text}");
        assert!(text.contains("  ok    node v22"), "{text}");
        assert!(
            text.contains("  FAIL  wasm-opt missing\n         fix: brew install binaryen"),
            "{text}"
        );
        assert!(text.contains("1 ok, 0 warnings, 1 failure"), "{text}");
    }
}
