//! Where the platform runtimes (Swift, Kotlin, TypeScript) are found.
//!
//! The generated bindings and the app shells depend on `UndraRuntime`, `dev.undra:runtime` and
//! `@undra/runtime`. In a checkout of the Undra repository they are used straight from it (no
//! publishing step, edits show immediately); otherwise they come from the package registries at
//! the project's Undra version.

use std::path::{Path, PathBuf};

use crate::error::{CliError, Code, Result};
use crate::project::Project;

/// Where one runtime is found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeRef {
    /// A directory (absolute) in a checkout.
    Path(PathBuf),
    /// The package registry, at this version.
    Registry {
        /// The version requested.
        version: String,
    },
}

/// Where the three runtimes are found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runtimes {
    /// The `UndraRuntime` Swift package directory.
    pub swift: RuntimeRef,
    /// The Gradle build of the Kotlin runtime (`runtimes/kotlin/undra-runtime`).
    pub kotlin: RuntimeRef,
    /// The `@undra/runtime` npm package directory.
    pub ts: RuntimeRef,
}

/// Where a runtime lives inside an Undra checkout.
pub const SWIFT_IN_REPO: &str = "runtimes/swift/UndraRuntime";
/// Where the Kotlin runtime lives inside an Undra checkout.
pub const KOTLIN_IN_REPO: &str = "runtimes/kotlin/undra-runtime";
/// Where the TypeScript runtime lives inside an Undra checkout.
pub const TS_IN_REPO: &str = "runtimes/ts/@undra/runtime";

impl Runtimes {
    /// The runtimes of `project`: `[runtimes]` overrides, else the checkout of `[undra] path`,
    /// else the registries.
    #[must_use]
    pub fn for_project(project: &Project) -> Runtimes {
        let repo = project.undra_repo();
        let pick = |over: &Option<String>, in_repo: &str| -> RuntimeRef {
            if let Some(path) = over {
                let joined = project.root.join(path);
                return RuntimeRef::Path(joined.canonicalize().unwrap_or(joined));
            }
            match &repo {
                Some(repo) => RuntimeRef::Path(repo.join(in_repo)),
                None => RuntimeRef::Registry {
                    version: project.config.undra_version.clone(),
                },
            }
        };
        Runtimes {
            swift: pick(&project.config.runtimes.swift, SWIFT_IN_REPO),
            kotlin: pick(&project.config.runtimes.kotlin, KOTLIN_IN_REPO),
            ts: pick(&project.config.runtimes.ts, TS_IN_REPO),
        }
    }

    /// Runtimes taken from a checkout.
    #[must_use]
    pub fn in_repo(repo: &Path) -> Runtimes {
        Runtimes {
            swift: RuntimeRef::Path(repo.join(SWIFT_IN_REPO)),
            kotlin: RuntimeRef::Path(repo.join(KOTLIN_IN_REPO)),
            ts: RuntimeRef::Path(repo.join(TS_IN_REPO)),
        }
    }

    /// Runtimes taken from the registries.
    #[must_use]
    pub fn from_registries(version: &str) -> Runtimes {
        let registry = RuntimeRef::Registry {
            version: version.to_owned(),
        };
        Runtimes {
            swift: registry.clone(),
            kotlin: registry.clone(),
            ts: registry,
        }
    }
}

/// What a checkout of the Undra repository has: the crates the core links and the runtimes the
/// shells use.
pub(crate) const CHECKOUT_FILES: [&str; 6] = [
    "crates/undra/Cargo.toml",
    "crates/undra-ffi/Cargo.toml",
    "crates/undra-transport/Cargo.toml",
    "runtimes/swift/UndraRuntime/Package.swift",
    "runtimes/kotlin/undra-runtime/settings.gradle.kts",
    "runtimes/ts/@undra/runtime/package.json",
];

/// The first file `repo` lacks to be a checkout of the Undra repository, if any.
fn missing_from_checkout(repo: &Path) -> Option<&'static str> {
    CHECKOUT_FILES
        .into_iter()
        .find(|needed| !repo.join(needed).is_file())
}

