//! What Xcode's build phase needs of `undra build`: a configuration name and a stamp.
//!
//! The Xcode project `undra init` writes has a Run Script phase, before Compile Sources, that runs
//! `undra build --platform ios --configuration $CONFIGURATION`. Xcode skips a script phase when its
//! declared outputs are newer than its declared inputs, and the XCFramework alone cannot say
//! *which configuration* built it: a Release build after a Debug one would find the Debug core
//! "up to date". So a build for a configuration writes a stamp,
//! `build/ios/.undra-configuration-<Configuration>`, which the phase lists as an output, and
//! removes the stamps of the other configurations: switching configuration makes the phase run.
//!
//! **The input list.** Xcode compares the modification time of every listed input with the
//! outputs, and it looks at a directory's own entry only, never at the files inside it: editing
//! `core/src/todo.rs` is invisible unless that file is listed. The list lives in
//! `ios/Config/undra-core-inputs.xcfilelist`; `undra init` writes it for the template core and every
//! iOS build refreshes it ([`refresh_inputs`]) from what the core is really built from (its
//! sources, manifests, and those of the path dependencies it has), so it never goes stale. The
//! directories are listed too: adding a file changes its directory's entry, which Xcode sees.

use std::path::{Path, PathBuf};

use crate::error::{CliError, Result};
use crate::fsutil::write_if_changed;
use crate::names::{portable, relative_path};

/// The prefix of a stamp file's name; the configuration name follows.
pub const STAMP_PREFIX: &str = ".undra-configuration-";

/// Whether an Xcode configuration builds a release core: `Release` and every name that contains
/// it (`Release-Staging`, `AppStore Release`), compared without regard to case. Any other name
/// (`Debug`, `Beta`) builds a debug core, which is what Xcode's own templates mean by the two.
#[must_use]
pub fn is_release(configuration: &str) -> bool {
    configuration.to_ascii_lowercase().contains("release")
}

/// Where the input list of the Run Script phase is, relative to the project root.
pub const INPUTS_LIST: &str = "ios/Config/undra-core-inputs.xcfilelist";

/// Everything the core is built from, as absolute paths, sorted: the project's `undra.toml`,
/// workspace `Cargo.toml` and `Cargo.lock` (the shim follows it: `cargo update` rebuilds the core,
/// as it does in Gradle), and for each directory of `core_dirs` its `Cargo.toml`, `build.rs`,
/// every file below `src/` and every directory of `src/`. Only what exists is listed (Xcode fails
/// the build on a listed input that is missing).
#[must_use]
pub fn input_paths(project_root: &Path, core_dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                out.push(path.clone());
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for file in ["undra.toml", "Cargo.toml", "Cargo.lock"] {
        out.push(project_root.join(file));
    }
    for dir in core_dirs {
        out.push(dir.join("Cargo.toml"));
        out.push(dir.join("build.rs"));
        let src = dir.join("src");
        if src.is_dir() {
            out.push(src.clone());
            walk(&src, &mut out);
        }
    }
    out.retain(|p| p.exists());
    out.sort();
    out.dedup();
    out
}

/// The text of the input list: one `$(SRCROOT)/<path>` per line, `srcroot` being the directory of
/// the Xcode project.
#[must_use]
pub fn render_inputs(srcroot: &Path, paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for path in paths {
        let rel = relative_path(srcroot, path).unwrap_or_else(|| path.clone());
        out.push_str("$(SRCROOT)/");
        out.push_str(&portable(&rel));
        out.push('\n');
    }
    out
}

/// Writes the input list of the Xcode project in `ios/` for a core built from `core_dirs`.
/// Returns whether the file changed.
///
/// # Errors
///
/// `C0010` when it cannot be written.
pub fn write_inputs(project_root: &Path, core_dirs: &[PathBuf]) -> Result<bool> {
    let list = project_root.join(INPUTS_LIST);
    let srcroot = project_root.join("ios");
    write_if_changed(
        &list,
        &render_inputs(&srcroot, &input_paths(project_root, core_dirs)),
    )
}

/// Brings the input list of the Xcode project up to date, when the project has one (an app that
/// was not made by `undra init` may not). Returns whether the file changed.
///
/// # Errors
///
/// `C0010` when it cannot be written.
pub fn refresh_inputs(project_root: &Path, core_dirs: &[PathBuf]) -> Result<bool> {
    if !project_root.join(INPUTS_LIST).is_file() {
        return Ok(false);
    }
    write_inputs(project_root, core_dirs)
}

/// The stamp of `configuration` below `ios_dir` (`<build>/ios`).
#[must_use]
pub fn stamp_path(ios_dir: &Path, configuration: &str) -> PathBuf {
    ios_dir.join(format!("{STAMP_PREFIX}{}", file_safe(configuration)))
}

/// A configuration name as a file name: path separators cannot appear in one.
fn file_safe(configuration: &str) -> String {
    configuration.replace(['/', '\\'], "_")
}

