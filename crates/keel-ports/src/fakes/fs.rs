//! [`MemFs`]: an in-memory file system.

use std::collections::{BTreeMap, BTreeSet};

use keel_wire::Bytes;
use parking_lot::Mutex;

use crate::{Fs, FsError};

#[derive(Default)]
struct Tree {
    /// Files by normalised path (`a/b/c`, no leading or trailing slash).
    files: BTreeMap<String, Vec<u8>>,
    /// Directories by normalised path. The root (`""`) is implicit.
    dirs: BTreeSet<String>,
}

/// An in-memory [`Fs`] with the semantics the platform adapters share.
///
/// * Paths are `/`-separated and relative to the root; empty and `.` segments are ignored and
///   a `..` segment is [`FsError::Denied`], so nothing can leave the root.
/// * `write` creates missing directories and replaces an existing file.
/// * `read` of a missing path is [`FsError::NotFound`]; of a directory,
///   [`FsError::Io`]. An empty path (the root) is `Io("the path is empty")` for `read`,
///   `write` and `delete`.
/// * `delete` removes a file, or a directory with everything under it; a missing path is
///   `NotFound`.
/// * `list` answers the names (not paths) of the entries directly inside a directory, files
///   and directories together, ascending. `list("")` lists the root.
///
/// ```
/// use keel_ports::{Fs, FsError};
/// use keel_ports::fakes::MemFs;
/// use keel_runtime::testing::TestRuntime;
/// use keel_wire::Bytes;
///
/// let fs = MemFs::new();
/// let t = TestRuntime::new();
/// t.run_until(fs.write("docs/notes/a.txt".into(), Bytes(b"hi".to_vec()))).unwrap();
/// assert_eq!(t.run_until(fs.list("docs".into())), Ok(vec!["notes".to_owned()]));
/// assert_eq!(t.run_until(fs.read("docs/missing".into())), Err(FsError::NotFound));
/// ```
#[derive(Default)]
pub struct MemFs {
    tree: Mutex<Tree>,
}

impl core::fmt::Debug for MemFs {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let tree = self.tree.lock();
        f.debug_struct("MemFs")
            .field("files", &tree.files.len())
            .field("dirs", &tree.dirs.len())
            .finish()
    }
}

/// Splits `path` into segments, dropping empty and `.` ones; `..` is denied.
fn segments(path: &str) -> Result<Vec<&str>, FsError> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(FsError::Denied),
            part => parts.push(part),
        }
    }
    Ok(parts)
}

fn empty_path() -> FsError {
    FsError::Io("the path is empty".to_owned())
}

impl MemFs {
    /// An empty file system.
    pub fn new() -> MemFs {
        MemFs::default()
    }

    /// Creates the file at `path` (and its directories) without going through the port: seeds a
    /// test. Fails like a `write` would (an empty path, `..`, a file in the way).
    pub fn seed(&self, path: &str, contents: impl Into<Vec<u8>>) -> Result<(), FsError> {
        self.write_file(path, contents.into())
    }

    /// The contents of the file at `path`, read directly. `None` if there is no such file.
    pub fn contents(&self, path: &str) -> Option<Vec<u8>> {
        let key = segments(path).ok()?.join("/");
        self.tree.lock().files.get(&key).cloned()
    }

