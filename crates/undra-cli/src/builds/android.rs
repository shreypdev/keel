//! The Android build: the shim as one `lib<namespace>.so` per ABI, through `cargo ndk`.
//!
//! The result is the `jniLibs/` layout Gradle packages (`<abi>/lib<namespace>.so`). The name
//! matters: the generated `UndraCoreNative` loads the core by its namespace (`System.loadLibrary`),
//! so two cores (two namespaces) sit side by side in one APK (ADR-044). The library is built with
//! the `jni` feature, whose `JNI_OnLoad` registers the natives on that generated class
//! (`<kotlin package>.UndraCoreNative`), and is checked for the 16 KB page alignment that Google Play requires of
//! 64-bit libraries (NDK r27 and `cargo-ndk` 4 do it by default; the check says so if a toolchain
//! does not).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::binary::{elf_is_64, elf_min_load_alignment};
use crate::error::{CliError, Code, Result};
use crate::fsutil::{copy_file, human_size, remove_dir_all, size_of};
use crate::session::Session;
use crate::toolchain::{Concern, ndk_major};

use super::{Artifact, gradle};

/// The Rust target of an Android ABI.
#[must_use]
pub fn triple_of(abi: &str) -> &'static str {
    match abi {
        "arm64-v8a" => "aarch64-linux-android",
        "x86_64" => "x86_64-linux-android",
        "armeabi-v7a" => "armv7-linux-androideabi",
        _ => "i686-linux-android",
    }
}

