//! The Rust checks: `rustup`, the toolchain, `rustc`, `cargo` and the cross-compilation targets.

use std::path::PathBuf;

use super::finding::{Check, Finding, State};
use super::{Context, version_line};

/// The command rustup's own site gives for installing it.
pub const RUSTUP_INSTALL: &str =
    "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y";

/// `rustup`: how the cross targets are installed.
pub const RUSTUP: Check = Check::new("rust.rustup", "rust");
/// The channel `rustup` has active.
pub const TOOLCHAIN: Check = Check::new("rust.toolchain", "rust");
/// `rustc`, at the workspace's MSRV or newer.
pub const RUSTC: Check = Check::new("rust.rustc", "rust");
/// `cargo`.
pub const CARGO: Check = Check::new("rust.cargo", "rust");

/// `wasm32-unknown-unknown`: the web build.
pub const TARGET_WASM: Check = Check::new("rust.target.wasm32-unknown-unknown", "rust-targets");
/// `aarch64-apple-ios`: iOS devices.
pub const TARGET_IOS_DEVICE: Check = Check::new("rust.target.aarch64-apple-ios", "rust-targets");
/// `aarch64-apple-ios-sim`: the simulator on Apple silicon.
pub const TARGET_IOS_SIM_ARM: Check =
    Check::new("rust.target.aarch64-apple-ios-sim", "rust-targets");
/// `x86_64-apple-ios`: the simulator on Intel Macs (`[ios] simulator_archs` with `x86_64`).
pub const TARGET_IOS_SIM_X86: Check = Check::new("rust.target.x86_64-apple-ios", "rust-targets");
/// `aarch64-linux-android`: the `arm64-v8a` ABI.
pub const TARGET_ANDROID_ARM64: Check =
    Check::new("rust.target.aarch64-linux-android", "rust-targets");
/// `x86_64-linux-android`: the `x86_64` ABI (the emulator on Intel and Linux).
pub const TARGET_ANDROID_X86_64: Check =
    Check::new("rust.target.x86_64-linux-android", "rust-targets");
/// `armv7-linux-androideabi`: the `armeabi-v7a` ABI.
pub const TARGET_ANDROID_ARMV7: Check =
    Check::new("rust.target.armv7-linux-androideabi", "rust-targets");
/// `i686-linux-android`: the `x86` ABI.
pub const TARGET_ANDROID_X86: Check = Check::new("rust.target.i686-linux-android", "rust-targets");

/// Every check of this module, for the tests that make sure each one has a test.
#[cfg(test)]
pub const ALL: &[Check] = &[
    RUSTUP,
    TOOLCHAIN,
    RUSTC,
    CARGO,
    TARGET_WASM,
    TARGET_IOS_DEVICE,
    TARGET_IOS_SIM_ARM,
    TARGET_IOS_SIM_X86,
    TARGET_ANDROID_ARM64,
    TARGET_ANDROID_X86_64,
    TARGET_ANDROID_ARMV7,
    TARGET_ANDROID_X86,
];

