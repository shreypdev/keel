//! One invocation of a command against a project: the project, the machine, the toolchain and
//! what Cargo says about the core.

use std::cell::OnceCell;
use std::path::PathBuf;

use undra_bindgen::naming::CoreNames;

use crate::cargo::{Cargo, CoreInfo, UndraSource};
use crate::config::{ProjectConfig, check_namespace};
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

    /// The core's namespace (ADR-044): `[core] namespace`, else the core's package name in snake
    /// case. It names the core's one C export, its libraries and the generated entry point, so two
    /// cores in one app need two; a project next to this one (a sibling directory with its own
    /// `undra.toml` and another `[project] id`) that claims the same namespace is refused here,
    /// before anything is built with it.
    ///
    /// # Errors
    ///
    /// `C0002` when the derived default is not a valid namespace (set one), or when a sibling
    /// project uses the same namespace; what [`Session::core`] returns when the default needs it.
    pub fn namespace(&self) -> Result<String> {
        let namespace = match &self.project.config.core_namespace {
            Some(namespace) => namespace.clone(),
            None => {
                let derived = CoreNames::default_namespace(&self.core()?.package);
                if let Err(why) = check_namespace(&derived) {
                    return Err(CliError::new(
                        Code::BadConfig,
                        format!("the core's default namespace `{derived}` is not usable: {why}"),
                        "the namespace names the core's C symbol and libraries; by default it is the core's package name in snake case",
                        "set one in undra.toml: `[core] namespace = \"acme_pay\"` (lowercase letters, digits and `_`, at most 32 characters)",
                    ));
                }
                derived
            }
        };
        if let Some(other) =
            sibling_with_namespace(&self.project.root, &self.project.config.id, &namespace)
        {
            return Err(CliError::new(
                Code::BadConfig,
                format!(
                    "the core namespace `{namespace}` is also the namespace of the project at {}",
                    other.display()
                ),
                "two cores in one app must have different namespaces: the namespace names each core's symbol and libraries, and the runtimes refuse to load two cores with one (ADR-044)",
                "give one of them its own `[core] namespace` in undra.toml",
            ));
        }
        Ok(namespace)
    }

    /// The names derived from [`Session::namespace`].
    ///
    /// # Errors
    ///
    /// See [`Session::namespace`].
    pub fn core_names(&self) -> Result<CoreNames> {
        Ok(CoreNames::new(&self.namespace()?))
    }

    /// Writes the shim crate for this project and returns its manifest.
    ///
    /// # Errors
    ///
    /// See [`Session::core`], [`Session::namespace`] and [`shim::write_shim`].
    pub fn shim_manifest(&self) -> Result<PathBuf> {
        let core = self.core()?;
        let namespace = self.namespace()?;
        shim::write_shim(
            &self.target_dir()?,
            &self.project.root,
            core,
            &shim::ShimNames {
                namespace,
                jni_class: CoreNames::jni_class(&self.project.kotlin_package()),
            },
            &self.project.config.web.opt_level,
        )
    }
}

/// A project in a directory next to `root` (another child of its parent) whose `undra.toml` sets
/// `[core] namespace = namespace`, or derives it from a core whose package is that name. A sibling
/// with this project's own `[project] id` (`own_id`) is another checkout of the same project (a
/// `git worktree`, a copy), not a second core of the app, and never counts.
fn sibling_with_namespace(
    root: &std::path::Path,
    own_id: &str,
    namespace: &str,
) -> Option<PathBuf> {
    let parent = root.parent()?;
    let own = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut siblings: Vec<PathBuf> = std::fs::read_dir(parent)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|dir| dir.join("undra.toml").is_file())
        .collect();
    siblings.sort();
    siblings.into_iter().find(|dir| {
        if dir.canonicalize().unwrap_or_else(|_| dir.clone()) == own {
            return false;
        }
        let Ok(text) = std::fs::read_to_string(dir.join("undra.toml")) else {
            return false;
        };
        let Ok(config) = ProjectConfig::parse(&text, &dir.join("undra.toml")) else {
            return false;
        };
        if config.id == own_id {
            return false;
        }
        let theirs = config.core_namespace.clone().or_else(|| {
            package_name(&dir.join(&config.core_path).join("Cargo.toml"))
                .map(|package| CoreNames::default_namespace(&package))
        });
        theirs.as_deref() == Some(namespace)
    })
}

