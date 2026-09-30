//! Cargo: learning what the core links, and running builds.
//!
//! The CLI never guesses where Undra comes from. It asks Cargo (`cargo metadata`) what the core
//! resolves `undra-runtime` to, and builds the library it ships (the *shim*, see
//! [`crate::shim`]) from the same source, so the core and the C ABI are the same crate instance:
//! two copies of `undra-runtime` would each have their own registry of `#[undra::api]` items and the
//! schema would come out empty.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::error::{CliError, Code, Result};
use crate::sys::Sys;
use crate::toolchain::Toolchain;

/// Where the Undra crates come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UndraSource {
    /// A checkout of the Undra repository (`<repo>/crates/undra-*`).
    Path {
        /// The repository root.
        repo: PathBuf,
    },
    /// A registry release; the crates are versioned in lockstep.
    Registry {
        /// The exact version the core resolved.
        version: String,
    },
    /// A git dependency.
    Git {
        /// The repository URL.
        url: String,
        /// The revision the core resolved.
        rev: Option<String>,
    },
}

impl UndraSource {
    /// The right-hand side of a Cargo dependency on the Undra crate `krate`, with `features`.
    ///
    /// ```text
    /// { path = "/src/undra/crates/undra-ffi", features = ["jni"] }
    /// ```
    #[must_use]
    pub fn dependency(&self, krate: &str, features: &[&str]) -> String {
        let mut parts = match self {
            UndraSource::Path { repo } => {
                let path = repo.join("crates").join(krate);
                format!(
                    "path = {}",
                    crate::toml_lite::quote(&path.to_string_lossy())
                )
            }
            UndraSource::Registry { version } => format!("version = \"={version}\""),
            UndraSource::Git { url, rev } => {
                let mut s = format!("git = {}", crate::toml_lite::quote(url));
                if let Some(rev) = rev {
                    s.push_str(&format!(", rev = {}", crate::toml_lite::quote(rev)));
                }
                s
            }
        };
        if !features.is_empty() {
            let list = features
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push_str(&format!(", features = [{list}]"));
        }
        format!("{{ {parts} }}")
    }
}

/// The Cargo workspace a core belongs to, when it is a member of one that is more than the core
/// crate alone (the Undra repository's own `examples/`, a monorepo with the core as one member).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// The directory of the workspace's `Cargo.toml`.
    pub root: PathBuf,
    /// Where Cargo builds the workspace (`target_directory` of `cargo metadata`, so `build.target-dir`
    /// and `CARGO_TARGET_DIR` are already taken into account).
    pub target_dir: PathBuf,
}

/// What the CLI needs to know about the core crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreInfo {
    /// The Cargo package name (`todo-core`).
    pub package: String,
    /// The package version.
    pub version: String,
    /// The library name (`todo_core`).
    pub lib_name: String,
    /// The package directory.
    pub dir: PathBuf,
    /// Where Undra comes from.
    pub undra: UndraSource,
    /// Whether the core links `undra-ports` (directly or through the `undra` facade), which decides
    /// whether the dev runner binds native ports.
    pub links_ports: bool,
    /// The directories of the path dependencies the core is built from (the core itself
    /// included, Undra's own crates excluded): what `undra dev` watches for changes.
    pub local_dirs: Vec<PathBuf>,
    /// The workspace the core is a member of, unless the core is a workspace of its own.
    pub workspace: Option<Workspace>,
}

