//! The iOS build: the shim as a static library for device and simulator, packaged as an
//! XCFramework.
//!
//! `KeelCore.xcframework` holds one static library per platform (`ios-arm64`,
//! `ios-arm64-simulator`; an x86_64 simulator slice is folded into the simulator one when
//! `[ios] simulator_archs` asks for it) and **no header and no module map**: the Swift runtime's
//! `KeelFFI` target already declares the C ABI as the module `KeelFFI`, and a second definition in
//! the XCFramework fails the app's build with "redefinition of module 'KeelFFI'". The app links the
//! XCFramework and builds the runtime package with `KEEL_LINK_CORE=1`, so the runtime's link-time
//! stand-ins do not shadow the real core.
//!
//! **Debug builds and `-force_load`.** The core's `#[keel::api]` registrations are static
//! constructors in object files that nothing references. A release build (`lto = "fat"`, one
//! codegen unit) is a single object and links as is; a debug static library has many, and the
//! linker drops the ones nothing references, which leaves the schema empty. The generated Xcode
//! project therefore links the library with `-force_load` in every configuration.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::cargo::{Build, Profile};
use crate::error::{CliError, Code, Result};
use crate::fsutil::{create_dir_all, remove_dir_all, size_of};
use crate::session::Session;
use crate::sys::Os;
use crate::toolchain::Concern;

use super::{Artifact, unsupported};

/// Rust target of a physical device.
pub const DEVICE_TRIPLE: &str = "aarch64-apple-ios";

/// The Rust target of a simulator architecture.
#[must_use]
pub fn simulator_triple(arch: &str) -> &'static str {
    if arch == "x86_64" {
        "x86_64-apple-ios"
    } else {
        "aarch64-apple-ios-sim"
    }
}