    /// The path of every file, ascending.
    pub fn file_paths(&self) -> Vec<String> {
        self.tree.lock().files.keys().cloned().collect()
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>, FsError> {
        let parts = segments(path)?;
        if parts.is_empty() {
            return Err(empty_path());
        }
        let key = parts.join("/");
        let tree = self.tree.lock();
        match tree.files.get(&key) {
            Some(bytes) => Ok(bytes.clone()),
            None if tree.dirs.contains(&key) => Err(FsError::Io("is a directory".to_owned())),
            None => Err(FsError::NotFound),
        }
    }

    fn write_file(&self, path: &str, data: Vec<u8>) -> Result<(), FsError> {
        let parts = segments(path)?;
        let Some((_, parents)) = parts.split_last() else {
            return Err(empty_path());
        };
        let key = parts.join("/");
        let mut tree = self.tree.lock();
        if tree.dirs.contains(&key) {
            return Err(FsError::Io("is a directory".to_owned()));
        }
        let mut prefix = String::new();
        for parent in parents {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(parent);
            if tree.files.contains_key(&prefix) {
                return Err(FsError::Io("not a directory".to_owned()));
            }
        }
        let mut prefix = String::new();
        for parent in parents {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(parent);
            tree.dirs.insert(prefix.clone());
        }
        tree.files.insert(key, data);
        Ok(())
    }

    fn delete_path(&self, path: &str) -> Result<(), FsError> {
        let parts = segments(path)?;
        if parts.is_empty() {
            return Err(empty_path());
        }
        let key = parts.join("/");
        let mut tree = self.tree.lock();
        if tree.files.remove(&key).is_some() {
            return Ok(());
        }
        if !tree.dirs.remove(&key) {
            return Err(FsError::NotFound);
        }
        let below = format!("{key}/");
        tree.files.retain(|path, _| !path.starts_with(&below));
        tree.dirs.retain(|path| !path.starts_with(&below));
        Ok(())
    }

    fn list_dir(&self, dir: &str) -> Result<Vec<String>, FsError> {
        let parts = segments(dir)?;
        let key = parts.join("/");
        let tree = self.tree.lock();
        if !parts.is_empty() {
            if tree.files.contains_key(&key) {
                return Err(FsError::Io("not a directory".to_owned()));
            }
            if !tree.dirs.contains(&key) {
                return Err(FsError::NotFound);
            }
        }
        let prefix = if parts.is_empty() {
            String::new()
        } else {
            format!("{key}/")
        };
        let names: BTreeSet<&str> = tree
            .files
            .keys()
            .chain(tree.dirs.iter())
            .filter_map(|path| path.strip_prefix(prefix.as_str()))
            .filter_map(|rest| rest.split('/').next())
            .filter(|name| !name.is_empty())
            .collect();
        Ok(names.into_iter().map(str::to_owned).collect())
    }
}

#[keel_macros::port]
impl Fs for MemFs {
    async fn read(&self, path: String) -> Result<Bytes, FsError> {
        self.read_file(&path).map(Bytes)
    }

    async fn write(&self, path: String, data: Bytes) -> Result<(), FsError> {
        self.write_file(&path, data.0)
    }

    async fn delete(&self, path: String) -> Result<(), FsError> {
        self.delete_path(&path)
    }

