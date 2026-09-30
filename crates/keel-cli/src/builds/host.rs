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

/// Builds the shim for the host and returns the library inside Cargo's target directory.
///
/// # Errors
///
/// See [`Session::core`] and [`crate::cargo::Cargo::build_library`].
pub fn cdylib(session: &Session<'_>, release: bool) -> Result<PathBuf> {
    let manifest = session.shim_manifest()?;
    let files = session.cargo().build_library(&Build {
        manifest,
        target_dir: session.target_dir(),
        triple: None,
        profile: if release { Profile::Release } else { Profile::Dev },
        crate_type: "cdylib",
        features: vec!["jni".to_owned()],
        env: Vec::new(),
        lib_name: "keel_core".to_owned(),
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
    let dest = session.project.build_dir().join("host").join(library_file_name());
    copy_file(&built, &dest)?;
    Ok(vec![Artifact {
        label: "host".to_owned(),
        size: size_of(&dest),
        path: dest,
        budget: None,
        note: None,
    }])
}