/// Interprets the output of `cargo metadata --format-version 1` for the crate at `manifest`.
///
/// # Errors
///
/// `C0005` when the crate is not in the metadata, has no library target, or does not depend on
/// Undra.
pub fn parse_metadata(meta: &Value, manifest: &Path) -> Result<CoreInfo> {
    let packages = meta["packages"].as_array().cloned().unwrap_or_default();
    let manifest_canonical = manifest
        .canonicalize()
        .unwrap_or_else(|_| manifest.to_path_buf());
    let is_core = |p: &Value| {
        p["manifest_path"].as_str().is_some_and(|m| {
            let m = Path::new(m);
            m == manifest || m.canonicalize().is_ok_and(|c| c == manifest_canonical)
        })
    };
    let Some(core) = packages.iter().find(|p| is_core(p)) else {
        return Err(CliError::new(
            Code::BadCore,
            format!("{} is not a package Cargo knows", manifest.display()),
            "`undra.toml` says the core crate is here, but `cargo metadata` does not list a package with that manifest",
            "check `[core] path` in undra.toml, or create the crate with `undra init`",
        ));
    };
    let package = core["name"].as_str().unwrap_or_default().to_owned();
    let version = core["version"].as_str().unwrap_or("0.0.0").to_owned();
    let lib_name = core["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| {
            t["kind"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| matches!(k.as_str(), Some("lib" | "rlib" | "cdylib" | "staticlib" | "dylib")))
        })
        .and_then(|t| t["name"].as_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            CliError::new(
                Code::BadCore,
                format!("the core crate `{package}` has no library target"),
                "Undra builds the core as a library (the app shells link it), so `src/lib.rs` must exist",
                "add a `src/lib.rs` (the `#[undra::api]` items go there) and remove `[lib]` overrides that disable it",
            )
        })?;

    let by_id: BTreeMap<&str, &Value> = packages
        .iter()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let undra_runtime = packages.iter().find(|p| p["name"] == "undra-runtime").ok_or_else(|| {
        CliError::new(
            Code::BadCore,
            format!("the core crate `{package}` does not depend on Undra"),
            "without the `undra` crate there are no `#[undra::api]` items to describe, and the schema would be empty",
            "add `undra = \"0.1\"` to the core's [dependencies] (or a `path` to your Undra checkout)",
        )
    })?;
    let undra = source_of(undra_runtime, &package)?;

    let core_id = core["id"].as_str().unwrap_or_default();
    let links_ports = reaches(meta, core_id, &by_id, "undra-ports");
    let dir = Path::new(core["manifest_path"].as_str().unwrap_or_default())
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let local_dirs = local_dirs(meta, core_id, &by_id, &dir, &undra);
    let workspace = workspace_of(meta, core_id, &dir);

    Ok(CoreInfo {
        package,
        version,
        lib_name,
        dir,
        undra,
        links_ports,
        local_dirs,
        workspace,
    })
}