/// Writes the stamp of `configuration` and removes the others.
///
/// # Errors
///
/// `C0010` when the directory cannot be written.
pub fn write_stamp(build_dir: &Path, configuration: &str) -> Result<PathBuf> {
    let ios_dir = build_dir.join("ios");
    let stamp = stamp_path(&ios_dir, configuration);
    if let Ok(entries) = std::fs::read_dir(&ios_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let is_stamp = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(STAMP_PREFIX));
            if is_stamp && path != stamp {
                std::fs::remove_file(&path).map_err(|e| CliError::io("remove", &path, &e))?;
            }
        }
    }
    let kind = if is_release(configuration) {
        "release"
    } else {
        "debug"
    };
    // The content never changes between two builds of one configuration, so the file's
    // modification time moves only when the stamp is created: Xcode compares it with the inputs.
    write_if_changed(
        &stamp,
        &format!(
            "undra {} built the {kind} core of the {configuration} configuration\n",
            crate::version::SEMVER
        ),
    )?;
    // A rebuild with nothing to rewrite must still count as newer than the sources it was built from.
    touch(&stamp);
    Ok(stamp)
}

/// Sets the modification time of `path` to now (best effort: the stamp is advisory).
fn touch(path: &Path) {
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::unique_temp_dir;

    #[test]
    fn release_configurations_are_recognised_by_name() {
        for name in ["Release", "release", "AppStore Release", "Release-Staging"] {
            assert!(is_release(name), "{name}");
        }
        for name in ["Debug", "Beta", "Staging", ""] {
            assert!(!is_release(name), "{name:?}");
        }
    }

    #[test]
    fn a_stamp_replaces_the_stamps_of_other_configurations() {
        let build = unique_temp_dir("xcode-stamp");
        let debug = write_stamp(&build, "Debug").unwrap();
        assert!(debug.is_file());
        assert!(debug.ends_with("ios/.undra-configuration-Debug"));
        let release = write_stamp(&build, "Release").unwrap();
        assert!(release.is_file());
        assert!(
            !debug.exists(),
            "the Debug stamp is gone: Xcode runs the phase again for Debug"
        );
        // Writing the same stamp again keeps it, and other files in the directory are left alone.
        std::fs::write(build.join("ios/TodoCore.xcframework"), "x").unwrap();
        write_stamp(&build, "Release").unwrap();
        assert!(release.is_file());
        assert!(build.join("ios/TodoCore.xcframework").is_file());
        let text = std::fs::read_to_string(&release).unwrap();
        assert!(
            text.contains("release core of the Release configuration"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(build);
    }

    #[test]
    fn a_name_with_a_slash_is_still_one_file() {
        let build = unique_temp_dir("xcode-stamp-slash");
        let stamp = write_stamp(&build, "Release/Staging").unwrap();
        assert_eq!(stamp.parent().unwrap(), build.join("ios"));
        assert!(stamp.is_file());
        let _ = std::fs::remove_dir_all(build);
    }

    fn core_tree(root: &Path) {
        for (path, text) in [
            ("undra.toml", ""),
            ("Cargo.toml", ""),
            ("Cargo.lock", ""),
            ("core/Cargo.toml", ""),
            ("core/src/lib.rs", ""),
            ("core/src/todo/list.rs", ""),
            ("core/src/.hidden.rs", ""),
            ("core/target/junk.rs", ""),
            ("shared/Cargo.toml", ""),
            ("shared/build.rs", ""),
            ("shared/src/lib.rs", ""),
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
    }

    #[test]
    fn the_inputs_are_what_the_core_is_built_from() {
        let root = unique_temp_dir("xcode-inputs");
        core_tree(&root);
        let paths = input_paths(&root, &[root.join("core"), root.join("shared")]);
        let shown: Vec<String> = paths
            .iter()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            shown,
            [
                "Cargo.lock",
                "Cargo.toml",
                "core/Cargo.toml",
                "core/src",
                "core/src/lib.rs",
                "core/src/todo",
                "core/src/todo/list.rs",
                "shared/Cargo.toml",
                "shared/build.rs",
                "shared/src",
                "shared/src/lib.rs",
                "undra.toml",
            ],
            "hidden files and target/ are not inputs; a file that does not exist is not listed"
        );
        let text = render_inputs(&root.join("ios"), &paths);
        assert!(
            text.starts_with(
                "$(SRCROOT)/../Cargo.lock\n$(SRCROOT)/../Cargo.toml\n$(SRCROOT)/../core/Cargo.toml\n"
            ),
            "{text}"
        );
        assert!(text.ends_with("$(SRCROOT)/../undra.toml\n"), "{text}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_list_is_refreshed_when_the_core_grows_and_left_alone_otherwise() {
        let root = unique_temp_dir("xcode-refresh");
        core_tree(&root);
        let dirs = [root.join("core")];
        // A project without an Xcode app has no list: nothing is written.
        assert!(!refresh_inputs(&root, &dirs).unwrap());
        assert!(!root.join(INPUTS_LIST).exists());

        assert!(write_inputs(&root, &dirs).unwrap(), "init writes it");
        assert!(
            !refresh_inputs(&root, &dirs).unwrap(),
            "unchanged: not rewritten (its time would move)"
        );
        std::fs::write(root.join("core/src/extra.rs"), "").unwrap();
        assert!(refresh_inputs(&root, &dirs).unwrap());
        let text = std::fs::read_to_string(root.join(INPUTS_LIST)).unwrap();
        assert!(text.contains("$(SRCROOT)/../core/src/extra.rs\n"), "{text}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_stamp_is_newer_after_every_build() {
        let build = unique_temp_dir("xcode-stamp-touch");
        let stamp = write_stamp(&build, "Debug").unwrap();
        let first = std::fs::metadata(&stamp).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        write_stamp(&build, "Debug").unwrap();
        let second = std::fs::metadata(&stamp).unwrap().modified().unwrap();
        assert!(second > first, "the stamp is touched on every build");
        let _ = std::fs::remove_dir_all(build);
    }
}
