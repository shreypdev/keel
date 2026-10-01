//! One invocation of a command against a project: the project, the machine, the toolchain and
//! what Cargo says about the core.

use std::cell::OnceCell;
use std::path::PathBuf;

use crate::cargo::{Cargo, CoreInfo, UndraSource};
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

    /// Cargo's target directory for everything the CLI builds, in this order:
    ///
    /// 1. `CARGO_TARGET_DIR`, when set;
    /// 2. the target directory of the Cargo workspace the core is a member of (`cargo metadata`
    ///    says where it is), so a core inside a bigger workspace shares one dependency build with
    ///    `cargo build`, `cargo test` and the rest of the workspace;
    /// 3. `<project>/target`.
    ///
    /// # Errors
    ///
    /// See [`Session::core`]: the workspace is read from Cargo's metadata of the core.
    pub fn target_dir(&self) -> Result<PathBuf> {
        if let Some(dir) = self.project.explicit_target_dir(self.sys) {
            return Ok(dir);
        }
        Ok(match &self.core()?.workspace {
            Some(workspace) => workspace.target_dir.clone(),
            None => self.project.local_target_dir(),
        })
    }

    /// What Cargo says about the core crate (read once per session).
    ///
    /// # Errors
    ///
    /// `C0005`, `C0004`, `C0003` (see [`Cargo::core_info`]) and `C0014` when `undra.toml` and the
    /// core disagree about where Undra comes from.
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
                    "undra.toml says the core is in `{}` ([core] path), and that directory has no Cargo.toml",
                    self.project.config.core_path
                ),
                "fix `[core] path` in undra.toml, or create a new project with `undra init <name>`",
            ));
        }
        let info = self.cargo().core_info(&manifest)?;
        self.check_undra_source(&info)?;
        Ok(self.core.get_or_init(|| info))
    }

    /// Checks that `[undra] path` and the core's own dependency on Undra name the same place: the
    /// runtimes come from one and the shim's `undra-ffi` from the other, and they must match.
    fn check_undra_source(&self, info: &CoreInfo) -> Result<()> {
        let Some(configured) = self.project.undra_repo() else {
            return Ok(());
        };
        match &info.undra {
            UndraSource::Path { repo } => {
                let actual = repo.canonicalize().unwrap_or_else(|_| repo.clone());
                if actual == configured {
                    return Ok(());
                }
                Err(mismatch(
                    &format!("undra.toml says Undra is at {}", configured.display()),
                    &format!("the core's `undra` dependency is at {}", actual.display()),
                ))
            }
            other => Err(mismatch(
                &format!("undra.toml says Undra is at {}", configured.display()),
                &format!(
                    "the core's `undra` dependency comes from {}",
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
            &self.target_dir()?,
            &self.project.root,
            core,
            &self.project.config.web.opt_level,
        )
    }
}

fn describe(source: &UndraSource) -> String {
    match source {
        UndraSource::Path { repo } => format!("the checkout at {}", repo.display()),
        UndraSource::Registry { version } => format!("the registry (version {version})"),
        UndraSource::Git { url, .. } => format!("git ({url})"),
    }
}

fn mismatch(configured: &str, actual: &str) -> CliError {
    CliError::new(
        Code::UndraMismatch,
        format!("{configured}, but {actual}"),
        "the library Undra ships is built from the same Undra as the core, and the platform runtimes come from `[undra] path`; if they differ, the core and the apps would speak different wire formats",
        "make them agree: set `path` of the core's `undra` dependency and `[undra] path` in undra.toml to the same checkout (or remove `[undra] path` to use released runtimes)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo::Workspace;
    use crate::config::{Platform, ProjectConfig};
    use crate::sys::fake::FakeSys;

    fn core_info(workspace: Option<Workspace>) -> CoreInfo {
        CoreInfo {
            package: "todo-core".into(),
            version: "0.1.0".into(),
            lib_name: "todo_core".into(),
            dir: PathBuf::from("/repo/apps/todo/core"),
            undra: UndraSource::Registry {
                version: "0.1.0".into(),
            },
            links_ports: false,
            local_dirs: Vec::new(),
            workspace,
        }
    }

    /// A session for the project at `/repo/apps/todo` whose core Cargo has already described.
    fn session<'a>(sys: &'a FakeSys, workspace: Option<Workspace>) -> Session<'a> {
        let project = Project {
            root: PathBuf::from("/repo/apps/todo"),
            config: ProjectConfig::new("todo", "com.example.todo", vec![Platform::Web]),
        };
        let session = Session::new(project, sys, Ui::plain());
        session.core.set(core_info(workspace)).unwrap();
        session
    }

    fn workspace() -> Workspace {
        Workspace {
            root: PathBuf::from("/repo"),
            target_dir: PathBuf::from("/repo/target"),
        }
    }

    #[test]
    fn a_core_in_a_workspace_builds_into_the_workspace_target() {
        let sys = FakeSys::macos();
        assert_eq!(
            session(&sys, Some(workspace())).target_dir().unwrap(),
            PathBuf::from("/repo/target"),
            "one dependency build for cargo and undra"
        );
    }

    #[test]
    fn a_core_that_is_a_workspace_of_its_own_builds_into_the_project() {
        let sys = FakeSys::macos();
        assert_eq!(
            session(&sys, None).target_dir().unwrap(),
            PathBuf::from("/repo/apps/todo/target")
        );
    }

    #[test]
    fn cargo_target_dir_wins_over_the_workspace() {
        let absolute = FakeSys::macos().with_env("CARGO_TARGET_DIR", "/fast/target");
        assert_eq!(
            session(&absolute, Some(workspace())).target_dir().unwrap(),
            PathBuf::from("/fast/target")
        );
        let relative = FakeSys::macos().with_env("CARGO_TARGET_DIR", "out");
        assert_eq!(
            session(&relative, None).target_dir().unwrap(),
            PathBuf::from("/repo/apps/todo/out"),
            "relative to the project, as it always was"
        );
    }

    #[test]
    fn mismatches_teach() {
        let e = mismatch(
            "undra.toml says Undra is at /a",
            "the core's `undra` dependency is at /b",
        );
        assert_eq!(e.code, Code::UndraMismatch);
        assert!(e.what.contains("/a") && e.what.contains("/b"), "{e}");
        assert!(e.fix.contains("[undra] path"), "{e}");
    }

    #[test]
    fn sources_are_described() {
        assert!(
            describe(&UndraSource::Registry {
                version: "0.1.0".into()
            })
            .contains("registry")
        );
        assert!(
            describe(&UndraSource::Git {
                url: "u".into(),
                reference: crate::cargo::GitRef::DefaultBranch
            })
            .contains("git")
        );
    }
}