/// Builds `build/ios/KeelCore.xcframework`.
///
/// # Errors
///
/// `C0012` off macOS, `C0003` without Xcode's tools, `C0011` without the Rust targets, `C0004`
/// when a build or `xcodebuild` fails.
pub fn build(session: &Session<'_>, release: bool) -> Result<Vec<Artifact>> {
    if session.sys.os() != Os::Macos {
        return Err(unsupported(
            "iOS",
            "iOS libraries are linked with Apple's toolchain, which exists only on macOS",
            "build the iOS slice on a Mac (in CI: a macos runner) and the others anywhere: `keel build --platform android,web`",
        ));
    }
    let xcodebuild = session.toolchain.which(session.sys, "xcodebuild").ok_or_else(|| {
        CliError::missing_tool(
            "xcodebuild",
            "creating the XCFramework",
            "install Xcode from the App Store, then `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`",
        )
    })?;
    let lipo = session
        .toolchain
        .which(session.sys, "lipo")
        .ok_or_else(|| {
            CliError::missing_tool(
                "lipo",
                "combining simulator slices",
                "install Xcode or the command line tools: `xcode-select --install`",
            )
        })?;
    let check = session
        .sys
        .run(&xcodebuild, &["-version"], &session.toolchain.env_pairs());
    if !check.as_ref().is_some_and(|out| out.success) {
        return Err(CliError::new(
            Code::MissingTool,
            "`xcodebuild` is installed but does not work",
            "it is the stub that ships with the command line tools: creating an XCFramework needs the full Xcode app",
            "install Xcode, then `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` (or set DEVELOPER_DIR)",
        ));
    }
    for note in session.toolchain.notes_for(Concern::Apple) {
        session.ui.detail(note);
    }

    let manifest = session.shim_manifest()?;
    let profile = if release {
        Profile::Release
    } else {
        Profile::Dev
    };
    let target_dir = session.target_dir();
    let stage =
        crate::shim::ios_stage_dir(&target_dir, &session.project.root).join(profile.dir_name());
    let env = vec![(
        "IPHONEOS_DEPLOYMENT_TARGET".to_owned(),
        session.project.config.ios.deployment_target.clone(),
    )];

    let staticlib = |triple: &str| -> Result<PathBuf> {
        session.ui.detail(&format!("rust target {triple}"));
        let files = session.cargo().build_library(&Build {
            manifest: manifest.clone(),
            target_dir: target_dir.clone(),
            triple: Some(triple.to_owned()),
            profile,
            crate_type: "staticlib",
            features: Vec::new(),
            env: env.clone(),
            lib_name: "keel_core".to_owned(),
            rustc_args: Vec::new(),
        })?;
        files
            .into_iter()
            .find(|f| f.extension().is_some_and(|e| e == "a"))
            .ok_or_else(|| {
                CliError::new(
                    Code::ToolFailed,
                    format!("cargo built for {triple} but produced no static library"),
                    "the iOS build links the shim as a .a",
                    "run `cargo clean` and try again; if it persists this is a bug in keel-cli",
                )
            })
    };

    // Device.
    let device = staticlib(DEVICE_TRIPLE)?;
    // Simulator: one library per architecture, folded into one with lipo.
    let mut sim_libs = Vec::new();
    for arch in &session.project.config.ios.simulator_archs {
        sim_libs.push(staticlib(simulator_triple(arch))?);
    }
    let sim = if sim_libs.len() == 1 {
        sim_libs.remove(0)
    } else {
        let fat = stage.join("simulator-fat/libkeel_core.a");
        create_dir_all(fat.parent().expect("has a parent"))?;
        let mut cmd = Command::new(&lipo);
        cmd.arg("-create")
            .args(&sim_libs)
            .arg("-output")
            .arg(&fat)
            .stdin(Stdio::null());
        session.toolchain.apply(&mut cmd);
        let status = cmd.status().map_err(|e| CliError::io("run", &lipo, &e))?;
        if !status.success() {
            return Err(CliError::tool_failed(
                "lipo",
                "combining the simulator slices",
                &status.to_string(),
            ));
        }
        fat
    };

    let out_dir = session.project.build_dir().join("ios");
    let xcframework = out_dir.join("KeelCore.xcframework");
    remove_dir_all(&xcframework)?;
    create_dir_all(&out_dir)?;
    session.ui.step("Creating KeelCore.xcframework");
    let mut cmd = Command::new(&xcodebuild);
    cmd.arg("-create-xcframework");
    for lib in [&device, &sim] {
        cmd.arg("-library").arg(lib);
    }
    cmd.arg("-output").arg(&xcframework).stdin(Stdio::null());
    session.toolchain.apply(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| CliError::io("run", &xcodebuild, &e))?;
    if !output.status.success() {
        return Err(CliError::tool_failed(
            "xcodebuild -create-xcframework",
            "packaging the libraries",
            &output.status.to_string(),
        )
        .with_detail(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let slice_note = |p: &Path| format!("{} in the archive", crate::fsutil::human_size(size_of(p)));
    Ok(vec![Artifact {
        label: "ios xcframework".to_owned(),
        size: size_of(&xcframework),
        path: xcframework.clone(),
        budget: Some(
            "900 KB arm64 added to an app (LTO, stripped); a static archive is an upper bound"
                .to_owned(),
        ),
        note: Some(format!(
            "device {}, simulator {}",
            slice_note(&device),
            slice_note(&sim)
        )),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulator_triples() {
        assert_eq!(simulator_triple("arm64"), "aarch64-apple-ios-sim");
        assert_eq!(simulator_triple("x86_64"), "x86_64-apple-ios");
    }

    #[test]
    fn the_runtime_declares_the_c_abi_module_itself() {
        // The XCFramework carries no header and no module map, and this is why: the Swift
        // runtime's `KeelFFI` target already defines the module `KeelFFI`, and a second
        // definition in the XCFramework fails the build ("redefinition of module 'KeelFFI'").
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let modulemap =
            repo.join("runtimes/swift/KeelRuntime/Sources/KeelFFI/include/module.modulemap");
        if !modulemap.is_file() {
            return; // built outside the repository (a packaged crate)
        }
        assert!(
            std::fs::read_to_string(modulemap)
                .unwrap()
                .contains("module KeelFFI")
        );
    }

    #[test]
    fn off_macos_the_error_points_at_the_other_platforms() {
        let e = unsupported("iOS", "why", "fix");
        assert_eq!(e.code, Code::Unsupported);
    }
}
