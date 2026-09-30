//! The host build: the shim as a cdylib for this machine.
//!
//! `keel bindgen` loads it to read the schema; the Kotlin runtime's JVM tests load it through
//! JNI (`-Dkeel.native.path=build/host/libkeel_core.dylib`). It is built with the `jni` feature
//! so one library serves both, and so a `keel bindgen` followed by a `keel build` builds `keel-ffi`
//! once.

use std::path::PathBuf;

use crate::cargo::{Build, Profile};
use crate::error::Result;
use crate::fsutil::{copy_file, size_of};
use crate::session::Session;
use crate::sys::Os;

use super::Artifact;

/// The name of the library Cargo produces on this machine.
#[must_use]
pub fn library_file_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libkeel_core.dylib"
    } else if cfg!(windows) {
        "keel_core.dll"
    } else {
        "libkeel_core.so"
    }
}

/// The rustc arguments that give the host library a location-independent identity.
///
/// A macOS dylib records its *install name*, and rustc defaults it to the absolute path of the
/// file it just wrote (`/Users/ana/app/target/debug/deps/libkeel_core.dylib`). Anything linked
/// against, or embedding, a copy of the library would then look for it on the machine that built
/// it, and the path leaks into whatever embeds it. `@rpath/<name>` lets the consumer decide where
/// the library lives. Other systems record no path (ELF has no install name unless asked), so
/// nothing is passed there.
#[must_use]
pub fn identity_args(os: Os, file_name: &str) -> Vec<String> {
    match os {
        Os::Macos => vec![format!("-Clink-arg=-Wl,-install_name,@rpath/{file_name}")],
        Os::Linux | Os::Windows => Vec::new(),
    }
}

/// Builds the shim for the host and returns the library inside Cargo's target directory.
///
/// # Errors
///
/// See [`Session::core`] and [`crate::cargo::Cargo::build_library`].
pub fn cdylib(session: &Session<'_>, release: bool) -> Result<PathBuf> {
    let manifest = session.shim_manifest()?;
    let files = session.cargo().build_library(&Build {
        manifest,
        target_dir: session.target_dir()?,
        triple: None,
        profile: if release {
            Profile::Release
        } else {
            Profile::Dev
        },
        crate_type: "cdylib",
        features: vec!["jni".to_owned()],
        env: Vec::new(),
        lib_name: "keel_core".to_owned(),
        rustc_args: identity_args(session.sys.os(), library_file_name()),
    })?;
    let wanted = library_file_name();
    Ok(files
        .iter()
        .find(|f| f.file_name().is_some_and(|n| n == wanted))
        .or_else(|| files.first())
        .cloned()
        .expect("build_library returns at least one file"))
}

/// Builds the host library and copies it to `build/host/`.
///
/// # Errors
///
/// See [`cdylib`].
pub fn package(session: &Session<'_>, release: bool) -> Result<Vec<Artifact>> {
    let built = cdylib(session, release)?;
    let dest = session
        .project
        .build_dir()
        .join("host")
        .join(library_file_name());
    copy_file(&built, &dest)?;
    Ok(vec![Artifact {
        label: "host".to_owned(),
        size: size_of(&dest),
        path: dest,
        budget: None,
        note: None,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_macos_library_is_named_relative_to_rpath() {
        assert_eq!(
            identity_args(Os::Macos, "libkeel_core.dylib"),
            vec!["-Clink-arg=-Wl,-install_name,@rpath/libkeel_core.dylib".to_owned()]
        );
    }

    #[test]
    fn other_systems_record_no_path_so_nothing_is_passed() {
        assert!(identity_args(Os::Linux, "libkeel_core.so").is_empty());
        assert!(identity_args(Os::Windows, "keel_core.dll").is_empty());
    }
}
