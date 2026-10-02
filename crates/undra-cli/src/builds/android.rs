//! The Android build: the shim as one `lib<namespace>.so` per ABI, through `cargo ndk`.
//!
//! The result is the `jniLibs/` layout Gradle packages (`<abi>/lib<namespace>.so`). The name
//! matters: the generated `UndraCoreNative` loads the core by its namespace (`System.loadLibrary`),
//! so two cores (two namespaces) sit side by side in one APK (ADR-044). The library is built with
//! the `jni` feature, whose `JNI_OnLoad` registers the natives on that generated class
//! (`<kotlin package>.UndraCoreNative`), and is checked for the 16 KB page alignment that Google Play requires of
//! 64-bit libraries (NDK r27 and `cargo-ndk` 4 do it by default; the check says so if a toolchain
//! does not).
//!
//! **Symbols (ADR-046).** The libraries carry a GNU build id (`-Wl,--build-id=sha1`), which is what
//! a panic report names its image by. A release build keeps the line tables the shim's profile asks
//! for in the library Cargo made, ships a stripped copy of it (`llvm-strip --strip-debug
//! --strip-unneeded` from the NDK) and keeps the unstripped one as the symbol file:
//!
//! * `jniLibs/<abi>/lib<namespace>.so` is what Gradle packages;
//! * `symbols/android/<abi>/lib<namespace>.so` is its unstripped twin (same code, same build id);
//! * `symbols/android/native-debug-symbols.zip` holds the twins as `<abi>/lib<namespace>.so`, the
//!   layout the Play Console takes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::binary::{elf_is_64, elf_min_load_alignment};
use crate::cargo::{PathRemap, Profile};
use crate::error::{CliError, Code, Result};
use crate::fsutil::{copy_file, create_dir_all, human_size, remove_dir_all, size_of};
use crate::session::Session;
use crate::symbols::{Entry, Format, Symbols, image, sha256, slash_relative, tools, zip};
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
pub fn build(
    session: &Session<'_>,
    release: bool,
    symbols: &Symbols<'_, '_>,
) -> Result<Vec<Artifact>> {
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
        // The builder's home directory stays out of what ships (ADR-052), as in the other builds.
        match session.cargo().path_remap(Profile::Release) {
            Some(PathRemap::Config(arg)) => {
                cmd.arg("--config").arg(arg);
            }
            Some(PathRemap::EncodedEnv(flags)) => {
                cmd.env("CARGO_ENCODED_RUSTFLAGS", flags);
            }
            None => {}
        }
    }
    for config in symbols.cargo_config(
        if release {
            Profile::Release
        } else {
            Profile::Dev
        },
        false,
    ) {
        cmd.arg("--config").arg(config);
    }
    // The image's identity (what a panic report names it by): a GNU build id, in the unstripped
    // library and in the stripped copy alike.
    cmd.args(["--", "-C", BUILD_ID_LINK_ARG]);
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
        if release && symbols.enabled {
            // The profile no longer strips (it keeps the line tables for the twin below): the copy
            // that ships is stripped here instead.
            let strip = strip_tool(session)?;
            strip_shipped(&strip, &dest, session)?;
        }
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
        if release && symbols.enabled {
            artifacts.extend(keep_symbols(
                session, symbols, abi, &built, &dest, &library,
            )?);
        }
        artifacts.push(Artifact {
            label: format!("android {abi}"),
            size: size_of(&dest),
            path: dest,
            budget: Some("1.2 MB per ABI (hello world, release)".to_owned()),
            note,
        });
    }
    if release && symbols.enabled {
        artifacts.extend(play_archive(session, symbols, &library)?);
    } else if release {
        symbols.forget("android")?;
    }
    let root = &session.project.root;
    if let Some(message) =
        gradle::reconcile(root, &jni_libs, &cfg.abis, &library)?.message(root, &jni_libs)
    {
        session.ui.warn(&message);
    }
    Ok(artifacts)
}

/// The linker argument that gives the library a GNU build id: the identity a panic report names
/// the image by (`PanicReport.image_id`), and what Play Console and Crashlytics match symbol files
/// with. SHA-1 (20 bytes) rather than lld's default 8-byte hash, so two builds do not collide.
pub const BUILD_ID_LINK_ARG: &str = "link-arg=-Wl,--build-id=sha1";

/// The arguments of the NDK's `llvm-strip` that make the shipped copy: debug info and every symbol
/// the dynamic loader does not need.
pub const STRIP_ARGS: [&str; 2] = ["--strip-debug", "--strip-unneeded"];

/// The NDK's `llvm-strip`.
fn strip_tool(session: &Session<'_>) -> Result<PathBuf> {
    tools::find(session.sys, &session.toolchain, &["llvm-strip"]).ok_or_else(|| {
        CliError::missing_tool(
            "llvm-strip",
            "stripping the library that ships (the unstripped one is kept as the symbol file)",
            "install the NDK (`sdkmanager \"ndk;27.2.12479018\"`), which has it; or `undra build --release --no-symbols` to skip the symbol files",
        )
    })
}

/// Strips `library` in place with [`STRIP_ARGS`].
fn strip_shipped(strip: &Path, library: &Path, session: &Session<'_>) -> Result<()> {
    let mut cmd = Command::new(strip);
    cmd.args(STRIP_ARGS)
        .arg(library)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    session.toolchain.apply(&mut cmd);
    let output = cmd.output().map_err(|e| CliError::io("run", strip, &e))?;
    if output.status.success() {
        return Ok(());
    }
    Err(CliError::tool_failed(
        "llvm-strip",
        "stripping the library that ships",
        &output.status.to_string(),
    )
    .with_detail(String::from_utf8_lossy(&output.stderr).into_owned()))
}

