//! The iOS checks: full Xcode, where `xcode-select` points, `lipo`, a simulator runtime and the
//! Rust targets `undra build --platform ios` compiles for.

use std::path::Path;

use crate::sys::Os;
use crate::toolchain::XCODE_DEVELOPER_DIR;

use super::Context;
use super::finding::{Check, Finding, State};
use super::rust::target;

/// iOS builds need a Mac.
pub const PLATFORM: Check = Check::new("ios.platform", "xcode");
/// Full Xcode (not only the command line tools), at a version the generated project opens.
pub const XCODE: Check = Check::new("ios.xcode", "xcode");
/// Where `xcode-select` points, and what `undra` does about it.
pub const XCODE_SELECT: Check = Check::new("ios.xcode-select", "xcode");
/// `lipo`, which folds the simulator slices together.
pub const LIPO: Check = Check::new("ios.lipo", "xcode");
/// At least one iOS simulator runtime, to run the app.
pub const SIMULATOR_RUNTIME: Check = Check::new("ios.simulator-runtime", "ios-simulator-runtime");

/// Every check of this module (the Rust targets are in [`super::rust::ALL`]).
#[cfg(test)]
pub const ALL: &[Check] = &[PLATFORM, XCODE, XCODE_SELECT, LIPO, SIMULATOR_RUNTIME];

/// The oldest Xcode that opens the generated project (`objectVersion 77`, synchronized folders).
pub const MIN_XCODE: u32 = 16;

/// The command that opens Xcode's page in the App Store.
const XCODE_APP_STORE: &str = "open macappstore://apps.apple.com/app/xcode/id497799835";
const SELECT_XCODE: &str = "sudo xcode-select -s /Applications/Xcode.app/Contents/Developer";
const ACCEPT_LICENSE: &str = "sudo xcodebuild -license accept";

/// `Xcode 26.6` into `26`.
#[must_use]
pub fn xcode_major(line: &str) -> Option<u32> {
    line.split_whitespace()
        .nth(1)?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// The `iOS` simulator runtimes in the output of `xcrun simctl list runtimes`, as `iOS 26.5`.
#[must_use]
pub fn ios_runtimes(listing: &str) -> Vec<String> {
    listing
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("iOS ") && !l.contains("unavailable"))
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            Some(format!("{} {}", words.next()?, words.next()?))
        })
        .collect()
}

/// Every iOS finding.
#[must_use]
pub fn check(cx: &Context<'_>) -> Vec<Finding> {
    if cx.sys.os() != Os::Macos {
        return vec![PLATFORM.skip(
            "iOS builds need macOS (Xcode); build the other platforms here and iOS on a Mac or in CI",
        )];
    }
    let mut out = vec![xcode(cx), xcode_select(cx)];
    if cx.toolchain.which(cx.sys, "lipo").is_none() {
        out.push(LIPO.missing("lipo was not found", &["xcode-select --install"]));
    } else {
        out.push(LIPO.ok("lipo"));
    }
    out.push(simulator_runtime(cx));
    out.push(target(cx, "aarch64-apple-ios", "iOS devices", false));
    let archs = cx.simulator_archs;
    if archs.iter().any(|a| a == "arm64") {
        out.push(target(
            cx,
            "aarch64-apple-ios-sim",
            "the simulator on Apple silicon",
            false,
        ));
    }
    if archs.iter().any(|a| a == "x86_64") {
        out.push(target(
            cx,
            "x86_64-apple-ios",
            "the simulator on Intel Macs ([ios] simulator_archs has x86_64)",
            false,
        ));
    } else if cx.project.is_none() {
        out.push(target(
            cx,
            "x86_64-apple-ios",
            "only if a project lists x86_64 in [ios] simulator_archs",
            true,
        ));
    }
    out
}

