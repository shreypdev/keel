//! File-system helpers shared by the commands.

use std::fs;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use crate::error::{CliError, Result};

/// Creates `dir` and its parents.
///
/// # Errors
///
/// `C0010`.
pub fn create_dir_all(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| CliError::io("create", dir, &e))
}

/// Writes `contents` to `path`, creating parent directories, but only when the file does not
/// already hold exactly that: rewriting an identical file would change its modification time and
/// make Cargo, Gradle and Xcode rebuild what did not change. Returns whether it wrote.
///
/// # Errors
///
/// `C0010`.
pub fn write_if_changed(path: &Path, contents: &str) -> Result<bool> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    fs::write(path, contents).map_err(|e| CliError::io("write", path, &e))?;
    Ok(true)
}

/// Makes `path` executable (no-op where that does not exist).
///
/// # Errors
///
/// `C0010`.
pub fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .map_err(|e| CliError::io("read the permissions of", path, &e))?
            .permissions();
        perms.set_mode(perms.mode() | 0o111);
        fs::set_permissions(path, perms).map_err(|e| CliError::io("make executable", path, &e))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Copies `from` to `to`, creating parent directories, replacing what is there.
///
/// # Errors
///
/// `C0010`.
pub fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        create_dir_all(parent)?;
    }
    // Remove first: a read-only or hard-linked target would make `fs::copy` fail or write through.
    let _ = fs::remove_file(to);
    fs::copy(from, to).map_err(|e| CliError::io("copy to", to, &e))?;
    Ok(())
}

#[cfg(test)]
/// Copies a directory tree, following nothing: symlinks are recreated as files only when
/// they point at files.
///
/// # Errors
///
/// `C0010`.
pub fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    create_dir_all(to)?;
    let entries = fs::read_dir(from).map_err(|e| CliError::io("read", from, &e))?;
    for entry in entries {
        let entry = entry.map_err(|e| CliError::io("read", from, &e))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            copy_file(&src, &dst)?;
        }
    }
    Ok(())
}

/// Removes a directory tree if it exists.
///
/// # Errors
///
/// `C0010`.
pub fn remove_dir_all(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CliError::io("remove", dir, &e)),
    }
}

/// The size of a file, or of every file below a directory, in bytes.
#[must_use]
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_dir() {
        fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| size_of(&e.path()))
                    .sum()
            })
            .unwrap_or(0)
    } else {
        meta.len()
    }
}

/// `1234567` as `1.2 MB`.
#[must_use]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
/// Every file below `dir` (recursively), as paths relative to `dir` with `/` separators, sorted.
#[must_use]
pub fn list_files(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Whether `dir` does not exist or has no entries.
#[must_use]
pub fn is_empty_dir(dir: &Path) -> bool {
    fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none())
}

#[cfg(test)]
/// A fresh directory under the system temporary directory, for tests and scratch work; the
/// caller removes it.
#[must_use]
pub fn unique_temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("undra-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_if_changed_does_not_touch_identical_files() {
        let dir = unique_temp_dir("fsutil");
        let file = dir.join("a/b.txt");
        assert!(write_if_changed(&file, "one").unwrap());
        let before = fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(!write_if_changed(&file, "one").unwrap());
        assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), before);
        assert!(write_if_changed(&file, "two").unwrap());
        assert_eq!(fs::read_to_string(&file).unwrap(), "two");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1_500), "1.5 KB");
        assert_eq!(human_size(241_000), "241.0 KB");
        assert_eq!(human_size(12_345_678), "12.3 MB");
    }

    #[test]
    fn trees_copy_and_list() {
        let dir = unique_temp_dir("fsutil-tree");
        write_if_changed(&dir.join("src/a/x.txt"), "x").unwrap();
        write_if_changed(&dir.join("src/y.txt"), "yy").unwrap();
        copy_dir(&dir.join("src"), &dir.join("dst")).unwrap();
        assert_eq!(list_files(&dir.join("dst")), ["a/x.txt", "y.txt"]);
        assert_eq!(size_of(&dir.join("dst")), 3);
        assert!(is_empty_dir(&dir.join("nope")));
        assert!(!is_empty_dir(&dir.join("dst")));
        remove_dir_all(&dir).unwrap();
        remove_dir_all(&dir).unwrap();
    }
}