/// The workspace `core_id` (whose directory is `core_dir`) is a member of, when its root is not
/// the core's own directory: a core that is a workspace of its own has nothing to share.
fn workspace_of(meta: &Value, core_id: &str, core_dir: &Path) -> Option<Workspace> {
    let member = meta["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|id| id.as_str() == Some(core_id));
    let root = PathBuf::from(meta["workspace_root"].as_str()?);
    let target_dir = PathBuf::from(meta["target_directory"].as_str()?);
    (member && root != core_dir).then_some(Workspace { root, target_dir })
}

/// The directories of the path packages reachable from the core through normal dependencies,
/// except the ones inside an Undra checkout.
fn local_dirs(
    meta: &Value,
    root: &str,
    by_id: &BTreeMap<&str, &Value>,
    core_dir: &Path,
    undra: &UndraSource,
) -> Vec<PathBuf> {
    let nodes: BTreeMap<&str, &Value> = meta["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([root]);
    let mut dirs = vec![core_dir.to_path_buf()];
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(package) = by_id.get(id) {
            let is_path = package["source"].is_null();
            let dir = Path::new(package["manifest_path"].as_str().unwrap_or_default())
                .parent()
                .map(Path::to_path_buf);
            if let (true, Some(dir)) = (is_path, dir) {
                let in_undra = matches!(undra, UndraSource::Path { repo } if dir.starts_with(repo));
                if !in_undra && !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in node["deps"].as_array().into_iter().flatten() {
            let normal = dep["dep_kinds"]
                .as_array()
                .is_none_or(|kinds| kinds.is_empty() || kinds.iter().any(|k| k["kind"].is_null()));
            if let (true, Some(pkg)) = (normal, dep["pkg"].as_str()) {
                queue.push_back(pkg);
            }
        }
    }
    dirs
}

/// Where `undra-runtime` (and so, in lockstep, every Undra crate) comes from.
fn source_of(runtime: &Value, package: &str) -> Result<UndraSource> {
    match runtime["source"].as_str() {
        None => {
            let manifest = Path::new(runtime["manifest_path"].as_str().unwrap_or_default());
            let repo = manifest
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent);
            match repo {
                Some(repo) if repo.join("crates/undra-ffi/Cargo.toml").is_file() => {
                    Ok(UndraSource::Path {
                        repo: repo.to_path_buf(),
                    })
                }
                _ => Err(CliError::new(
                    Code::BadCore,
                    format!(
                        "`{package}` uses undra-runtime from {}, which is not an Undra checkout",
                        manifest.display()
                    ),
                    "the library Undra ships (`undra-ffi`) must come from the same place as `undra-runtime`, and it is expected next to it in `crates/`",
                    "point the core's `undra` dependency at a checkout of the Undra repository (`path = \"<repo>/crates/undra\"`) or at a released version",
                )),
            }
        }
        Some(source) if source.starts_with("registry+") || source.starts_with("sparse+") => {
            Ok(UndraSource::Registry {
                version: runtime["version"].as_str().unwrap_or("0.1.0").to_owned(),
            })
        }
        Some(source) if source.starts_with("git+") => {
            let rest = &source["git+".len()..];
            let (url_and_query, sha) = rest
                .split_once('#')
                .map_or((rest, None), |(a, b)| (a, Some(b)));
            let url = url_and_query
                .split('?')
                .next()
                .unwrap_or(url_and_query)
                .to_owned();
            Ok(UndraSource::Git {
                url,
                rev: sha.map(ToOwned::to_owned),
            })
        }
        Some(other) => Err(CliError::new(
            Code::BadCore,
            format!("undra-runtime comes from `{other}`, a source the CLI does not know"),
            "it has to build the matching `undra-ffi` from the same source",
            "use a path, a git or a registry dependency on `undra`",
        )),
    }
}

/// Whether the normal-dependency graph of package `root` reaches a package called `name`.
fn reaches(meta: &Value, root: &str, by_id: &BTreeMap<&str, &Value>, name: &str) -> bool {
    let nodes: BTreeMap<&str, &Value> = meta["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([root]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        if by_id.get(id).is_some_and(|p| p["name"] == name) {
            return true;
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in node["deps"].as_array().into_iter().flatten() {
            let normal = dep["dep_kinds"]
                .as_array()
                .is_none_or(|kinds| kinds.is_empty() || kinds.iter().any(|k| k["kind"].is_null()));
            if let (true, Some(pkg)) = (normal, dep["pkg"].as_str()) {
                queue.push_back(pkg);
            }
        }
    }
    false
}

/// Runs `cargo`.
pub struct Cargo<'a> {
    /// The toolchain environment.
    pub toolchain: &'a Toolchain,
    /// The machine.
    pub sys: &'a dyn Sys,
}

/// How a library is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// `dev`: fast to build, for `undra bindgen`, `undra dev` and debug builds.
    Dev,
    /// `release`: LTO, one codegen unit (the shim's profile).
    Release,
    /// `release-wasm`: `release` with `opt-level = "z"` and `panic = "abort"` (SPEC 7).
    ReleaseWasm,
}

impl Profile {
    /// The profile's name in Cargo and in `target/` (`debug` for `dev`).
    #[must_use]
    pub fn dir_name(self) -> &'static str {
        match self {
            Profile::Dev => "debug",
            Profile::Release => "release",
            Profile::ReleaseWasm => "release-wasm",
        }
    }

    fn args(self) -> Vec<&'static str> {
        match self {
            Profile::Dev => vec![],
            Profile::Release => vec!["--release"],
            Profile::ReleaseWasm => vec!["--profile", "release-wasm"],
        }
    }
}

/// One library build.
#[derive(Clone, Debug)]
pub struct Build {
    /// The shim's `Cargo.toml`.
    pub manifest: PathBuf,
    /// Cargo's target directory.
    pub target_dir: PathBuf,
    /// The Rust target triple; `None` for the host.
    pub triple: Option<String>,
    /// The profile.
    pub profile: Profile,
    /// `cdylib` or `staticlib`.
    pub crate_type: &'static str,
    /// Cargo features of the shim (`jni`).
    pub features: Vec<String>,
    /// Extra environment for the build (`IPHONEOS_DEPLOYMENT_TARGET`).
    pub env: Vec<(String, String)>,
    /// The library name to look for among Cargo's artifacts.
    pub lib_name: String,
    /// Arguments for rustc itself, after `--` (`-Clink-arg=...`): they apply to the shim only,
    /// never to the dependencies, which stay shared with the workspace's own builds.
    pub rustc_args: Vec<String>,
}