fn xcode(cx: &Context<'_>) -> Finding {
    let Some(xcodebuild) = cx.toolchain.which(cx.sys, "xcodebuild") else {
        return XCODE.missing(
            "xcodebuild was not found: iOS builds need the full Xcode app",
            &[XCODE_APP_STORE, SELECT_XCODE, ACCEPT_LICENSE],
        );
    };
    let env = cx.toolchain.env_pairs();
    match cx.sys.run(&xcodebuild, &["-version"], &env) {
        Some(out) if out.success => {
            let line = out.text().lines().next().unwrap_or("").trim().to_owned();
            match xcode_major(&line) {
                Some(major) if major < MIN_XCODE => XCODE.wrong_version(
                    line.clone(),
                    format!("{line}: the generated Xcode project needs Xcode {MIN_XCODE} or newer"),
                    &[XCODE_APP_STORE],
                ),
                _ => XCODE.ok(line),
            }
        }
        Some(out) if out.text().to_ascii_lowercase().contains("license") => XCODE.fail(
            State::Missing,
            None,
            "Xcode is installed but its license has not been accepted, so xcodebuild refuses to run",
            &[ACCEPT_LICENSE],
        ),
        _ => XCODE.fail(
            State::Missing,
            None,
            "xcodebuild does not work (only the command line tools are installed)",
            &[XCODE_APP_STORE, SELECT_XCODE, ACCEPT_LICENSE],
        ),
    }
}

fn xcode_select(cx: &Context<'_>) -> Finding {
    let selected = cx
        .toolchain
        .which(cx.sys, "xcode-select")
        .and_then(|tool| cx.sys.run(&tool, &["-p"], &[]))
        .map(|o| o.stdout.trim().to_owned())
        .unwrap_or_default();
    if let Some(dir) = cx.sys.env("DEVELOPER_DIR") {
        return XCODE_SELECT.ok_with(format!("DEVELOPER_DIR is set to {dir}"), dir);
    }
    if selected.ends_with("CommandLineTools") {
        if cx.sys.is_dir(Path::new(XCODE_DEVELOPER_DIR)) {
            return XCODE_SELECT.warn(
                State::WrongVersion,
                Some(selected),
                "xcode-select points at the command line tools although Xcode is installed (undra uses DEVELOPER_DIR for its own builds; xcodebuild and Gradle's tools do not)",
                &[SELECT_XCODE],
            );
        }
        return XCODE_SELECT.fail(
            State::Missing,
            Some(selected),
            "xcode-select points at the command line tools, and Xcode is not installed",
            &[XCODE_APP_STORE, SELECT_XCODE],
        );
    }
    if selected.is_empty() {
        return XCODE_SELECT.skip("xcode-select did not say where the developer directory is");
    }
    XCODE_SELECT.ok_with(format!("xcode-select: {selected}"), selected)
}

fn simulator_runtime(cx: &Context<'_>) -> Finding {
    let download = "xcodebuild -downloadPlatform iOS";
    let Some(xcrun) = cx.toolchain.which(cx.sys, "xcrun") else {
        return SIMULATOR_RUNTIME.warn_missing(
            "xcrun was not found, so the iOS simulator runtimes cannot be listed",
            &["xcode-select --install"],
        );
    };
    let Some(listing) = version_text(cx, &xcrun, &["simctl", "list", "runtimes"]) else {
        return SIMULATOR_RUNTIME.warn(
            State::Missing,
            None,
            "could not list the iOS simulator runtimes (`xcrun simctl` needs the full Xcode)",
            &[download],
        );
    };
    let runtimes = ios_runtimes(&listing);
    if runtimes.is_empty() {
        SIMULATOR_RUNTIME.warn_missing(
            "no iOS simulator runtime is installed (needed to run the app on a simulator, not to build it)",
            &[download],
        )
    } else {
        SIMULATOR_RUNTIME.ok_with(
            format!("iOS simulator runtime: {}", runtimes.join(", ")),
            runtimes.join(", "),
        )
    }
}

