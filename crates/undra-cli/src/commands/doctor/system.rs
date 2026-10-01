//! The checks that do not belong to one platform: free disk space and whether `undra` itself is
//! on `PATH` (the Gradle task, the Xcode build phase and the Vite plugin call it by name), and the
//! checks that are only for people working on Undra.

use std::path::{Path, PathBuf};

use crate::fsutil::human_size;
use crate::sys::Os;

use super::finding::{Check, Finding, State};
use super::{Context, version_line};

/// Free disk space where the builds go.
pub const DISK: Check = Check::new("system.disk", "disk-space").optional();
/// `undra` on `PATH`, at the version that is running.
pub const UNDRA_ON_PATH: Check = Check::new("system.undra-on-path", "undra-on-path").optional();
/// The Kotlin compiler, for the Kotlin runtime tests.
pub const KOTLINC: Check =
    Check::new("contributors.kotlinc", "kotlin-compiler-contributors").for_contributors();

/// Every check of this module.
#[cfg(test)]
pub const ALL: &[Check] = &[DISK, UNDRA_ON_PATH, KOTLINC];

/// Below this much free space (decimal gigabytes) doctor warns: Rust, Xcode and Gradle builds
/// each keep several gigabytes of caches.
pub const MIN_FREE_BYTES: u64 = 10_000_000_000;

/// The install command of `undra` (`docs/RELEASING.md`).
pub const INSTALL_UNDRA: &str = "curl -fsSL https://shreypdev.github.io/undra/install.sh | sh";