/// Checks that `repo` is a checkout of the Undra repository: it has the crates and the runtimes.
///
/// # Errors
///
/// `C0009` naming what is missing.
pub fn require_checkout(repo: &Path) -> Result<()> {
    match missing_from_checkout(repo) {
        None => Ok(()),
        Some(needed) => Err(CliError::new(
            Code::BadArgument,
            format!(
                "{} does not look like a checkout of the Undra repository: {needed} is missing",
                repo.display()
            ),
            "with `--undra-path` the crates and the platform runtimes are used straight from the checkout, so all of them have to be there",
            "point `--undra-path` at the repository root (the directory that holds `crates/` and `runtimes/`)",
        )),
    }
}

/// The checkout of the Undra repository that `dir` is inside (or is), when there is one: the
/// nearest directory at or above `dir` that has the crates and the runtimes. A project created
/// there depends on the checkout by path, as with `--undra-path`, because the release it would
/// otherwise pin is not the code being worked on.
#[must_use]
pub fn enclosing_checkout(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|candidate| missing_from_checkout(candidate).is_none())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Platform, ProjectConfig};

    fn project(root: &str, undra_path: Option<&str>) -> Project {
        let mut config = ProjectConfig::new("demo", "com.example.demo", vec![Platform::Web]);
        config.undra_path = undra_path.map(ToOwned::to_owned);
        Project {
            root: PathBuf::from(root),
            config,
        }
    }

    #[test]
    fn registry_when_there_is_no_checkout() {
        let r = Runtimes::for_project(&project("/p", None));
        assert_eq!(r, Runtimes::from_registries(crate::config::UNDRA_VERSION));
    }

    #[test]
    fn checkout_paths_when_undra_path_is_set() {
        let r = Runtimes::for_project(&project("/nonexistent/p", Some("../undra")));
        assert!(
            matches!(&r.swift, RuntimeRef::Path(p) if p.ends_with("undra/runtimes/swift/UndraRuntime")),
            "{r:?}"
        );
        assert!(
            matches!(&r.ts, RuntimeRef::Path(p) if p.ends_with("@undra/runtime")),
            "{r:?}"
        );
    }

    #[test]
    fn overrides_win() {
        let mut p = project("/nonexistent/p", Some("../undra"));
        p.config.runtimes.kotlin = Some("vendor/kotlin".into());
        let r = Runtimes::for_project(&p);
        assert_eq!(
            r.kotlin,
            RuntimeRef::Path(PathBuf::from("/nonexistent/p/vendor/kotlin"))
        );
    }

    #[test]
    fn a_directory_that_is_not_a_checkout_is_rejected() {
        let e = require_checkout(Path::new("/definitely/not/undra")).unwrap_err();
        assert_eq!(e.code, Code::BadArgument);
        assert!(e.what.contains("crates/undra/Cargo.toml"), "{e}");
    }

    /// A directory tree with the files of a checkout.
    fn fake_checkout(root: &Path) {
        for file in CHECKOUT_FILES {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
    }

    #[test]
    fn a_directory_inside_a_checkout_finds_it() {
        let root = crate::fsutil::unique_temp_dir("enclosing");
        fake_checkout(&root);
        let inside = root.join("examples/app/core");
        std::fs::create_dir_all(&inside).unwrap();
        assert_eq!(enclosing_checkout(&inside), Some(root.clone()));
        assert_eq!(enclosing_checkout(&root), Some(root.clone()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_directory_outside_any_checkout_finds_none() {
        let dir = crate::fsutil::unique_temp_dir("not-enclosed");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(enclosing_checkout(&dir), None);
        // Half a checkout is not one: the runtimes are missing.
        std::fs::create_dir_all(dir.join("crates/undra")).unwrap();
        std::fs::write(dir.join("crates/undra/Cargo.toml"), "").unwrap();
        assert_eq!(enclosing_checkout(&dir), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