/// The whole output of a command that succeeded.
fn version_text(cx: &Context<'_>, program: &Path, args: &[&str]) -> Option<String> {
    let out = cx.sys.run(program, args, &cx.toolchain.env_pairs())?;
    out.success.then(|| out.text())
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;
    use crate::commands::doctor::finding::Status;

    #[test]
    fn xcode_versions_and_runtimes_are_parsed() {
        assert_eq!(xcode_major("Xcode 26.6"), Some(26));
        assert_eq!(xcode_major("Xcode 15.4"), Some(15));
        assert_eq!(xcode_major("nonsense"), None);
        let listing = "== Runtimes ==\niOS 26.5 (26.5 - 23F77) - com.apple.CoreSimulator.SimRuntime.iOS-26-5\niOS 17.0 (17.0 - 21A328) - com.apple.CoreSimulator.SimRuntime.iOS-17-0 (unavailable, runtime profile not found)\ntvOS 26.0 (26.0 - 23J1) - com.apple.CoreSimulator.SimRuntime.tvOS-26-0\n";
        assert_eq!(ios_runtimes(listing), ["iOS 26.5"]);
    }

    #[test]
    fn ios_needs_a_mac() {
        let report = scan(&good_linux_machine(), &["ios"]);
        let f = by_id(&report, "ios.platform");
        assert_eq!((f.status, f.state), (Status::Skip, State::NotApplicable));
        assert!(f.message.contains("macOS"), "{f:?}");
        assert!(
            report
                .findings()
                .all(|f| !f.id.starts_with("ios.") || f.id == "ios.platform")
        );
    }

    #[test]
    fn xcode_ok_old_missing_unlicensed_and_command_line_tools_only() {
        let f = by_id(&scan(&good_machine(), &["ios"]), "ios.xcode").clone();
        assert_eq!(f.status, Status::Ok);
        assert_eq!(f.observed.as_deref(), Some("Xcode 26.6"));

        let old = good_machine().with_output(
            "xcodebuild",
            "-version",
            "Xcode 15.4\nBuild version 15F31d\n",
        );
        let f = by_id(&scan(&old, &["ios"]), "ios.xcode").clone();
        assert_eq!((f.status, f.state), (Status::Fail, State::WrongVersion));
        assert!(f.message.contains("Xcode 16 or newer"), "{f:?}");

        let missing = bare_mac();
        let f = by_id(&scan(&missing, &["ios"]), "ios.xcode").clone();
        assert_eq!((f.status, f.state), (Status::Fail, State::Missing));
        assert!(
            f.fix.iter().any(|c| c.starts_with("open macappstore://")),
            "{f:?}"
        );
        assert!(
            f.fix
                .iter()
                .any(|c| c == "sudo xcode-select -s /Applications/Xcode.app/Contents/Developer"),
            "{f:?}"
        );

        let unlicensed = good_machine().with_failing_output(
            "xcodebuild",
            "-version",
            "Agreeing to the Xcode/iOS license requires admin privileges, please run \"sudo xcodebuild -license\"",
        );
        let f = by_id(&scan(&unlicensed, &["ios"]), "ios.xcode").clone();
        assert_eq!(f.status, Status::Fail);
        assert_eq!(f.fix, ["sudo xcodebuild -license accept"]);

        let stub = good_machine().with_failing_output(
            "xcodebuild",
            "-version",
            "xcode-select: error: tool 'xcodebuild' requires Xcode",
        );
        let f = by_id(&scan(&stub, &["ios"]), "ios.xcode").clone();
        assert!(f.message.contains("only the command line tools"), "{f:?}");
    }

    #[test]
    fn xcode_select_pointing_at_the_command_line_tools_is_explained() {
        let clt = good_machine().with_dir(XCODE_DEVELOPER_DIR).with_output(
            "xcode-select",
            "-p",
            "/Library/Developer/CommandLineTools\n",
        );
        let f = by_id(&scan(&clt, &["ios"]), "ios.xcode-select").clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::WrongVersion));
        assert_eq!(
            f.fix,
            ["sudo xcode-select -s /Applications/Xcode.app/Contents/Developer"]
        );
        assert_eq!(
            f.observed.as_deref(),
            Some("/Library/Developer/CommandLineTools")
        );

        // Without Xcode installed it is a failure.
        let no_xcode = bare_mac()
            .with_tool("xcode-select", "/usr/bin/xcode-select")
            .with_output(
                "xcode-select",
                "-p",
                "/Library/Developer/CommandLineTools\n",
            );
        let f = by_id(&scan(&no_xcode, &["ios"]), "ios.xcode-select").clone();
        assert_eq!(f.status, Status::Fail);

        // A correct selection, an explicit DEVELOPER_DIR, and a machine that cannot say.
        let f = by_id(&scan(&good_machine(), &["ios"]), "ios.xcode-select").clone();
        assert_eq!(f.status, Status::Ok);
        assert!(
            f.message
                .starts_with("xcode-select: /Applications/Xcode.app"),
            "{f:?}"
        );
        let explicit =
            good_machine().with_env("DEVELOPER_DIR", "/Somewhere/Xcode.app/Contents/Developer");
        let f = by_id(&scan(&explicit, &["ios"]), "ios.xcode-select").clone();
        assert!(
            f.message.contains("DEVELOPER_DIR is set to /Somewhere"),
            "{f:?}"
        );
        let silent = bare_mac();
        assert_eq!(
            by_id(&scan(&silent, &["ios"]), "ios.xcode-select").status,
            Status::Skip
        );
    }

    #[test]
    fn lipo_is_checked() {
        assert_eq!(
            by_id(&scan(&good_machine(), &["ios"]), "ios.lipo").status,
            Status::Ok
        );
        let f = by_id(&scan(&bare_mac(), &["ios"]), "ios.lipo").clone();
        assert_eq!((f.status, f.state), (Status::Fail, State::Missing));
        assert_eq!(f.fix, ["xcode-select --install"]);
    }

    #[test]
    fn a_simulator_runtime_is_needed_to_run_the_app() {
        let f = by_id(&scan(&good_machine(), &["ios"]), "ios.simulator-runtime").clone();
        assert_eq!(f.status, Status::Ok);
        assert_eq!(f.observed.as_deref(), Some("iOS 26.5"));

        let none = good_machine().with_output("xcrun", "simctl list runtimes", "== Runtimes ==\n");
        let f = by_id(&scan(&none, &["ios"]), "ios.simulator-runtime").clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::Missing));
        assert_eq!(f.fix, ["xcodebuild -downloadPlatform iOS"]);

        let stub = good_machine().with_failing_output(
            "xcrun",
            "simctl list runtimes",
            "unable to find utility \"simctl\"",
        );
        let f = by_id(&scan(&stub, &["ios"]), "ios.simulator-runtime").clone();
        assert!(f.message.contains("could not list"), "{f:?}");

        let no_xcrun = bare_mac();
        let f = by_id(&scan(&no_xcrun, &["ios"]), "ios.simulator-runtime").clone();
        assert!(f.message.contains("xcrun was not found"), "{f:?}");
    }

    #[test]
    fn the_rust_targets_follow_the_simulator_architectures() {
        // The default project: the device and the arm64 simulator; the Intel slice only as an option.
        let report = scan(&good_machine(), &["ios"]);
        assert_eq!(
            by_id(&report, "rust.target.aarch64-apple-ios").status,
            Status::Ok
        );
        assert_eq!(
            by_id(&report, "rust.target.aarch64-apple-ios-sim").status,
            Status::Ok
        );
        let x86 = by_id(&report, "rust.target.x86_64-apple-ios");
        assert_eq!((x86.status, x86.optional), (Status::Warn, true));

        // A project that asks for both slices needs the Intel target.
        let report = run_in_project(&good_machine(), &["ios"], &["arm64", "x86_64"]);
        let x86 = by_id(&report, "rust.target.x86_64-apple-ios");
        assert_eq!((x86.status, x86.optional), (Status::Fail, false));
        assert_eq!(x86.fix, ["rustup target add x86_64-apple-ios"]);

        // A project with only the Intel simulator does not need the arm64 one.
        let report = run_in_project(&good_machine(), &["ios"], &["x86_64"]);
        assert!(
            report
                .findings()
                .all(|f| f.id != "rust.target.aarch64-apple-ios-sim")
        );

        // Nothing installed: the device target fails with its fix.
        let report = scan(&good_machine_without_targets(), &["ios"]);
        let f = by_id(&report, "rust.target.aarch64-apple-ios");
        assert_eq!((f.status, f.state), (Status::Fail, State::Missing));
    }
}
