//! The host build: the shim as a cdylib for this machine, `build/host/lib<namespace>.{dylib,so}`.
//!
//! `undra bindgen` loads it to read the schema (through `<namespace>_undra_api`); the Kotlin
//! runtime's JVM tests load it through JNI (the generated `UndraCoreNative` loads `<namespace>`,
//! `-Djava.library.path=build/host` or `-Dundra.native.<namespace>.path=<file>`). It is built with the
//! `jni` feature so one library serves both, and so an `undra bindgen` followed by an `undra build`
//! builds `undra-ffi` once.

use std::path::PathBuf;

use crate::cargo::{Build, Profile};
use crate::error::Result;
use crate::fsutil::{copy_file, size_of};
use crate::session::Session;
use crate::sys::Os;

use super::Artifact;

/// The file name of a library called `name` on this machine (`lib<name>.dylib`, `lib<name>.so`,
/// `<name>.dll`): the core's is `name` = its namespace (ADR-044), what `System.loadLibrary` and
/// `dlopen` look for.
#[must_use]
pub fn library_file_name(name: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("lib{name}.dylib")
    } else if cfg!(windows) {
        format!("{name}.dll")
    } else {
        format!("lib{name}.so")
    }
}

/// The rustc arguments that give the host library a location-independent identity.
///
/// A macOS dylib records its *install name*, and rustc defaults it to the absolute path of the
/// file it just wrote (`/Users/ana/app/target/debug/deps/libundra_core_1234abcd.dylib`). Anything linked
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
    let namespace = session.namespace()?;
    let files = session.cargo().build_library(&Build {
        manifest,
        // A target directory of its own, not the project's: see `host_lib_target_dir` (ADR-029).
        target_dir: crate::shim::host_lib_target_dir(&session.target_dir()?, &session.project.root),
        triple: None,
        profile: if release {
            Profile::Release
        } else {
            Profile::Dev
        },
        crate_type: "cdylib",
        features: vec!["jni".to_owned()],
        // Belt to the profile's `incremental = false`: overrides an inherited `CARGO_INCREMENTAL=1`.
        env: vec![("CARGO_INCREMENTAL".to_owned(), "0".to_owned())],
        lib_name: crate::shim::shim_lib_name(&session.project.root),
        rustc_args: identity_args(session.sys.os(), &library_file_name(&namespace)),
    })?;
    let wanted = library_file_name(&crate::shim::shim_lib_name(&session.project.root));
    Ok(files
        .iter()
        .find(|f| f.file_name().is_some_and(|n| n == wanted.as_str()))
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
        .join(library_file_name(&session.namespace()?));
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
            identity_args(Os::Macos, "libacme_pay.dylib"),
            vec!["-Clink-arg=-Wl,-install_name,@rpath/libacme_pay.dylib".to_owned()]
        );
    }

    #[test]
    fn other_systems_record_no_path_so_nothing_is_passed() {
        assert!(identity_args(Os::Linux, "libacme_pay.so").is_empty());
        assert!(identity_args(Os::Windows, "acme_pay.dll").is_empty());
    }

    #[test]
    fn the_library_is_named_after_the_namespace() {
        let name = library_file_name("acme_pay");
        assert!(
            ["libacme_pay.dylib", "libacme_pay.so", "acme_pay.dll"].contains(&name.as_str()),
            "{name}"
        );
    }
}
