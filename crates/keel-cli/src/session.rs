//! One invocation of a command against a project: the project, the machine, the toolchain and
//! what Cargo says about the core.

use std::cell::OnceCell;
use std::path::PathBuf;

use crate::cargo::{Cargo, CoreInfo, KeelSource};
use crate::error::{CliError, Code, Result};
use crate::project::Project;
use crate::shim;
use crate::sys::Sys;
use crate::toolchain::Toolchain;
use crate::ui::Ui;

/// A project being worked on.
pub struct Session<'a> {
    /// The project.
    pub project: Project,
    /// The machine.
    pub sys: &'a dyn Sys,
    /// The detected toolchain environment.
    pub toolchain: Toolchain,
    /// Output.
    pub ui: Ui,
    core: OnceCell<CoreInfo>,
}

impl<'a> Session<'a> {
    /// Starts a session for `project`.
    #[must_use]
    pub fn new(project: Project, sys: &'a dyn Sys, ui: Ui) -> Session<'a> {
        let toolchain = Toolchain::detect(sys);
        Session {
            project,
            sys,
            toolchain,
            ui,
            core: OnceCell::new(),
        }
    }

    /// A Cargo runner for this session.
    #[must_use]
    pub fn cargo(&self) -> Cargo<'_> {
        Cargo {
            toolchain: &self.toolchain,
            sys: self.sys,
        }
    }

    /// Cargo's target directory for everything the CLI builds.
    #[must_use]
    pub fn target_dir(&self) -> PathBuf {
        self.project.target_dir()
    }

    /// What Cargo says about the core crate (read once per session).
    ///
    /// # Errors
    ///
    /// `C0005`, `C0004`, `C0003` (see [`Cargo::core_info`]) and `C0014` when `keel.toml` and the
    /// core disagree about where Keel comes from.
    pub fn core(&self) -> Result<&CoreInfo> {
        if let Some(core) = self.core.get() {
            return Ok(core);
        }
        let manifest = self.project.core_manifest();
        if !manifest.is_file() {
            return Err(CliError::new(
                Code::BadCore,
                format!("there is no core crate at {}", manifest.display()),
                format!(
                    "keel.toml says the core is in `{}` ([core] path), and that directory has no Cargo.toml",
                    self.project.config.core_path
                ),
                "fix `[core] path` in keel.toml, or create a new project with `keel init <name>`",
            ));
        }
        let info = self.cargo().core_info(&manifest)?;
        self.check_keel_source(&info)?;
        Ok(self.core.get_or_init(|| info))
    }

    /// Checks that `[keel] path` and the core's own dependency on Keel name the same place: the
    /// runtimes come from one and the shim's `keel-ffi` from the other, and they must match.
    fn check_keel_source(&self, info: &CoreInfo) -> Result<()> {
        let Some(configured) = self.project.keel_repo() else {
            return Ok(());
        };
        match &info.keel {
            KeelSource::Path { repo } => {
                let actual = repo.canonicalize().unwrap_or_else(|_| repo.clone());
                if actual == configured {
                    return Ok(());
                }
                Err(mismatch(
                    &format!("keel.toml says Keel is at {}", configured.display()),
                    &format!("the core's `keel` dependency is at {}", actual.display()),
                ))
            }
            other => Err(mismatch(
                &format!("keel.toml says Keel is at {}", configured.display()),
                &format!(
                    "the core's `keel` dependency comes from {}",
                    describe(other)
                ),
            )),
        }
    }

    /// Writes the shim crate for this project and returns its manifest.
    ///
    /// # Errors
    ///
    /// See [`Session::core`] and [`shim::write_shim`].
    pub fn shim_manifest(&self) -> Result<PathBuf> {
        let core = self.core()?;
        shim::write_shim(
            &self.target_dir(),
            &self.project.root,
            core,
            &self.project.config.web.opt_level,
        )
    }
}

fn describe(source: &KeelSource) -> String {
    match source {
        KeelSource::Path { repo } => format!("the checkout at {}", repo.display()),
        KeelSource::Registry { version } => format!("the registry (version {version})"),
        KeelSource::Git { url, .. } => format!("git ({url})"),
    }
}

fn mismatch(configured: &str, actual: &str) -> CliError {
    CliError::new(
        Code::KeelMismatch,
        format!("{configured}, but {actual}"),
        "the library Keel ships is built from the same Keel as the core, and the platform runtimes come from `[keel] path`; if they differ, the core and the apps would speak different wire formats",
        "make them agree: set `path` of the core's `keel` dependency and `[keel] path` in keel.toml to the same checkout (or remove `[keel] path` to use released runtimes)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatches_teach() {
        let e = mismatch(
            "keel.toml says Keel is at /a",
            "the core's `keel` dependency is at /b",
        );
        assert_eq!(e.code, Code::KeelMismatch);
        assert!(e.what.contains("/a") && e.what.contains("/b"), "{e}");
        assert!(e.fix.contains("[keel] path"), "{e}");
    }

    #[test]
    fn sources_are_described() {
        assert!(
            describe(&KeelSource::Registry {
                version: "0.1.0".into()
            })
            .contains("registry")
        );
        assert!(
            describe(&KeelSource::Git {
                url: "u".into(),
                rev: None
            })
            .contains("git")
        );
    }
}
