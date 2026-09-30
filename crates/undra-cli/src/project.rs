//! The project on disk: where `undra.toml` is and where everything the CLI writes goes.

use std::path::{Path, PathBuf};

use crate::config::ProjectConfig;
use crate::error::{CliError, Result};
use crate::names::Names;
use crate::sys::Sys;

/// The file that makes a directory an Undra project.
pub const CONFIG_FILE: &str = "undra.toml";

/// An Undra project: its root directory and its `undra.toml`.
#[derive(Clone, Debug)]
pub struct Project {
    /// The directory that holds `undra.toml`.
    pub root: PathBuf,
    /// The parsed `undra.toml`.
    pub config: ProjectConfig,
}

impl Project {
    /// Finds the project containing `start`: the nearest directory, `start` itself or a parent,
    /// that holds an `undra.toml`.
    ///
    /// # Errors
    ///
    /// `C0001` when there is none, `C0002` when the file is invalid, `C0010` when it cannot be
    /// read.
    pub fn discover(start: &Path) -> Result<Project> {
        let start = start
            .canonicalize()
            .map_err(|e| CliError::io("open", start, &e))?;
        let mut dir = Some(start.as_path());
        while let Some(d) = dir {
            if d.join(CONFIG_FILE).is_file() {
                return Project::open(d);
            }
            dir = d.parent();
        }
        Err(CliError::no_project(&start))
    }

    /// Reads the `undra.toml` of the project rooted at `root`.
    ///
    /// # Errors
    ///
    /// `C0010` when the file cannot be read, `C0002` when it is invalid.
    pub fn open(root: &Path) -> Result<Project> {
        let file = root.join(CONFIG_FILE);
        let text = std::fs::read_to_string(&file).map_err(|e| CliError::io("read", &file, &e))?;
        let config = ProjectConfig::parse(&text, &file)?;
        Ok(Project {
            root: root.to_path_buf(),
            config,
        })
    }

    /// The directory of the core crate.
    #[must_use]
    pub fn core_dir(&self) -> PathBuf {
        self.root.join(&self.config.core_path)
    }

    /// The core crate's `Cargo.toml`.
    #[must_use]
    pub fn core_manifest(&self) -> PathBuf {
        self.core_dir().join("Cargo.toml")
    }

    /// Where `undra bindgen` writes by default.
    #[must_use]
    pub fn generated_dir(&self) -> PathBuf {
        self.root.join(&self.config.generated)
    }

    /// Where `undra build` writes artifacts.
    #[must_use]
    pub fn build_dir(&self) -> PathBuf {
        self.root.join(&self.config.build)
    }

    /// The target directory the environment asks for: `CARGO_TARGET_DIR` when set (a relative one
    /// is relative to the project root).
    #[must_use]
    pub fn explicit_target_dir(&self, sys: &dyn Sys) -> Option<PathBuf> {
        let dir = PathBuf::from(sys.env("CARGO_TARGET_DIR")?);
        Some(if dir.is_absolute() {
            dir
        } else {
            self.root.join(dir)
        })
    }

    /// The project's own target directory, `<root>/target`: where everything the CLI builds goes
    /// unless the environment or the core's Cargo workspace says otherwise (see
    /// [`crate::session::Session::target_dir`]). The generated shim and dev runner live below it,
    /// in `undra/`.
    #[must_use]
    pub fn local_target_dir(&self) -> PathBuf {
        self.root.join("target")
    }

    /// The names derived from the project name.
    #[must_use]
    pub fn names(&self) -> Names {
        Names::derive(&self.config.name)
    }

    /// The Kotlin package of the bindings: `[bindings] kotlin_package`, else `<id>.core`.
    #[must_use]
    pub fn kotlin_package(&self) -> String {
        self.config
            .bindings
            .kotlin_package
            .clone()
            .unwrap_or_else(|| format!("{}.core", self.config.id))
    }

    /// The Swift module of the bindings: `[bindings] swift_module`, else `<Pascal>Core`.
    #[must_use]
    pub fn swift_module(&self) -> String {
        self.config
            .bindings
            .swift_module
            .clone()
            .unwrap_or_else(|| self.names().swift_module)
    }

    /// The checkout of the Undra repository from `[undra] path`, resolved and absolute.
    #[must_use]
    pub fn undra_repo(&self) -> Option<PathBuf> {
        let path = self.config.undra_path.as_ref()?;
        let joined = self.root.join(path);
        Some(joined.canonicalize().unwrap_or(joined))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Platform;
    use crate::error::Code;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("undra-cli-unit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_config(dir: &Path) {
        let cfg = ProjectConfig::new("demo", "com.example.demo", vec![Platform::Web]);
        std::fs::write(dir.join(CONFIG_FILE), cfg.render()).unwrap();
    }

    #[test]
    fn discovery_walks_up_to_undra_toml() {
        let root = temp("discover");
        write_config(&root);
        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        let project = Project::discover(&nested).unwrap();
        assert_eq!(project.root, root.canonicalize().unwrap());
        assert_eq!(project.config.name, "demo");
        assert_eq!(
            project.core_manifest(),
            project.root.join("core/Cargo.toml")
        );
        assert_eq!(project.kotlin_package(), "com.example.demo.core");
        assert_eq!(project.swift_module(), "DemoCore");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn outside_a_project_the_error_teaches() {
        let root = temp("none");
        let e = Project::discover(&root).unwrap_err();
        assert_eq!(e.code, Code::NoProject);
        assert!(e.fix.contains("undra init"), "{e}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_invalid_config_names_the_file() {
        let root = temp("invalid");
        std::fs::write(root.join(CONFIG_FILE), "[project]\nname = 1\n").unwrap();
        let e = Project::open(&root).unwrap_err();
        assert_eq!(e.code, Code::BadConfig);
        assert!(e.what.contains("undra.toml"), "{e}");
        let _ = std::fs::remove_dir_all(root);
    }
}
