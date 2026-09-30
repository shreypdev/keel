//! Where the platform runtimes (Swift, Kotlin, TypeScript) are found.
//!
//! The generated bindings and the app shells depend on `KeelRuntime`, `dev.keel:runtime` and
//! `@keel/runtime`. In a checkout of the Keel repository they are used straight from it (no
//! publishing step, edits show immediately); otherwise they come from the package registries at
//! the project's Keel version.

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
    /// The `KeelRuntime` Swift package directory.
    pub swift: RuntimeRef,
    /// The Gradle build of the Kotlin runtime (`runtimes/kotlin/keel-runtime`).
    pub kotlin: RuntimeRef,
    /// The `@keel/runtime` npm package directory.
    pub ts: RuntimeRef,
}

/// Where a runtime lives inside a Keel checkout.
pub const SWIFT_IN_REPO: &str = "runtimes/swift/KeelRuntime";
/// Where the Kotlin runtime lives inside a Keel checkout.
pub const KOTLIN_IN_REPO: &str = "runtimes/kotlin/keel-runtime";
/// Where the TypeScript runtime lives inside a Keel checkout.
pub const TS_IN_REPO: &str = "runtimes/ts/@keel/runtime";

impl Runtimes {
    /// The runtimes of `project`: `[runtimes]` overrides, else the checkout of `[keel] path`,
    /// else the registries.
    #[must_use]
    pub fn for_project(project: &Project) -> Runtimes {
        let repo = project.keel_repo();
        let pick = |over: &Option<String>, in_repo: &str| -> RuntimeRef {
            if let Some(path) = over {
                let joined = project.root.join(path);
                return RuntimeRef::Path(joined.canonicalize().unwrap_or(joined));
            }
            match &repo {
                Some(repo) => RuntimeRef::Path(repo.join(in_repo)),
                None => RuntimeRef::Registry {
                    version: project.config.keel_version.clone(),
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

/// Checks that `repo` is a checkout of the Keel repository: it has the crates and the runtimes.
///
/// # Errors
///
/// `C0009` naming what is missing.
pub fn require_checkout(repo: &Path) -> Result<()> {
    for needed in [
        "crates/keel/Cargo.toml",
        "crates/keel-ffi/Cargo.toml",
        "crates/keel-transport/Cargo.toml",
        "runtimes/swift/KeelRuntime/Package.swift",
        "runtimes/kotlin/keel-runtime/settings.gradle.kts",
        "runtimes/ts/@keel/runtime/package.json",
    ] {
        if !repo.join(needed).is_file() {
            return Err(CliError::new(
                Code::BadArgument,
                format!(
                    "{} does not look like a checkout of the Keel repository: {needed} is missing",
                    repo.display()
                ),
                "with `--keel-path` the crates and the platform runtimes are used straight from the checkout, so all of them have to be there",
                "point `--keel-path` at the repository root (the directory that holds `crates/` and `runtimes/`)",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Platform, ProjectConfig};

    fn project(root: &str, keel_path: Option<&str>) -> Project {
        let mut config = ProjectConfig::new("demo", "com.example.demo", vec![Platform::Web]);
        config.keel_path = keel_path.map(ToOwned::to_owned);
        Project {
            root: PathBuf::from(root),
            config,
        }
    }

    #[test]
    fn registry_when_there_is_no_checkout() {
        let r = Runtimes::for_project(&project("/p", None));
        assert_eq!(r, Runtimes::from_registries("0.1"));
    }

    #[test]
    fn checkout_paths_when_keel_path_is_set() {
        let r = Runtimes::for_project(&project("/nonexistent/p", Some("../keel")));
        assert!(
            matches!(&r.swift, RuntimeRef::Path(p) if p.ends_with("keel/runtimes/swift/KeelRuntime")),
            "{r:?}"
        );
        assert!(
            matches!(&r.ts, RuntimeRef::Path(p) if p.ends_with("@keel/runtime")),
            "{r:?}"
        );
    }

    #[test]
    fn overrides_win() {
        let mut p = project("/nonexistent/p", Some("../keel"));
        p.config.runtimes.kotlin = Some("vendor/kotlin".into());
        let r = Runtimes::for_project(&p);
        assert_eq!(
            r.kotlin,
            RuntimeRef::Path(PathBuf::from("/nonexistent/p/vendor/kotlin"))
        );
    }

    #[test]
    fn a_directory_that_is_not_a_checkout_is_rejected() {
        let e = require_checkout(Path::new("/definitely/not/keel")).unwrap_err();
        assert_eq!(e.code, Code::BadArgument);
        assert!(e.what.contains("crates/keel/Cargo.toml"), "{e}");
    }
}