/// The directory the free space is measured on: the project, else the home directory.
fn disk_path(cx: &Context<'_>) -> PathBuf {
    cx.project
        .map(Path::to_path_buf)
        .or_else(|| cx.sys.home())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The disk space and `PATH` findings.
#[must_use]
pub fn check(cx: &Context<'_>) -> Vec<Finding> {
    vec![disk(cx), undra_on_path(cx)]
}

fn disk(cx: &Context<'_>) -> Finding {
    let path = disk_path(cx);
    let Some(free) = cx.sys.free_disk_bytes(&path) else {
        return DISK.skip("could not tell how much disk space is free");
    };
    let shown = human_size(free);
    if free >= MIN_FREE_BYTES {
        return DISK.ok_with(
            format!("{shown} free on the disk of {}", path.display()),
            shown,
        );
    }
    let mut fix = vec!["cargo clean"];
    if cx.sys.os() == Os::Macos {
        fix.push("xcrun simctl delete unavailable");
        fix.push("rm -rf \"$HOME/Library/Developer/Xcode/DerivedData\"");
    }
    DISK.warn(
        State::WrongVersion,
        Some(shown.clone()),
        format!(
            "only {shown} free on the disk of {} (Rust, Xcode and Gradle builds keep several gigabytes of caches; {} is the comfortable minimum)",
            path.display(),
            human_size(MIN_FREE_BYTES)
        ),
        &fix,
    )
}

fn undra_on_path(cx: &Context<'_>) -> Finding {
    let own = crate::version::SEMVER;
    let Some(found) = cx.toolchain.which(cx.sys, "undra") else {
        return UNDRA_ON_PATH.warn_missing(
            "`undra` is not on PATH: the Gradle task, the Xcode build phase and the Vite plugin of a generated project run it by name",
            &[INSTALL_UNDRA, "export PATH=\"$HOME/.undra/bin:$PATH\""],
        );
    };
    let line = version_line(cx, &found, &["--version"]).unwrap_or_else(|| "undra".to_owned());
    let on_path_version = line.split_whitespace().nth(1).unwrap_or("");
    if on_path_version.is_empty() || on_path_version == own {
        UNDRA_ON_PATH.ok_with(format!("{line} at {}", found.display()), line)
    } else {
        UNDRA_ON_PATH.warn_version(
            line.clone(),
            format!(
                "the `undra` on PATH ({}) is {on_path_version}, not the {own} that is running; the build integrations of a project run the one on PATH",
                found.display()
            ),
            &[INSTALL_UNDRA],
        )
    }
}

/// The checks for people who work on Undra itself. For everyone else a missing tool is not a
/// problem, so it is reported as not applicable.
#[must_use]
pub fn contributors(cx: &Context<'_>) -> Vec<Finding> {
    vec![kotlinc(cx)]
}

fn kotlinc(cx: &Context<'_>) -> Finding {
    let Some(tool) = cx.toolchain.which(cx.sys, "kotlinc") else {
        if !cx.contributor {
            return KOTLINC.skip(
                "kotlinc (for contributors) is not installed: only the Kotlin runtime tests of the Undra repository use it, an app does not",
            );
        }
        let install = if cx.sys.os() == Os::Macos {
            "brew install kotlin"
        } else {
            "curl -s https://get.sdkman.io | bash && sdk install kotlin"
        };
        return KOTLINC.warn_missing(
            "kotlinc (for contributors) was not found: the Kotlin runtime tests (`runtimes/kotlin/undra-runtime/scripts/test-local.sh`) compile with it",
            &[install],
        );
    };
    let line = version_line(cx, &tool, &["-version"])
        .map(|l| l.trim_start_matches("info: ").to_owned())
        .unwrap_or_else(|| "kotlinc".to_owned());
    KOTLINC.ok_with(
        format!("{line} (for contributors: the Kotlin runtime tests)"),
        line,
    )
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;
    use crate::commands::doctor::finding::{Audience, Status};

    #[test]
    fn disk_space_warns_under_ten_gigabytes() {
        let f = by_id(&scan(&good_machine(), &[]), "system.disk").clone();
        assert_eq!(f.status, Status::Ok);
        assert!(
            f.message.contains("free on the disk of /Users/dev"),
            "{f:?}"
        );

        let low = good_machine().with_free_disk(4_200_000_000);
        let f = by_id(&scan(&low, &[]), "system.disk").clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::WrongVersion));
        assert!(
            f.message.contains("4.2 GB") && f.message.contains("10.0 GB"),
            "{f:?}"
        );
        assert_eq!(f.fix[0], "cargo clean");
        assert!(
            f.fix
                .iter()
                .any(|c| c.contains("simctl delete unavailable")),
            "{f:?}"
        );

        // Exactly at the limit is fine; a Linux box gets no Xcode advice.
        let edge = good_linux_machine().with_free_disk(MIN_FREE_BYTES);
        assert_eq!(by_id(&scan(&edge, &[]), "system.disk").status, Status::Ok);
        let low_linux = good_linux_machine().with_free_disk(1);
        assert_eq!(
            by_id(&scan(&low_linux, &[]), "system.disk").fix,
            ["cargo clean"]
        );

        let unknown = crate::sys::fake::FakeSys::linux();
        assert_eq!(
            by_id(&scan(&unknown, &[]), "system.disk").status,
            Status::Skip
        );
    }

    #[test]
    fn the_disk_is_measured_where_the_project_is() {
        let report = run_in_project(&good_machine(), &["web"], &["arm64"]);
        let f = by_id(&report, "system.disk");
        assert!(f.message.contains("/work/app"), "{f:?}");
    }

    #[test]
    fn undra_must_be_on_path_at_the_running_version() {
        let f = by_id(&scan(&good_machine(), &[]), "system.undra-on-path").clone();
        assert_eq!(f.status, Status::Ok);
        assert!(f.message.contains("/Users/dev/.undra/bin/undra"), "{f:?}");

        let f = by_id(&scan(&bare_mac(), &[]), "system.undra-on-path").clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::Missing));
        assert_eq!(f.fix[0], INSTALL_UNDRA);
        assert!(
            f.message.contains("Gradle task") && f.message.contains("Xcode build phase"),
            "{f:?}"
        );

        let old = good_machine().with_output("undra", "--version", "undra 0.0.9 (abc1234)\n");
        let f = by_id(&scan(&old, &[]), "system.undra-on-path").clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::WrongVersion));
        assert!(f.message.contains("0.0.9"), "{f:?}");
    }

    #[test]
    fn kotlinc_is_marked_for_contributors() {
        let f = by_id(&scan(&good_machine(), &[]), "contributors.kotlinc").clone();
        assert_eq!(f.status, Status::Ok);
        assert_eq!(f.audience, Audience::Contributors);
        assert!(f.message.contains("(for contributors"), "{f:?}");
        assert_eq!(
            f.observed.as_deref(),
            Some("kotlinc-jvm 2.4.20 (JRE 17.0.20.1+0)")
        );

        // Not a contributor and not installed: nothing to do.
        let f = by_id(&scan(&bare_mac(), &[]), "contributors.kotlinc").clone();
        assert_eq!((f.status, f.state), (Status::Skip, State::NotApplicable));
        assert!(f.fix.is_empty());

        // A contributor without it gets the install.
        let f = by_id(
            &run_as_contributor(&bare_mac(), &[]),
            "contributors.kotlinc",
        )
        .clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::Missing));
        assert_eq!(f.fix, ["brew install kotlin"]);
        let f = by_id(
            &run_as_contributor(&bare_machine(), &[]),
            "contributors.kotlinc",
        )
        .clone();
        assert!(f.fix[0].contains("sdkman"), "{f:?}");
    }
}