/// Builds `build/android/jniLibs/<abi>/lib<namespace>.so` for every ABI of `[android] abis`.
///
/// # Errors
///
/// `C0003` without `cargo-ndk` or the NDK, `C0011` without the Rust targets, `C0004` when the
/// build fails.
pub fn build(session: &Session<'_>, release: bool) -> Result<Vec<Artifact>> {
    let cfg = &session.project.config.android;
    let cargo = session
        .toolchain
        .which(session.sys, "cargo")
        .ok_or_else(|| {
            CliError::missing_tool(
                "cargo",
                "building the core",
                "install Rust with rustup: https://rustup.rs",
            )
        })?;
    if session.toolchain.which(session.sys, "cargo-ndk").is_none() {
        return Err(CliError::missing_tool(
            "cargo-ndk",
            "building for Android",
            "cargo install cargo-ndk",
        ));
    }
    let Some(ndk) = session.toolchain.android_ndk.clone() else {
        return Err(CliError::new(
            Code::MissingTool,
            "the Android NDK was not found",
            "the core is compiled for Android with the NDK's clang and linker; neither ANDROID_NDK_HOME nor an `ndk/<version>` directory of the Android SDK exists",
            "install it with `sdkmanager \"ndk;27.2.12479018\"` (r27 or newer, for 16 KB pages) and set ANDROID_NDK_HOME, or set ANDROID_HOME so `ndk/` is found",
        ));
    };
    for note in session.toolchain.notes_for(Concern::Android) {
        session.ui.detail(note);
    }
    if ndk_major(&ndk).is_some_and(|major| major < 27) {
        session.ui.warn(&format!(
            "the NDK at {} is older than r27: its libraries are not 16 KB page aligned, which Google Play requires for 64-bit apps",
            ndk.display()
        ));
    }
    for abi in &cfg.abis {
        session.cargo().require_target(triple_of(abi))?;
    }

    let manifest = session.shim_manifest()?;
    let library = format!("lib{}.so", session.namespace()?);
    let target_dir = session.target_dir()?;
    let staging = crate::shim::android_stage_dir(&target_dir, &session.project.root);
    remove_dir_all(&staging)?;

    let mut cmd = Command::new(&cargo);
    cmd.arg("ndk");
    for abi in &cfg.abis {
        cmd.args(["-t", abi]);
    }
    cmd.args(["-P", &cfg.min_sdk.to_string()])
        .arg("-o")
        .arg(&staging)
        .arg("--manifest-path")
        .arg(&manifest)
        .args([
            "rustc",
            "--lib",
            "--crate-type",
            "cdylib",
            "--features",
            "jni",
        ]);
    if release {
        cmd.arg("--release");
    }
    cmd.env("CARGO_TARGET_DIR", &target_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    session.toolchain.apply(&mut cmd);
    let status = cmd.status().map_err(|e| CliError::io("run", &cargo, &e))?;
    if !status.success() {
        return Err(CliError::tool_failed(
            "cargo ndk",
            &format!("building the core for {}", cfg.abis.join(", ")),
            &status.to_string(),
        ));
    }

    let jni_libs: PathBuf = session.project.build_dir().join("android/jniLibs");
    remove_dir_all(&jni_libs)?;
    let mut artifacts = Vec::new();
    for abi in &cfg.abis {
        let shim_so = format!(
            "lib{}.so",
            crate::shim::shim_lib_name(&session.project.root)
        );
        let built = staging.join(abi).join(&shim_so);
        if !built.is_file() {
            return Err(CliError::new(
                Code::ToolFailed,
                format!("cargo-ndk finished but {} does not exist", built.display()),
                "the Android build expects the shim library for every ABI it was asked for",
                "update cargo-ndk (`cargo install cargo-ndk --force`) and try again",
            ));
        }
        let dest = jni_libs.join(abi).join(&library);
        copy_file(&built, &dest)?;
        let mut note = None;
        if let Ok(bytes) = std::fs::read(&dest) {
            if elf_is_64(&bytes) {
                match elf_min_load_alignment(&bytes) {
                    Some(align) if align >= 16 * 1024 => note = Some("16 KB aligned".to_owned()),
                    Some(align) => session.ui.warn(&format!(
                        "{abi}: LOAD segments are aligned to {align} bytes, not 16 KB; Google Play rejects 64-bit libraries like this. Use NDK r27+ with cargo-ndk 3.5+"
                    )),
                    None => {}
                }
            }
        }
        artifacts.push(Artifact {
            label: format!("android {abi}"),
            size: size_of(&dest),
            path: dest,
            budget: Some("1.2 MB per ABI (hello world, release)".to_owned()),
            note,
        });
    }
    let root = &session.project.root;
    if let Some(message) =
        gradle::reconcile(root, &jni_libs, &cfg.abis, &library)?.message(root, &jni_libs)
    {
        session.ui.warn(&message);
    }
    Ok(artifacts)
}

/// The size of the release library an earlier `undra build --platform android --release` left in
/// Cargo's target directory for `abi`, if there is one (`shim` is the shim's library name).
fn earlier_release_size(target_dir: &Path, abi: &str, shim: &str) -> Option<u64> {
    let library = target_dir
        .join(triple_of(abi))
        .join(format!("release/lib{shim}.so"));
    std::fs::metadata(library).ok().map(|m| m.len())
}

/// The hint printed after a debug Android build: what a debug core weighs and what packaging
/// with `--release` would change.
///
/// `debug` is the largest library of the build and `abi` its ABI; `target_dir` is where Cargo put
/// an earlier release build of the shim `shim`, whose size makes the hint exact.
#[must_use]
pub fn debug_size_hint(target_dir: &Path, shim: &str, debug: u64, abi: &str) -> String {
    let release = earlier_release_size(target_dir, abi, shim).filter(|size| *size > 0);
    let how = "undra build --platform android --release";
    match release {
        Some(release) => format!(
            "this Android core is a debug build ({} per ABI, fine for the dev loop); a release build is {}, {}x smaller. Package with `{how}`",
            human_size(debug),
            human_size(release),
            debug / release
        ),
        None => format!(
            "this Android core is a debug build ({} per ABI, fine for the dev loop); a release build is typically 20x or more smaller. Package with `{how}`",
            human_size(debug)
        ),
    }
}

/// The hint for a debug build, from what `artifacts` holds; `None` when nothing Android was built.
pub(crate) fn hint(session: &Session<'_>, artifacts: &[Artifact]) -> Option<String> {
    let largest = artifacts
        .iter()
        .filter(|a| a.label.starts_with("android "))
        .max_by_key(|a| a.size)?;
    let abi = largest.path.parent()?.file_name()?.to_str()?;
    Some(debug_size_hint(
        &session.target_dir().ok()?,
        &crate::shim::shim_lib_name(&session.project.root),
        largest.size,
        abi,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_debug_hint_names_the_release_command_and_the_saving() {
        // No release build to measure: a rule of thumb.
        let none = debug_size_hint(
            Path::new("/does/not/exist"),
            "shim",
            42_400_000,
            "arm64-v8a",
        );
        assert!(
            none.contains("debug build")
                && none.contains("42.4 MB per ABI")
                && none.contains("20x or more smaller")
                && none.contains("`undra build --platform android --release`"),
            "{none}"
        );
        assert!(!none.contains('\n'), "one line");

        // With the release library of an earlier build at hand: the real numbers.
        let target = crate::fsutil::unique_temp_dir("android-hint");
        let release = target.join("aarch64-linux-android/release");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::write(release.join("libshim.so"), vec![0_u8; 1_500_000]).unwrap();
        let known = debug_size_hint(&target, "shim", 42_400_000, "arm64-v8a");
        assert!(
            known.contains("a release build is 1.5 MB, 28x smaller"),
            "{known}"
        );
        // Another ABI has no release library of its own.
        assert!(debug_size_hint(&target, "shim", 42_300_000, "x86_64").contains("typically"));
        let _ = std::fs::remove_dir_all(target);
    }

    #[test]
    fn abis_map_to_rust_targets() {
        assert_eq!(triple_of("arm64-v8a"), "aarch64-linux-android");
        assert_eq!(triple_of("x86_64"), "x86_64-linux-android");
        assert_eq!(triple_of("armeabi-v7a"), "armv7-linux-androideabi");
        assert_eq!(triple_of("x86"), "i686-linux-android");
    }
}
