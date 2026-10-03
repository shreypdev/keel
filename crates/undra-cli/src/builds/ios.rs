//! The iOS build: the shim as a static library for device and simulator, each slice **prelinked**
//! into one object whose only global symbol is the core's entry, packaged as an XCFramework
//! (ADR-044).
//!
//! `<Namespace>Core.xcframework` (`PlaygroundCore.xcframework` for the namespace `playground_core`)
//! holds one `lib<namespace>.a` per platform (`ios-arm64`, `ios-arm64-simulator`; an x86_64
//! simulator slice is folded into the simulator one when `[ios] simulator_archs` asks for it) and
//! the core's header, `<namespace>_undra.h`, which declares its one export,
//! `<namespace>_undra_api()`. The header comes without a module map: the generated Swift package
//! declares the module (`<Namespace>CoreFFI`), and a second definition of it in the XCFramework
//! would fail the app's build ("redefinition of module"); C, C++ and Objective-C hosts (the
//! React Native pod) include the header directly.
//!
//! **Why prelink.** A Rust static library exports every Rust symbol it contains. Linked plainly,
//! two cores in one app either collide (`-force_load`: thousands of duplicate symbols) or silently
//! merge into one registry and one runtime (ADR-044's probe). `ld -r` with an exported-symbol list
//! turns each slice into a single relocatable object whose only global is
//! `_<namespace>_undra_api`: everything else (std, the runtime, the core, compiler builtins) becomes
//! private to that object. A release slice (fat LTO, one object) is prelinked with
//! `-u _<namespace>_undra_api`, which pulls exactly what the entry reaches; a debug slice (many
//! objects) with `-all_load`, so the static constructors of the core's `#[undra::api]`
//! registrations are kept. Because the entry symbol pulls the one object that holds every
//! registration, the app no longer links the library with `-force_load`.
//!
//! **Symbols.** A release build's profile keeps the DWARF line tables of the core in Cargo's
//! objects, and `ld -r` does what Apple's linker always does with debug info: it writes no DWARF,
//! it writes a *debug map* (`N_OSO` stabs) naming the object files the DWARF is in. The app's link
//! forwards that map, and Xcode's `dsymutil` (Release: `dwarf-with-dsym`) follows it into those
//! objects, so **the app's own dSYM** gets the Rust frames, as ADR-044's probe showed. The objects
//! are the ones in Cargo's target directory (`libundra_core_<hash>.a(…rcgu.o)`); they must still be
//! there, unchanged, when the app is linked, which they are for a build phase that runs `undra
//! build` and then compiles and links the app. A release build records each slice in
//! `build/symbols/manifest.json` and warns when the prelinked library names no object that exists
//! (ADR-046).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::cargo::{Build, Profile};
use crate::error::{CliError, Code, Result};
use crate::fsutil::{create_dir_all, remove_dir_all, size_of, write_if_changed};
use crate::session::Session;
use crate::symbols::{Entry, Format, Symbols, sha256, slash_relative};
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