/// Keeps the unstripped library `built` as `build/symbols/android/<abi>/lib<namespace>.so` and
/// records it, with the stripped copy `shipped`, in the manifest.
fn keep_symbols(
    session: &Session<'_>,
    symbols: &Symbols<'_, '_>,
    abi: &str,
    built: &Path,
    shipped: &Path,
    library: &str,
) -> Result<Vec<Artifact>> {
    let identity = symbols.identity()?.clone();
    let twin = symbols.dir().join("android").join(abi).join(library);
    copy_file(built, &twin)?;
    let bytes = std::fs::read(&twin).map_err(|e| CliError::io("read", &twin, &e))?;
    let image_id = image::elf_build_id(&bytes);
    if image_id.is_none() {
        session.ui.warn(&format!(
            "{abi}: the library has no GNU build id, so a crash report cannot name it; the linker was asked for one with {BUILD_ID_LINK_ARG}"
        ));
    }
    let shipped_bytes = std::fs::read(shipped).map_err(|e| CliError::io("read", shipped, &e))?;
    // The copy that ships is the same image: the same build id (stripping keeps the note).
    if image::elf_build_id(&shipped_bytes) != image_id {
        session.ui.warn(&format!(
            "{abi}: the stripped library has another build id than the unstripped one, so the symbol file does not match what ships"
        ));
    }
    symbols.record(Entry {
        platform: "android".to_owned(),
        namespace: identity.namespace,
        core_version: identity.core_version,
        schema_hash: identity.schema_hash,
        arch: abi.to_owned(),
        format: Format::Elf,
        image_id,
        sha256: sha256::hex(&shipped_bytes),
        shipped: slash_relative(&session.project.build_dir(), shipped),
        shipped_bytes: shipped_bytes.len() as u64,
        symbols: Some(slash_relative(&symbols.dir(), &twin)),
        function_map: None,
        dwarf: None,
    })?;
    Ok(vec![Artifact {
        label: format!("symbols android {abi}"),
        size: size_of(&twin),
        path: twin,
        budget: None,
        note: Some("unstripped, not shipped".to_owned()),
    }])
}

/// Writes `build/symbols/android/native-debug-symbols.zip`: every ABI's unstripped library as
/// `<abi>/lib<namespace>.so`, the layout the Play Console takes. With `zip` installed it is
/// compressed; without, the CLI writes the archive itself, stored.
fn play_archive(
    session: &Session<'_>,
    symbols: &Symbols<'_, '_>,
    library: &str,
) -> Result<Option<Artifact>> {
    let dir = symbols.dir().join("android");
    let abis = &session.project.config.android.abis;
    let archive = dir.join(PLAY_ARCHIVE);
    let _ = std::fs::remove_file(&archive);
    create_dir_all(&dir)?;
    let tool = session.toolchain.which(session.sys, "zip");
    let made = tool.is_some_and(|zip_tool| {
        let mut cmd = Command::new(zip_tool);
        cmd.args(["-q", "-X", "-9", PLAY_ARCHIVE])
            .args(abis.iter().map(|abi| format!("{abi}/{library}")))
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        session.toolchain.apply(&mut cmd);
        cmd.status().is_ok_and(|status| status.success())
    });
    if !made {
        let mut files = Vec::new();
        for abi in abis {
            let path = dir.join(abi).join(library);
            let bytes = std::fs::read(&path).map_err(|e| CliError::io("read", &path, &e))?;
            files.push((format!("{abi}/{library}"), bytes));
        }
        let bytes = zip::archive(&files).map_err(|why| {
            CliError::new(
                Code::ToolFailed,
                format!("cannot write {PLAY_ARCHIVE}: {why}"),
                "the archive holds every ABI's unstripped library",
                "install `zip`, or build fewer ABIs",
            )
        })?;
        std::fs::write(&archive, bytes).map_err(|e| CliError::io("write", &archive, &e))?;
    }
    Ok(Some(Artifact {
        label: "symbols android play".to_owned(),
        size: size_of(&archive),
        path: archive,
        budget: None,
        note: Some("upload to the Play Console (native debug symbols)".to_owned()),
    }))
}

/// The Play Console's native debug symbols archive, below `build/symbols/android`.
pub const PLAY_ARCHIVE: &str = "native-debug-symbols.zip";

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

/// The hint for a debug build of an app whose Gradle script strips the core's debug info from
/// the APK: what to add so the native debugger can stop in Rust (ADR-046).
pub(crate) fn debugger_hint(session: &Session<'_>) -> Option<String> {
    let library = format!("lib{}.so", session.namespace().ok()?);
    let (script, line) = gradle::missing_debug_symbols(&session.project.root, &library)?;
    Some(format!(
        "to step into Rust from Android Studio, {} has to keep the debug info of {library} in the debug APK; add: {line}",
        script
            .strip_prefix(&session.project.root)
            .unwrap_or(&script)
            .display()
    ))
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
    fn the_libraries_get_a_build_id_and_the_shipped_copy_loses_everything_the_loader_does_not_need()
    {
        assert_eq!(BUILD_ID_LINK_ARG, "link-arg=-Wl,--build-id=sha1");
        assert_eq!(STRIP_ARGS, ["--strip-debug", "--strip-unneeded"]);
    }

    #[test]
    fn abis_map_to_rust_targets() {
        assert_eq!(triple_of("arm64-v8a"), "aarch64-linux-android");
        assert_eq!(triple_of("x86_64"), "x86_64-linux-android");
        assert_eq!(triple_of("armeabi-v7a"), "armv7-linux-androideabi");
        assert_eq!(triple_of("x86"), "i686-linux-android");
    }
}