/// The oldest Rust that builds Undra: the workspace's `rust-version` (CLAUDE.md: MSRV 1.85).
#[must_use]
pub fn msrv() -> (u32, u32) {
    let text = env!("CARGO_PKG_RUST_VERSION");
    let mut parts = text.split('.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(1);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(85);
    (major, minor)
}

/// Parses `rustc 1.98.1 (...)` into `(1, 98)`.
#[must_use]
pub fn rust_version(line: &str) -> Option<(u32, u32)> {
    let version = line.split_whitespace().nth(1)?;
    let mut parts = version.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// The check that names `triple`.
#[must_use]
pub fn target_check(triple: &str) -> Check {
    match triple {
        "wasm32-unknown-unknown" => TARGET_WASM,
        "aarch64-apple-ios" => TARGET_IOS_DEVICE,
        "aarch64-apple-ios-sim" => TARGET_IOS_SIM_ARM,
        "x86_64-apple-ios" => TARGET_IOS_SIM_X86,
        "aarch64-linux-android" => TARGET_ANDROID_ARM64,
        "x86_64-linux-android" => TARGET_ANDROID_X86_64,
        "armv7-linux-androideabi" => TARGET_ANDROID_ARMV7,
        _ => TARGET_ANDROID_X86,
    }
}

/// Whether the standard library for `triple` is installed: `None` when `rustc` cannot say.
fn target_installed(cx: &Context<'_>, triple: &str) -> Option<bool> {
    let rustc = cx.toolchain.which(cx.sys, "rustc")?;
    let out = cx
        .sys
        .run(&rustc, &["--print", "sysroot"], &cx.toolchain.env_pairs())?;
    if !out.success {
        return None;
    }
    let sysroot = PathBuf::from(out.stdout.trim());
    Some(cx.sys.is_dir(&sysroot.join("lib/rustlib").join(triple)))
}

/// The finding for one Rust target. `needed_for` says what needs it; `optional` marks one that
/// only some configurations use (a missing optional target is a warning).
#[must_use]
pub fn target(cx: &Context<'_>, triple: &str, needed_for: &str, optional: bool) -> Finding {
    let mut check = target_check(triple);
    if optional {
        check = check.optional();
    }
    let add = format!("rustup target add {triple}");
    match target_installed(cx, triple) {
        Some(true) => check.ok(format!("Rust target {triple}")),
        Some(false) => check.missing(
            format!("Rust target {triple} is not installed ({needed_for})"),
            &[&add],
        ),
        None => check.warn(
            State::Missing,
            None,
            format!(
                "could not check the Rust target {triple} (`rustc --print sysroot` does not work)"
            ),
            &[RUSTUP_INSTALL],
        ),
    }
}

/// `rustup`, the channel, `rustc` and `cargo`.
#[must_use]
pub fn check(cx: &Context<'_>) -> Vec<Finding> {
    let mut out = Vec::new();
    let rustc = cx.toolchain.which(cx.sys, "rustc");
    let rustup = cx.toolchain.which(cx.sys, "rustup");

    match &rustup {
        Some(path) => {
            let version = version_line(cx, path, &["--version"]).unwrap_or_else(|| "rustup".to_owned());
            out.push(RUSTUP.ok(version));
        }
        None if rustc.is_some() => out.push(RUSTUP.warn(
            State::Missing,
            None,
            "rustup was not found: this Rust was installed another way, so the cross targets (iOS, Android, wasm) cannot be added with `rustup target add`",
            &[RUSTUP_INSTALL],
        )),
        None => out.push(RUSTUP.missing(
            "rustup was not found (it installs Rust and the cross targets Undra builds for)",
            &[RUSTUP_INSTALL],
        )),
    }

    if let Some(path) = &rustup {
        out.push(toolchain(cx, path));
    }

    match &rustc {
        None => out.push(RUSTC.missing(
            "rustc was not found",
            &[RUSTUP_INSTALL, "source \"$HOME/.cargo/env\""],
        )),
        Some(path) => match version_line(cx, path, &["--version"]) {
            Some(line) => {
                let msrv = msrv();
                match rust_version(&line) {
                    Some(v) if v >= msrv => out.push(RUSTC.ok(line)),
                    Some(_) => out.push(RUSTC.wrong_version(
                        line.clone(),
                        format!(
                            "{line}: Undra needs Rust {}.{} or newer (edition 2024)",
                            msrv.0, msrv.1
                        ),
                        &["rustup update stable"],
                    )),
                    None => out.push(RUSTC.warn(
                        State::WrongVersion,
                        Some(line.clone()),
                        format!("rustc prints an unexpected version: {line}"),
                        &["rustup update stable"],
                    )),
                }
            }
            None => out.push(RUSTC.fail(
                State::Missing,
                None,
                "rustc is installed but does not run",
                &["rustup self uninstall", RUSTUP_INSTALL],
            )),
        },
    }

    match cx.toolchain.which(cx.sys, "cargo") {
        Some(path) => match version_line(cx, &path, &["--version"]) {
            Some(line) => out.push(CARGO.ok(line)),
            None => out.push(CARGO.fail(
                State::Missing,
                None,
                "cargo is installed but does not run",
                &["rustup update stable"],
            )),
        },
        None => out.push(CARGO.missing("cargo was not found", &[RUSTUP_INSTALL])),
    }
    out
}

/// The channel `rustup` has active: Undra is built and tested on stable.
fn toolchain(cx: &Context<'_>, rustup: &std::path::Path) -> Finding {
    match version_line(cx, rustup, &["show", "active-toolchain"]) {
        Some(line) => {
            let name = line.split_whitespace().next().unwrap_or(&line).to_owned();
            let channel = name.split('-').next().unwrap_or("");
            if matches!(channel, "nightly" | "beta") {
                TOOLCHAIN.warn_version(
                    line.clone(),
                    format!("the active toolchain is {name}: Undra is built and tested on stable, and a {channel} compiler can break a build that stable runs"),
                    &["rustup default stable"],
                )
            } else {
                TOOLCHAIN.ok(format!("active toolchain: {line}"))
            }
        }
        None => TOOLCHAIN.warn(
            State::Missing,
            None,
            "rustup has no default toolchain",
            &["rustup default stable"],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;
    use crate::commands::doctor::finding::Status;

    #[test]
    fn the_msrv_is_the_workspace_rust_version() {
        assert_eq!(msrv(), (1, 85));
        assert_eq!(
            rust_version("rustc 1.98.1 (48a229cea 2026-09-01)"),
            Some((1, 98))
        );
        assert_eq!(
            rust_version("rustc 1.99.0-nightly (abc 2026-09-01)"),
            Some((1, 99))
        );
        assert_eq!(rust_version("nonsense"), None);
    }

    #[test]
    fn rustup_ok_missing_and_installed_another_way() {
        let good = scan(&good_machine(), &[]);
        let f = by_id(&good, "rust.rustup");
        assert_eq!(f.status, Status::Ok);
        assert!(
            f.observed.as_deref().unwrap().starts_with("rustup 1."),
            "{f:?}"
        );

        // No rustup and no rustc: a failure with the installer.
        let bare = scan(&bare_machine(), &[]);
        let f = by_id(&bare, "rust.rustup");
        assert_eq!((f.status, f.state), (Status::Fail, State::Missing));
        assert_eq!(f.fix, [RUSTUP_INSTALL]);

        // rustc from a distribution package: Rust works, the targets cannot be added.
        let sys = bare_machine()
            .with_tool("rustc", "/usr/bin/rustc")
            .with_output("rustc", "--version", "rustc 1.90.0 (abc 2026-01-01)\n");
        let f = by_id(&scan(&sys, &[]), "rust.rustup").clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::Missing));
        assert!(f.message.contains("rustup target add"), "{f:?}");
    }

    #[test]
    fn the_active_toolchain_should_be_stable() {
        let f = by_id(&scan(&good_machine(), &[]), "rust.toolchain").clone();
        assert_eq!(f.status, Status::Ok);
        assert!(f.message.contains("stable-aarch64-apple-darwin"), "{f:?}");

        let nightly = good_machine().with_output(
            "rustup",
            "show active-toolchain",
            "nightly-aarch64-apple-darwin (default)\n",
        );
        let f = by_id(&scan(&nightly, &[]), "rust.toolchain").clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::WrongVersion));
        assert_eq!(f.fix, ["rustup default stable"]);

        let pinned = good_machine().with_output(
            "rustup",
            "show active-toolchain",
            "1.85.0-aarch64-apple-darwin (overridden by '/p/rust-toolchain.toml')\n",
        );
        assert_eq!(
            by_id(&scan(&pinned, &[]), "rust.toolchain").status,
            Status::Ok
        );

        let none = good_machine().with_failing_output(
            "rustup",
            "show active-toolchain",
            "error: rustup could not choose a version of rustc to run",
        );
        let f = by_id(&scan(&none, &[]), "rust.toolchain").clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::Missing));
    }

    #[test]
    fn rustc_must_meet_the_msrv() {
        let f = by_id(&scan(&good_machine(), &[]), "rust.rustc").clone();
        assert_eq!(f.status, Status::Ok);
        assert!(
            f.observed.as_deref().unwrap().starts_with("rustc 1.98.1"),
            "{f:?}"
        );

        let old =
            good_machine().with_output("rustc", "--version", "rustc 1.80.0 (abc 2024-07-01)\n");
        let f = by_id(&scan(&old, &[]), "rust.rustc").clone();
        assert_eq!((f.status, f.state), (Status::Fail, State::WrongVersion));
        assert!(f.message.contains("1.85 or newer"), "{f:?}");
        assert_eq!(f.fix, ["rustup update stable"]);

        let odd = good_machine().with_output("rustc", "--version", "rustc unknown\n");
        assert_eq!(by_id(&scan(&odd, &[]), "rust.rustc").status, Status::Warn);

        let broken = good_machine().with_failing_output("rustc", "--version", "dyld: missing");
        assert_eq!(
            by_id(&scan(&broken, &[]), "rust.rustc").status,
            Status::Fail
        );

        let f = by_id(&scan(&bare_machine(), &[]), "rust.rustc").clone();
        assert_eq!((f.status, f.state), (Status::Fail, State::Missing));
        assert_eq!(f.fix[0], RUSTUP_INSTALL);
    }

    #[test]
    fn cargo_ok_missing_and_broken() {
        let f = by_id(&scan(&good_machine(), &[]), "rust.cargo").clone();
        assert_eq!(f.status, Status::Ok);
        let f = by_id(&scan(&bare_machine(), &[]), "rust.cargo").clone();
        assert_eq!((f.status, f.state), (Status::Fail, State::Missing));
        let broken = good_machine().with_failing_output("cargo", "--version", "broken");
        assert_eq!(
            by_id(&scan(&broken, &[]), "rust.cargo").status,
            Status::Fail
        );
    }

    #[test]
    fn every_target_check_names_its_triple() {
        for check in ALL.iter().filter(|c| c.id.starts_with("rust.target.")) {
            let triple = check.id.trim_start_matches("rust.target.");
            assert_eq!(target_check(triple).id, check.id);
        }
    }

    #[test]
    fn a_missing_target_says_how_to_add_it_and_an_unreadable_sysroot_is_a_warning() {
        let sys = good_machine_without_targets();
        let report = scan(&sys, &["web"]);
        let f = by_id(&report, "rust.target.wasm32-unknown-unknown").clone();
        assert_eq!((f.status, f.state), (Status::Fail, State::Missing));
        assert_eq!(f.fix, ["rustup target add wasm32-unknown-unknown"]);
        assert_eq!(f.anchor, "rust-targets");

        let no_sysroot = good_machine().with_failing_output("rustc", "--print sysroot", "boom");
        let f = by_id(
            &scan(&no_sysroot, &["web"]),
            "rust.target.wasm32-unknown-unknown",
        )
        .clone();
        assert_eq!(f.status, Status::Warn);
        assert!(f.message.contains("could not check"), "{f:?}");
    }
}