impl Cargo<'_> {
    fn program(&self, purpose: &str) -> Result<PathBuf> {
        self.toolchain.which(self.sys, "cargo").ok_or_else(|| {
            CliError::missing_tool(
                "cargo",
                purpose,
                "install Rust with rustup: https://rustup.rs (then open a new terminal)",
            )
        })
    }

    /// Runs `cargo metadata` for `manifest` and interprets it with [`parse_metadata`].
    ///
    /// # Errors
    ///
    /// `C0003` when cargo is missing, `C0004` when it fails (an unresolvable dependency, a
    /// broken manifest), `C0005` when the crate is not an Undra core.
    pub fn core_info(&self, manifest: &Path) -> Result<CoreInfo> {
        let cargo = self.program("reading the core crate")?;
        let mut cmd = Command::new(&cargo);
        cmd.args(["metadata", "--format-version", "1", "--manifest-path"])
            .arg(manifest)
            .stdin(Stdio::null());
        self.toolchain.apply(&mut cmd);
        let output = cmd.output().map_err(|e| CliError::io("run", &cargo, &e))?;
        if !output.status.success() {
            return Err(CliError::tool_failed(
                "cargo metadata",
                "reading the core crate",
                &status_text(&output.status),
            )
            .with_detail(String::from_utf8_lossy(&output.stderr).trim().to_owned()));
        }
        let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
            CliError::new(
                Code::ToolFailed,
                format!("`cargo metadata` printed something that is not JSON: {e}"),
                "the CLI reads Cargo's machine-readable output and this Cargo produced unexpected output",
                "update Rust (`rustup update`) and try again",
            )
        })?;
        parse_metadata(&json, manifest)
    }

    /// Builds a library and returns the files Cargo produced for it.
    ///
    /// Cargo's own progress is shown on stderr as it runs.
    ///
    /// # Errors
    ///
    /// `C0003` when cargo is missing, `C0011` when the Rust target is not installed, `C0004` when
    /// the build fails.
    pub fn build_library(&self, build: &Build) -> Result<Vec<PathBuf>> {
        let cargo = self.program("building the core")?;
        if let Some(triple) = &build.triple {
            self.require_target(triple)?;
        }
        let mut cmd = Command::new(&cargo);
        cmd.arg("rustc")
            .arg("--manifest-path")
            .arg(&build.manifest)
            .arg("--target-dir")
            .arg(&build.target_dir)
            .args(["--lib", "--crate-type", build.crate_type])
            .args(build.profile.args());
        if let Some(triple) = &build.triple {
            cmd.args(["--target", triple]);
        }
        if !build.features.is_empty() {
            cmd.arg("--features").arg(build.features.join(","));
        }
        cmd.args(["--message-format", "json-render-diagnostics"]);
        if !build.rustc_args.is_empty() {
            cmd.arg("--").args(&build.rustc_args);
        }
        cmd.envs(build.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        self.toolchain.apply(&mut cmd);
        let output = cmd.output().map_err(|e| CliError::io("run", &cargo, &e))?;
        if !output.status.success() {
            return Err(CliError::tool_failed(
                "cargo",
                &format!("building the core ({})", describe_build(build)),
                &status_text(&output.status),
            ));
        }
        let files = artifacts(
            &String::from_utf8_lossy(&output.stdout),
            &build.lib_name,
            build.crate_type,
        );
        if files.is_empty() {
            return Err(CliError::new(
                Code::ToolFailed,
                format!(
                    "cargo finished but reported no `{}` {} artifact",
                    build.lib_name, build.crate_type
                ),
                "the CLI finds the library through Cargo's JSON messages and there was none",
                "run the command again with a clean target (`cargo clean`); if it persists this is a bug in undra-cli",
            ));
        }
        Ok(files)
    }

    /// Builds the binary `bin` of the crate at `manifest` (debug profile) and returns the
    /// executable. Cargo's progress and diagnostics go to stderr as it runs.
    ///
    /// # Errors
    ///
    /// `C0003` when cargo is missing, `C0004` when the build fails (the compiler's diagnostics were
    /// already shown).
    pub fn build_executable(
        &self,
        manifest: &Path,
        target_dir: &Path,
        bin: &str,
    ) -> Result<PathBuf> {
        let cargo = self.program("building the core")?;
        let mut cmd = Command::new(&cargo);
        cmd.args(["build", "--bin", bin, "--manifest-path"])
            .arg(manifest)
            .arg("--target-dir")
            .arg(target_dir)
            .args(["--message-format", "json-render-diagnostics"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        self.toolchain.apply(&mut cmd);
        let output = cmd.output().map_err(|e| CliError::io("run", &cargo, &e))?;
        if !output.status.success() {
            return Err(CliError::tool_failed(
                "cargo",
                "building the core",
                &status_text(&output.status),
            ));
        }
        executable(&String::from_utf8_lossy(&output.stdout), bin).ok_or_else(|| {
            CliError::new(
                Code::ToolFailed,
                format!("cargo finished but reported no executable for `{bin}`"),
                "the CLI finds the binary through Cargo's JSON messages and there was none",
                "run `cargo clean` and try again; if it persists this is a bug in undra-cli",
            )
        })
    }

    /// Checks that the Rust standard library for `triple` is installed.
    ///
    /// # Errors
    ///
    /// `C0011` naming the `rustup target add` command.
    pub fn require_target(&self, triple: &str) -> Result<()> {
        let Some(rustc) = self.toolchain.which(self.sys, "rustc") else {
            return Ok(()); // cargo will say it
        };
        let env = self.toolchain.env_pairs();
        let Some(out) = self.sys.run(&rustc, &["--print", "sysroot"], &env) else {
            return Ok(());
        };
        let sysroot = PathBuf::from(out.stdout.trim());
        if out.success
            && !sysroot.as_os_str().is_empty()
            && !self.sys.is_dir(&sysroot.join("lib/rustlib").join(triple))
        {
            return Err(CliError::new(
                Code::MissingTarget,
                format!("the Rust target `{triple}` is not installed"),
                "building for this platform needs the standard library compiled for it, and rustup has not installed it",
                format!("rustup target add {triple}"),
            ));
        }
        Ok(())
    }
}

fn describe_build(build: &Build) -> String {
    format!(
        "{}, {} profile",
        build.triple.as_deref().unwrap_or("host"),
        build.profile.dir_name()
    )
}

/// `exit status: 101` / `signal: 9` from an exit status.
#[must_use]
pub fn status_text(status: &std::process::ExitStatus) -> String {
    status.to_string()
}

/// The files of the `compiler-artifact` messages for library `lib_name` in Cargo's
/// `--message-format json` output. Only the last such message counts (the library itself, not a
/// same-named dependency build).
#[must_use]
pub fn artifacts(json_lines: &str, lib_name: &str, crate_type: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for line in json_lines.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" || message["target"]["name"] != lib_name {
            continue;
        }
        let is_type = message["target"]["crate_types"]
            .as_array()
            .is_some_and(|types| types.iter().any(|t| t == crate_type));
        if !is_type {
            continue;
        }
        found = message["filenames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str().map(PathBuf::from))
            .collect();
    }
    found
}

/// The executable of the last `compiler-artifact` message for binary `bin`.
#[must_use]
pub fn executable(json_lines: &str, bin: &str) -> Option<PathBuf> {
    let mut found = None;
    for line in json_lines.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if message["reason"] == "compiler-artifact" && message["target"]["name"] == bin {
            if let Some(path) = message["executable"].as_str() {
                found = Some(PathBuf::from(path));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn meta(runtime_source: Value, runtime_manifest: &str, with_ports_dep: bool) -> Value {
        let ports_kind = if with_ports_dep {
            Value::Null
        } else {
            json!("dev")
        };
        let mut nodes = vec![
            json!({"id": "core 0.1.0", "deps": [{"pkg": "undra 0.1.0", "dep_kinds": [{"kind": null}]}]}),
            json!({"id": "undra 0.1.0", "deps": [
                {"pkg": "undra-runtime 0.1.0", "dep_kinds": [{"kind": null}]},
                {"pkg": "undra-ports 0.1.0", "dep_kinds": [{"kind": ports_kind}]},
            ]}),
            json!({"id": "undra-runtime 0.1.0", "deps": []}),
            json!({"id": "undra-ports 0.1.0", "deps": []}),
        ];
        nodes.reverse();
        json!({
            "packages": [
                {"id": "core 0.1.0", "name": "todo-core", "version": "0.3.0", "manifest_path": "/proj/core/Cargo.toml",
                 "targets": [{"name": "todo_core", "kind": ["lib"]}]},
                {"id": "undra 0.1.0", "name": "undra", "version": "0.1.0", "manifest_path": "/k/crates/undra/Cargo.toml", "source": null, "targets": []},
                {"id": "undra-runtime 0.1.0", "name": "undra-runtime", "version": "0.1.0", "manifest_path": runtime_manifest, "source": runtime_source, "targets": []},
                {"id": "undra-ports 0.1.0", "name": "undra-ports", "version": "0.1.0", "manifest_path": "/k/crates/undra-ports/Cargo.toml", "source": null, "targets": []},
            ],
            "resolve": {"nodes": nodes},
        })
    }

    #[test]
    fn registry_undra_is_pinned_exactly() {
        let m = meta(
            json!("registry+https://github.com/rust-lang/crates.io-index"),
            "/r/undra-runtime/Cargo.toml",
            false,
        );
        let info = parse_metadata(&m, Path::new("/proj/core/Cargo.toml")).unwrap();
        assert_eq!(info.package, "todo-core");
        assert_eq!(info.lib_name, "todo_core");
        assert_eq!(info.version, "0.3.0");
        assert_eq!(info.dir, PathBuf::from("/proj/core"));
        assert_eq!(
            info.undra,
            UndraSource::Registry {
                version: "0.1.0".into()
            }
        );
        assert_eq!(
            info.undra.dependency("undra-ffi", &["jni"]),
            "{ version = \"=0.1.0\", features = [\"jni\"] }"
        );
        assert!(!info.links_ports, "a dev-only edge does not count");
    }

    /// `meta()` with the workspace fields of `cargo metadata` added.
    fn meta_in_workspace(root: &str, members: &[&str]) -> Value {
        let mut m = meta(json!("registry+x"), "/r/Cargo.toml", false);
        m["workspace_root"] = json!(root);
        m["target_directory"] = json!(format!("{root}/target"));
        m["workspace_members"] = json!(members);
        m
    }

    #[test]
    fn a_core_in_a_bigger_workspace_reports_its_target_directory() {
        let m = meta_in_workspace("/proj", &["core 0.1.0", "undra 0.1.0"]);
        let info = parse_metadata(&m, Path::new("/proj/core/Cargo.toml")).unwrap();
        assert_eq!(
            info.workspace,
            Some(Workspace {
                root: PathBuf::from("/proj"),
                target_dir: PathBuf::from("/proj/target"),
            })
        );
    }

    #[test]
    fn a_core_that_is_its_own_workspace_shares_nothing() {
        let m = meta_in_workspace("/proj/core", &["core 0.1.0"]);
        let info = parse_metadata(&m, Path::new("/proj/core/Cargo.toml")).unwrap();
        assert_eq!(info.workspace, None);
    }

    #[test]
    fn a_path_dependency_outside_the_members_is_not_a_workspace_member() {
        // The workspace of some other crate that happens to depend on the core by path.
        let m = meta_in_workspace("/elsewhere", &["other 0.1.0"]);
        let info = parse_metadata(&m, Path::new("/proj/core/Cargo.toml")).unwrap();
        assert_eq!(info.workspace, None);
    }

    #[test]
    fn metadata_without_workspace_fields_means_no_workspace() {
        let m = meta(json!("registry+x"), "/r/Cargo.toml", false);
        let info = parse_metadata(&m, Path::new("/proj/core/Cargo.toml")).unwrap();
        assert_eq!(info.workspace, None);
    }

    #[test]
    fn ports_are_found_through_the_facade() {
        let m = meta(json!("registry+x"), "/r/Cargo.toml", true);
        assert!(
            parse_metadata(&m, Path::new("/proj/core/Cargo.toml"))
                .unwrap()
                .links_ports
        );
    }

    #[test]
    fn git_sources_keep_the_revision() {
        let m = meta(
            json!("git+https://github.com/shreypdev/undra?branch=main#abc123"),
            "/g/undra-runtime/Cargo.toml",
            false,
        );
        let info = parse_metadata(&m, Path::new("/proj/core/Cargo.toml")).unwrap();
        assert_eq!(
            info.undra,
            UndraSource::Git {
                url: "https://github.com/shreypdev/undra".into(),
                rev: Some("abc123".into())
            }
        );
        assert_eq!(
            info.undra.dependency("undra-ffi", &[]),
            "{ git = \"https://github.com/shreypdev/undra\", rev = \"abc123\" }"
        );
    }

    #[test]
    fn a_path_undra_outside_a_checkout_is_explained() {
        let m = meta(Value::Null, "/somewhere/undra-runtime/Cargo.toml", false);
        let e = parse_metadata(&m, Path::new("/proj/core/Cargo.toml")).unwrap_err();
        assert_eq!(e.code, Code::BadCore);
        assert!(e.what.contains("not an Undra checkout"), "{e}");
    }

    #[test]
    fn a_core_without_undra_is_explained() {
        let mut m = meta(json!("registry+x"), "/r/Cargo.toml", false);
        m["packages"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["name"] != "undra-runtime");
        let e = parse_metadata(&m, Path::new("/proj/core/Cargo.toml")).unwrap_err();
        assert!(e.what.contains("does not depend on Undra"), "{e}");
        assert!(e.fix.contains("undra = "), "{e}");
    }

    #[test]
    fn an_unknown_manifest_is_explained() {
        let m = meta(json!("registry+x"), "/r/Cargo.toml", false);
        let e = parse_metadata(&m, Path::new("/elsewhere/Cargo.toml")).unwrap_err();
        assert!(e.what.contains("not a package Cargo knows"), "{e}");
    }

    #[test]
    fn path_dependencies_render_with_quoting() {
        let s = UndraSource::Path {
            repo: PathBuf::from("/my \"repo\""),
        };
        assert_eq!(
            s.dependency("undra-runtime", &[]),
            "{ path = \"/my \\\"repo\\\"/crates/undra-runtime\" }"
        );
    }

    #[test]
    fn artifact_messages_are_filtered_by_name_and_type() {
        let lines = [
            r#"{"reason":"compiler-artifact","target":{"name":"serde","crate_types":["lib"]},"filenames":["/t/libserde.rlib"]}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"undra_core","crate_types":["cdylib"]},"filenames":["/t/libundra_core.dylib"]}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"undra_core","crate_types":["staticlib"]},"filenames":["/t/libundra_core.a"]}"#,
            "not json",
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .join("\n");
        assert_eq!(
            artifacts(&lines, "undra_core", "cdylib"),
            vec![PathBuf::from("/t/libundra_core.dylib")]
        );
        assert_eq!(
            artifacts(&lines, "undra_core", "staticlib"),
            vec![PathBuf::from("/t/libundra_core.a")]
        );
        assert!(artifacts(&lines, "other", "cdylib").is_empty());
    }

    #[test]
    fn a_missing_rust_target_names_the_fix() {
        use crate::sys::fake::FakeSys;
        let sys = FakeSys::macos()
            .with_tool("rustc", "/home/.cargo/bin/rustc")
            .with_output(
                "rustc",
                "--print sysroot",
                "/home/.rustup/toolchains/stable\n",
            )
            .with_dir("/home/.rustup/toolchains/stable/lib/rustlib/aarch64-apple-darwin");
        let tc = Toolchain::default();
        let cargo = Cargo {
            toolchain: &tc,
            sys: &sys,
        };
        cargo.require_target("aarch64-apple-darwin").unwrap();
        let e = cargo.require_target("aarch64-apple-ios").unwrap_err();
        assert_eq!(e.code, Code::MissingTarget);
        assert_eq!(e.fix, "rustup target add aarch64-apple-ios");
    }
}