    async fn list(&self, dir: String) -> Result<Vec<String>, FsError> {
        self.list_dir(&dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::testing::block_on;

    fn write(fs: &MemFs, path: &str, data: &[u8]) -> Result<(), FsError> {
        block_on(fs.write(path.into(), Bytes(data.to_vec())))
    }

    fn read(fs: &MemFs, path: &str) -> Result<Vec<u8>, FsError> {
        block_on(fs.read(path.into())).map(|b| b.0)
    }

    fn list(fs: &MemFs, dir: &str) -> Result<Vec<String>, FsError> {
        block_on(fs.list(dir.into()))
    }

    #[test]
    fn write_read_round_trip_and_replace() {
        let fs = MemFs::new();
        write(&fs, "a.txt", b"one").unwrap();
        assert_eq!(read(&fs, "a.txt"), Ok(b"one".to_vec()));
        write(&fs, "a.txt", b"two").unwrap();
        assert_eq!(read(&fs, "a.txt"), Ok(b"two".to_vec()));
        write(&fs, "empty", b"").unwrap();
        assert_eq!(read(&fs, "empty"), Ok(vec![]));
    }

    #[test]
    fn write_creates_directories_and_paths_are_normalised() {
        let fs = MemFs::new();
        write(&fs, "/a//b/./c.txt", b"x").unwrap();
        assert_eq!(read(&fs, "a/b/c.txt"), Ok(b"x".to_vec()));
        assert_eq!(list(&fs, ""), Ok(vec!["a".to_owned()]));
        assert_eq!(list(&fs, "a"), Ok(vec!["b".to_owned()]));
        assert_eq!(list(&fs, "a/b/"), Ok(vec!["c.txt".to_owned()]));
        assert_eq!(fs.file_paths(), ["a/b/c.txt"]);
        assert_eq!(fs.contents("./a/b/c.txt"), Some(b"x".to_vec()));
    }

    #[test]
    fn missing_paths_are_not_found() {
        let fs = MemFs::new();
        assert_eq!(read(&fs, "nope"), Err(FsError::NotFound));
        assert_eq!(list(&fs, "nope"), Err(FsError::NotFound));
        assert_eq!(block_on(fs.delete("nope".into())), Err(FsError::NotFound));
        assert_eq!(fs.contents("nope"), None);
    }

    #[test]
    fn dot_dot_is_denied_everywhere() {
        let fs = MemFs::new();
        assert_eq!(read(&fs, "../x"), Err(FsError::Denied));
        assert_eq!(write(&fs, "a/../../x", b""), Err(FsError::Denied));
        assert_eq!(list(&fs, ".."), Err(FsError::Denied));
        assert_eq!(block_on(fs.delete("..".into())), Err(FsError::Denied));
        assert_eq!(fs.contents(".."), None);
    }

    #[test]
    fn the_root_is_not_a_file() {
        let fs = MemFs::new();
        let empty = FsError::Io("the path is empty".into());
        assert_eq!(read(&fs, ""), Err(empty.clone()));
        assert_eq!(read(&fs, "/"), Err(empty.clone()));
        assert_eq!(write(&fs, ".", b""), Err(empty.clone()));
        assert_eq!(block_on(fs.delete("".into())), Err(empty));
        assert_eq!(list(&fs, ""), Ok(vec![]), "an empty root lists nothing");
    }

    #[test]
    fn files_and_directories_do_not_mix() {
        let fs = MemFs::new();
        write(&fs, "d/f", b"").unwrap();
        assert_eq!(read(&fs, "d"), Err(FsError::Io("is a directory".into())));
        assert_eq!(
            write(&fs, "d", b""),
            Err(FsError::Io("is a directory".into()))
        );
        assert_eq!(
            write(&fs, "d/f/g", b""),
            Err(FsError::Io("not a directory".into()))
        );
        assert_eq!(list(&fs, "d/f"), Err(FsError::Io("not a directory".into())));
        assert_eq!(read(&fs, "d/f/g"), Err(FsError::NotFound));
    }

    #[test]
    fn list_returns_sorted_unique_names_of_files_and_directories() {
        let fs = MemFs::new();
        for path in ["z", "d/one", "d/sub/two", "d/sub/three", "a"] {
            write(&fs, path, b"").unwrap();
        }
        assert_eq!(list(&fs, ""), Ok(vec!["a".into(), "d".into(), "z".into()]));
        assert_eq!(list(&fs, "d"), Ok(vec!["one".into(), "sub".into()]));
        assert_eq!(list(&fs, "d/sub"), Ok(vec!["three".into(), "two".into()]));
    }

    #[test]
    fn delete_removes_a_file_or_a_whole_directory() {
        let fs = MemFs::new();
        write(&fs, "d/a", b"").unwrap();
        write(&fs, "d/sub/b", b"").unwrap();
        write(&fs, "dd/c", b"").unwrap();
        block_on(fs.delete("d/a".into())).unwrap();
        assert_eq!(
            list(&fs, "d"),
            Ok(vec!["sub".into()]),
            "the directory stays"
        );
        block_on(fs.delete("d".into())).unwrap();
        assert_eq!(
            fs.file_paths(),
            ["dd/c"],
            "a sibling with a common prefix survives"
        );
        assert_eq!(list(&fs, "d"), Err(FsError::NotFound));
        block_on(fs.delete("dd/c".into())).unwrap();
        assert_eq!(
            list(&fs, "dd"),
            Ok(vec![]),
            "an emptied directory still exists"
        );
    }

    #[test]
    fn seed_refuses_paths_that_a_port_call_would_refuse() {
        let fs = MemFs::new();
        assert_eq!(
            fs.seed("", b"".to_vec()),
            Err(FsError::Io("the path is empty".into()))
        );
        assert_eq!(fs.seed("../x", b"".to_vec()), Err(FsError::Denied));
        assert!(fs.file_paths().is_empty());
    }

    #[test]
    fn seed_and_debug() {
        let fs = MemFs::new();
        fs.seed("x/y", b"z".to_vec()).unwrap();
        assert_eq!(read(&fs, "x/y"), Ok(b"z".to_vec()));
        assert!(format!("{fs:?}").contains("files: 1"));
    }
}