/// Builds `build/ios/<Namespace>Core.xcframework` (`lib<namespace>.a` per slice and the core's
/// header).
///
/// # Errors
///
/// `C0012` off macOS, `C0003` without Xcode's tools, `C0011` without the Rust targets, `C0004`
/// when a build or `xcodebuild` fails.
pub fn build(
    session: &Session<'_>,
    release: bool,
    symbols: &Symbols<'_, '_>,
) -> Result<Vec<Artifact>> {
    if session.sys.os() != Os::Macos {
        return Err(unsupported(
            "iOS",
            "iOS libraries are linked with Apple's toolchain, which exists only on macOS",
            "build the iOS slice on a Mac (in CI: a macos runner) and the others anywhere: `undra build --platform android,web`",
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
    let xcrun = session.toolchain.which(session.sys, "xcrun").ok_or_else(|| {
        CliError::missing_tool(
            "xcrun",
            "prelinking the core (`ld -r`) and archiving it (`libtool`)",
            "install Xcode, then `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`",
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
    let names = session.core_names()?;
    let library = format!("lib{}.a", names.namespace());
    // A release build is the profile `[ios] opt_level` names, the size-tuned `release-mobile` by default
    // (ADR-052, "native size gates").
    let profile = Profile::mobile(release, session.project.config.ios.opt_level);
    let target_dir = session.target_dir()?;
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
            lib_name: crate::shim::shim_lib_name(&session.project.root),
            rustc_args: Vec::new(),
            cargo_config: symbols.cargo_config(profile, true),
            remap: session.remap_roots(),
        })?;
        files
            .into_iter()
            .find(|f| f.extension().is_some_and(|e| e == "a"))
            .ok_or_else(|| {
                CliError::new(
                    Code::ToolFailed,
                    format!("cargo built for {triple} but produced no static library"),
                    "the iOS build links the shim as a .a",
                    "run `cargo clean` and try again; if it persists this is a bug in undra-cli",
                )
            })
    };

    // One prelinked object per architecture (ADR-044), archived as `lib<namespace>.a`.
    let prelinked = |triple: &str| -> Result<PathBuf> {
        let built = staticlib(triple)?;
        prelink(
            session,
            &xcrun,
            &built,
            &stage.join(triple),
            names.namespace(),
            release,
        )
    };
    // Device.
    let device = prelinked(DEVICE_TRIPLE)?;
    // Simulator: one library per architecture, folded into one with lipo.
    let mut sim_libs = Vec::new();
    for arch in &session.project.config.ios.simulator_archs {
        sim_libs.push(prelinked(simulator_triple(arch))?);
    }
    let sim = if sim_libs.len() == 1 {
        sim_libs.remove(0)
    } else {
        let fat = stage.join("simulator-fat").join(&library);
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

    // The core's header, next to each slice's library: `<namespace>_undra.h` declares the entry.
    let headers = stage.join("headers");
    remove_dir_all(&headers)?;
    write_if_changed(&headers.join(names.header()), &names.header_text())?;

    let out_dir = session.project.build_dir().join("ios");
    let bundle = format!("{}.xcframework", names.bundle());
    let xcframework = out_dir.join(&bundle);
    remove_dir_all(&xcframework)?;
    create_dir_all(&out_dir)?;
    session.ui.step(&format!("Creating {bundle}"));
    let mut cmd = Command::new(&xcodebuild);
    cmd.arg("-create-xcframework");
    for lib in [&device, &sim] {
        cmd.arg("-library").arg(lib).arg("-headers").arg(&headers);
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

    if release {
        record_symbols(session, symbols, &xcframework, &library)?;
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

/// The object files a prelinked library's debug map names, from `nm -pa` output: the `N_OSO`
/// stabs, `OSO <path>` or `OSO <archive>(<member>)`, without the member, once each.
#[must_use]
pub fn debug_map_objects(nm_output: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in nm_output.lines() {
        let Some(at) = line.find(" OSO ") else {
            continue;
        };
        let path = line[at + 5..].trim();
        let path = path.split_once('(').map_or(path, |(archive, _)| archive);
        if !path.is_empty() && !out.iter().any(|p| p == path) {
            out.push(path.to_owned());
        }
    }
    out
}

/// Records every slice of the XCFramework in the symbol manifest (a release build), or forgets
/// the iOS symbols an earlier build wrote when they are not wanted (`--no-symbols`).
///
/// An iOS slice has no image identity at build time: the image a report names is the app that
/// links the library, whose UUID is that of the app's dSYM (see [`crate::symbols::manifest`]).
fn record_symbols(
    session: &Session<'_>,
    symbols: &Symbols<'_, '_>,
    xcframework: &Path,
    library: &str,
) -> Result<()> {
    if !symbols.enabled {
        return symbols.forget("ios");
    }
    let identity = symbols.identity()?.clone();
    let build_dir = session.project.build_dir();
    let mut slices: Vec<String> = std::fs::read_dir(xcframework)
        .map_err(|e| CliError::io("read", xcframework, &e))?
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().join(library).is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    slices.sort();
    for slice in slices {
        let path = xcframework.join(&slice).join(library);
        let bytes = std::fs::read(&path).map_err(|e| CliError::io("read", &path, &e))?;
        // The prelinked object has no DWARF of its own, but a debug map to the objects that do.
        let objects = session
            .toolchain
            .which(session.sys, "nm")
            .and_then(|nm| {
                session.sys.run(
                    &nm,
                    &["-pa", &path.display().to_string()],
                    &session.toolchain.env_pairs(),
                )
            })
            .map(|out| debug_map_objects(&out.stdout));
        if let Some(objects) = objects {
            if !objects
                .iter()
                .any(|object| session.sys.is_file(Path::new(object)))
            {
                session.ui.warn(&format!(
                    "{} names no object file with the core's DWARF line tables that exists any more ({}), so the app's dSYM will have no Rust frames; build the core again (`undra build --release`) before building the app",
                    path.display(),
                    objects.first().map_or("it has no debug map", String::as_str)
                ));
            }
        }
        symbols.record(Entry {
            platform: "ios".to_owned(),
            namespace: identity.namespace.clone(),
            core_version: identity.core_version.clone(),
            schema_hash: identity.schema_hash,
            arch: slice,
            format: Format::Macho,
            image_id: None,
            sha256: sha256::hex(&bytes),
            shipped: slash_relative(&build_dir, &path),
            shipped_bytes: bytes.len() as u64,
            symbols: None,
            function_map: None,
            dwarf: None,
        })?;
    }
    Ok(())
}

/// The architecture `ld -arch` names for a Rust iOS target.
fn arch_of(triple: &str) -> &'static str {
    if triple.starts_with("x86_64") {
        "x86_64"
    } else {
        "arm64"
    }
}

/// The `ld -platform_version` platform of a Rust iOS target.
fn platform_of(triple: &str) -> &'static str {
    if triple.ends_with("-sim") || triple.starts_with("x86_64") {
        "ios-simulator"
    } else {
        "ios"
    }
}

/// What a prelink needs to know about its slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice<'a> {
    /// The Rust target (`aarch64-apple-ios`, `aarch64-apple-ios-sim`, `x86_64-apple-ios`).
    pub triple: &'a str,
    /// The minimum iOS version (`[ios] deployment_target`).
    pub min_os: &'a str,
    /// The SDK version the slice is built against.
    pub sdk: &'a str,
}

/// The `ld -r` arguments that prelink `staticlib` into `object`, keeping `symbol` the only global:
/// `-u` the entry for a release (fat LTO) library, `-all_load` for a debug one (ADR-044).
#[must_use]
pub fn prelink_args(
    slice: &Slice<'_>,
    staticlib: &Path,
    object: &Path,
    symbol: &str,
    release: bool,
) -> Vec<String> {
    let mangled = format!("_{symbol}");
    let mut args = vec![
        "ld".to_owned(),
        "-r".to_owned(),
        "-arch".to_owned(),
        arch_of(slice.triple).to_owned(),
        "-platform_version".to_owned(),
        platform_of(slice.triple).to_owned(),
        slice.min_os.to_owned(),
        slice.sdk.to_owned(),
        "-exported_symbol".to_owned(),
        mangled.clone(),
    ];
    if release {
        args.extend(["-u".to_owned(), mangled]);
    } else {
        args.push("-all_load".to_owned());
    }
    args.push(staticlib.display().to_string());
    args.extend(["-o".to_owned(), object.display().to_string()]);
    args
}

/// Prelinks the shim's static library for `triple` into one object whose only global symbol is
/// `_<namespace>_undra_api`, and archives it as `<dir>/lib<namespace>.a`.
fn prelink(
    session: &Session<'_>,
    xcrun: &Path,
    staticlib: &Path,
    dir: &Path,
    namespace: &str,
    release: bool,
) -> Result<PathBuf> {
    create_dir_all(dir)?;
    let object = dir.join(format!("{namespace}.o"));
    let archive = dir.join(format!("lib{namespace}.a"));
    let _ = std::fs::remove_file(&archive);
    let triple = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let run = |args: &[String], what: &str| -> Result<()> {
        let mut cmd = Command::new(xcrun);
        cmd.args(args).stdin(Stdio::null());
        session.toolchain.apply(&mut cmd);
        let output = cmd.output().map_err(|e| CliError::io("run", xcrun, &e))?;
        if output.status.success() {
            return Ok(());
        }
        Err(CliError::tool_failed(
            &format!("xcrun {}", args.first().map_or("", String::as_str)),
            what,
            &output.status.to_string(),
        )
        .with_detail(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    };
    session
        .ui
        .detail(&format!("prelinking {triple} into one object (ld -r)"));
    let min_os = &session.project.config.ios.deployment_target;
    let sdk_name = if platform_of(&triple) == "ios" {
        "iphoneos"
    } else {
        "iphonesimulator"
    };
    // The SDK version recorded in the object; the deployment target when xcrun cannot say.
    let sdk = session
        .sys
        .run(
            xcrun,
            &["--sdk", sdk_name, "--show-sdk-version"],
            &session.toolchain.env_pairs(),
        )
        .filter(|out| out.success)
        .map(|out| out.stdout.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| min_os.clone());
    run(
        &prelink_args(
            &Slice {
                triple: &triple,
                min_os,
                sdk: &sdk,
            },
            staticlib,
            &object,
            &format!("{namespace}_undra_api"),
            release,
        ),
        "prelinking the core into one object",
    )?;
    run(
        &[
            "libtool".to_owned(),
            "-static".to_owned(),
            "-o".to_owned(),
            archive.display().to_string(),
            object.display().to_string(),
        ],
        "archiving the prelinked core",
    )?;
    Ok(archive)
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
    fn a_release_slice_pulls_the_entry_and_a_debug_one_everything() {
        let release = prelink_args(
            &Slice {
                triple: "aarch64-apple-ios",
                min_os: "17.0",
                sdk: "26.5",
            },
            Path::new("/t/libshim.a"),
            Path::new("/s/acme_pay.o"),
            "acme_pay_undra_api",
            true,
        );
        assert_eq!(
            release.join(" "),
            "ld -r -arch arm64 -platform_version ios 17.0 26.5 -exported_symbol _acme_pay_undra_api -u _acme_pay_undra_api /t/libshim.a -o /s/acme_pay.o"
        );
        let debug = prelink_args(
            &Slice {
                triple: "x86_64-apple-ios",
                min_os: "17.0",
                sdk: "26.5",
            },
            Path::new("/t/libshim.a"),
            Path::new("/s/acme_pay.o"),
            "acme_pay_undra_api",
            false,
        );
        assert_eq!(
            debug.join(" "),
            "ld -r -arch x86_64 -platform_version ios-simulator 17.0 26.5 -exported_symbol _acme_pay_undra_api -all_load /t/libshim.a -o /s/acme_pay.o"
        );
    }

    #[test]
    fn the_runtime_declares_the_c_abi_module_itself() {
        // The XCFramework carries the core's header but no module map: the Swift runtime's
        // `UndraFFI` target defines the module of `undra.h`, and the generated package the
        // module of the core's header; a second definition in the XCFramework fails the build
        // ("redefinition of module").
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let modulemap =
            repo.join("runtimes/swift/UndraRuntime/Sources/UndraFFI/include/module.modulemap");
        if !modulemap.is_file() {
            return; // built outside the repository (a packaged crate)
        }
        assert!(
            std::fs::read_to_string(modulemap)
                .unwrap()
                .contains("module UndraFFI")
        );
    }

    #[test]
    fn the_debug_map_names_the_objects_with_the_dwarf() {
        let nm = "0000000000000000 - 01 0000    SO \n\
                  0000000000000000 - 00 0001   OSO /t/aarch64-apple-ios-sim/release/libcore.a(core-cgu.0.rcgu.o)\n\
                  0000000000000000 - 00 0001   OSO /t/aarch64-apple-ios-sim/release/libcore.a(builtins.rcgu.o)\n\
                  0000000000000000 - 00 0001   OSO /t/shim/lib.o\n\
                  000000000000dad4 t _some_function\n";
        assert_eq!(
            debug_map_objects(nm),
            [
                "/t/aarch64-apple-ios-sim/release/libcore.a",
                "/t/shim/lib.o"
            ]
        );
        assert!(debug_map_objects("0000000000000000 T _x\n").is_empty());
    }

    #[test]
    fn off_macos_the_error_points_at_the_other_platforms() {
        let e = unsupported("iOS", "why", "fix");
        assert_eq!(e.code, Code::Unsupported);
    }
}