/// The `[package] name` of a `Cargo.toml`, read without Cargo (a cheap look at a sibling).
fn package_name(manifest: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = line.strip_prefix("name") {
                let value = rest.trim_start().strip_prefix('=')?.trim();
                return Some(value.trim_matches('"').to_owned());
            }
        }
    }
    None
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

    /// Writes `<parent>/<dir>/undra.toml` for project `id`, with `namespace` when given and a core
    /// whose package is `package`.
    fn project_at(
        parent: &std::path::Path,
        dir: &str,
        id: &str,
        namespace: Option<&str>,
        package: &str,
    ) {
        let root = parent.join(dir);
        std::fs::create_dir_all(root.join("core")).unwrap();
        let namespace = namespace.map_or(String::new(), |ns| format!("namespace = \"{ns}\"\n"));
        std::fs::write(
            root.join("undra.toml"),
            format!("[project]\nname = \"{dir}\"\nid = \"{id}\"\nplatforms = [\"web\"]\n\n[core]\npath = \"core\"\n{namespace}"),
        )
        .unwrap();
        std::fs::write(
            root.join("core/Cargo.toml"),
            format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\n"),
        )
        .unwrap();
    }

    /// ADR-044 (deviation): a project next to this one that claims the same namespace is refused,
    /// whether it sets the namespace or derives it from its core's package name.
    #[test]
    fn a_sibling_project_with_the_same_namespace_is_found() {
        let parent = crate::fsutil::unique_temp_dir("siblings");
        project_at(&parent, "app", "com.example.app", Some("acme"), "app-core");
        project_at(
            &parent,
            "vendor",
            "com.vendor.sdk",
            Some("acme"),
            "vendor-core",
        );
        project_at(&parent, "other", "com.other.app", None, "acme");
        project_at(
            &parent,
            "unrelated",
            "com.unrelated",
            Some("zeta"),
            "zeta-core",
        );
        let found = sibling_with_namespace(&parent.join("app"), "com.example.app", "acme");
        assert_eq!(
            found.as_deref(),
            Some(parent.join("other").as_path()),
            "derived from `acme`"
        );
        std::fs::remove_dir_all(parent.join("other")).unwrap();
        let found = sibling_with_namespace(&parent.join("app"), "com.example.app", "acme");
        assert_eq!(found.as_deref(), Some(parent.join("vendor").as_path()));
        assert_eq!(
            sibling_with_namespace(&parent.join("app"), "com.example.app", "zeta").as_deref(),
            Some(parent.join("unrelated").as_path())
        );
        assert_eq!(
            sibling_with_namespace(&parent.join("app"), "com.example.app", "free"),
            None
        );
        let _ = std::fs::remove_dir_all(&parent);
    }

    /// Review (abi-table): another checkout of the same project (a `git worktree` next to it, a
    /// copy) has the same namespace by construction and is not a second core of the app; it must
    /// not stop either from building.
    #[test]
    fn another_checkout_of_the_same_project_is_not_a_clash() {
        let parent = crate::fsutil::unique_temp_dir("worktrees");
        project_at(&parent, "app", "com.example.app", Some("acme"), "app-core");
        project_at(
            &parent,
            "app-feature",
            "com.example.app",
            Some("acme"),
            "app-core",
        );
        project_at(&parent, "app-copy", "com.example.app", None, "acme");
        assert_eq!(
            sibling_with_namespace(&parent.join("app"), "com.example.app", "acme"),
            None
        );
        assert_eq!(
            sibling_with_namespace(&parent.join("app-feature"), "com.example.app", "acme"),
            None
        );
        let _ = std::fs::remove_dir_all(&parent);
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
