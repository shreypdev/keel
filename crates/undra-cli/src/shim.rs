//! The crates the CLI generates below `target/undra/`: the *shim* and the *dev runner*.
//!
//! An app core is an ordinary library crate (`undra` plus `#[undra::api]` items). It does not
//! depend on the C ABI, name a `cdylib`, or care which platform it runs on. What ships to the
//! platforms is built from two generated crates instead, so those decisions are made once, here,
//! and cannot drift between projects:
//!
//! * the **shim** (`target/undra/<project>/shim`) links the core and `undra-ffi` into one library named
//!   `undra_core` (the name the Kotlin runtime loads), with the release profiles of SPEC 7. It is
//!   built as a `cdylib` (host, Android, web) or a `staticlib` (iOS);
//! * the **dev runner** (`target/undra/<project>/dev-runner`) links the core, `undra-runtime` and
//!   `undra-transport` into an executable that serves the core over a WebSocket (`undra dev`).
//!
//! Both are separate workspaces that share the project's target directory, so the core and the
//! Undra crates compile once; each lives in a directory named for its project, so projects that
//! share a `CARGO_TARGET_DIR` do not overwrite each other's. Both depend on the core by path and on Undra from wherever the core
//! gets it, which keeps exactly one copy of `undra-runtime` in the build.

use std::path::{Path, PathBuf};

use crate::cargo::CoreInfo;
use crate::error::{CliError, Result};
use crate::fsutil::write_if_changed;
use crate::render::Vars;
use crate::toml_lite::quote;

const SHIM_MANIFEST: &str = include_str!("../templates/shim/Cargo.toml.tmpl");
const SHIM_LIB: &str = include_str!("../templates/shim/lib.rs");
const RUNNER_MANIFEST: &str = include_str!("../templates/runner/Cargo.toml.tmpl");
const RUNNER_MAIN: &str = include_str!("../templates/runner/main.rs");

/// A directory name unique to the project: its folder name and a hash of its full path. The target
/// directory can be shared by many projects (`CARGO_TARGET_DIR` set globally), and each needs its
/// own generated crates: they name *its* core by path.
#[must_use]
pub fn project_key(project_root: &Path) -> String {
    let name: String = project_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let hash = undra_meta::ids::fnv1a32(&project_root.to_string_lossy());
    format!("{name}-{hash:08x}")
}

/// The shim crate's library name: `undra_core_` plus the project-path hash. Per-project so a
/// shared target directory never sees two crates fight over one `libundra_core.*` artifact
/// (undra-ffi's test fixture builds an `undra_core` of its own; a global `CARGO_TARGET_DIR`
/// would too). The build steps copy the artifact to its canonical `libundra_core.*` name.
#[must_use]
pub fn shim_lib_name(project_root: &Path) -> String {
    let hash = undra_meta::ids::fnv1a32(&project_root.to_string_lossy());
    format!("undra_core_{hash:08x}")
}

/// The directory the shim is generated into.
#[must_use]
pub fn shim_dir(target_dir: &Path, project_root: &Path) -> PathBuf {
    target_dir
        .join("undra")
        .join(project_key(project_root))
        .join("shim")
}

/// The target directory the **host** library is built in.
///
/// Isolated from the project's own target directory on purpose (ADR-029): the host `cdylib` is
/// *loaded* with no link-time reference to it, and on macOS the linker drops the app core's
/// `inventory` registrations and undra-ffi's JNI exports (both in dependency rlibs, linked with
/// `--start-lib` lazy semantics) unless they are compiled just right. Sharing the project's target
/// let a plain `cargo build`/`cargo test` leave an incremental core rlib that `undra build` then
/// reused and stripped. A dedicated directory always builds the core fresh with the shim's
/// (non-incremental) profile, so the result does not depend on what else touched the project's
/// target. It still lives under the resolved target directory, so `CARGO_TARGET_DIR` is honoured.
/// Android (ELF keeps the symbols) and iOS (its staticlib is `-force_load`ed) do not need this.
#[must_use]
pub fn host_lib_target_dir(target_dir: &Path, project_root: &Path) -> PathBuf {
    target_dir
        .join("undra")
        .join(project_key(project_root))
        .join("host-lib")
}

/// The directory the dev runner is generated into.
#[must_use]
pub fn runner_dir(target_dir: &Path, project_root: &Path) -> PathBuf {
    target_dir
        .join("undra")
        .join(project_key(project_root))
        .join("dev-runner")
}

/// The staging directory for Android builds of this project (`cargo ndk -o`).
#[must_use]
pub fn android_stage_dir(target_dir: &Path, project_root: &Path) -> PathBuf {
    target_dir
        .join("undra")
        .join(project_key(project_root))
        .join("android-ndk")
}

/// The staging directory for iOS builds of this project.
#[must_use]
pub fn ios_stage_dir(target_dir: &Path, project_root: &Path) -> PathBuf {
    target_dir
        .join("undra")
        .join(project_key(project_root))
        .join("ios")
}

fn common_vars(core: &CoreInfo) -> Vars {
    Vars::new()
        .with("CORE_DIR", quote(&core.dir.to_string_lossy()))
        .with("CORE_PACKAGE", quote(&core.package))
        .with("CORE_PACKAGE_RAW", core.package.clone())
}

fn render(template: &str, vars: &Vars) -> Result<String> {
    vars.render(template).map_err(|name| {
        CliError::new(
            crate::error::Code::ToolFailed,
            format!("an undra-cli template uses the placeholder @@{name}@@ and nothing sets it"),
            "this is a bug in undra-cli, not in your project",
            "report it at https://github.com/shreypdev/undra/issues",
        )
    })
}

/// Seeds `dir/Cargo.lock` from the project's lock file, or else from the lock file of the Cargo
/// workspace the core belongs to, so the shim resolves the same dependency versions the core was
/// tested with (and finds them already compiled in a shared target directory). Cargo completes it
/// with what the shim adds.
fn seed_lockfile(dir: &Path, project_root: &Path, core: &CoreInfo) {
    let target = dir.join("Cargo.lock");
    if target.exists() {
        return;
    }
    let candidates = std::iter::once(project_root.join("Cargo.lock")).chain(
        core.workspace
            .iter()
            .map(|workspace| workspace.root.join("Cargo.lock")),
    );
    for candidate in candidates {
        if candidate.is_file() {
            let _ = std::fs::copy(&candidate, &target);
            return;
        }
    }
}

/// Generates the shim crate and returns its `Cargo.toml`.
///
/// # Errors
///
/// `C0010` when it cannot be written.
pub fn write_shim(
    target_dir: &Path,
    project_root: &Path,
    core: &CoreInfo,
    wasm_opt_level: &str,
) -> Result<PathBuf> {
    let dir = shim_dir(target_dir, project_root);
    let vars = common_vars(core)
        .with("UNDRA_FFI", core.undra.dependency("undra-ffi", &[]))
        .with("SHIM_LIB_NAME", shim_lib_name(project_root))
        .with("WASM_OPT_LEVEL", wasm_opt_level);
    write_if_changed(&dir.join("Cargo.toml"), &render(SHIM_MANIFEST, &vars)?)?;
    write_if_changed(&dir.join("src/lib.rs"), SHIM_LIB)?;
    seed_lockfile(&dir, project_root, core);
    Ok(dir.join("Cargo.toml"))
}

/// Generates the dev runner crate and returns its `Cargo.toml`.
///
/// # Errors
///
/// `C0010` when it cannot be written.
pub fn write_runner(target_dir: &Path, project_root: &Path, core: &CoreInfo) -> Result<PathBuf> {
    let dir = runner_dir(target_dir, project_root);
    let ports_dep = if core.links_ports {
        format!(
            "undra-ports = {}",
            core.undra.dependency("undra-ports", &[])
        )
    } else {
        String::new()
    };
    let vars = common_vars(core)
        .with("UNDRA_RUNTIME", core.undra.dependency("undra-runtime", &[]))
        .with(
            "UNDRA_TRANSPORT",
            core.undra.dependency("undra-transport", &[]),
        )
        .with("PORTS_DEP", ports_dep)
        .with(
            "STATE_LIMIT_BYTES",
            crate::reload::state_limit().to_string(),
        )
        .with("SETTLE_MS", crate::reload::SETTLE.as_millis().to_string())
        .with(
            "NOTICE_WINDOW_SECS",
            crate::reload::NOTICE_WINDOW.as_secs().to_string(),
        );
    write_if_changed(&dir.join("Cargo.toml"), &render(RUNNER_MANIFEST, &vars)?)?;
    let main = strip_block(RUNNER_MAIN, "ports", core.links_ports);
    write_if_changed(&dir.join("src/main.rs"), &render(&main, &vars)?)?;
    seed_lockfile(&dir, project_root, core);
    Ok(dir.join("Cargo.toml"))
}

/// Keeps (`keep`) or removes the lines between `// @<name>:begin` and `// @<name>:end`; the
/// marker lines themselves always go.
#[must_use]
pub fn strip_block(text: &str, name: &str, keep: bool) -> String {
    let begin = format!("// @{name}:begin");
    let end = format!("// @{name}:end");
    let mut out = String::with_capacity(text.len());
    let mut inside = 0_u32;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == begin {
            inside += 1;
            continue;
        }
        if trimmed == end {
            inside = inside.saturating_sub(1);
            continue;
        }
        if inside == 0 || keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo::UndraSource;

    fn core(links_ports: bool) -> CoreInfo {
        CoreInfo {
            package: "todo-core".into(),
            version: "0.1.0".into(),
            lib_name: "todo_core".into(),
            dir: PathBuf::from("/proj/core"),
            undra: UndraSource::Path {
                repo: PathBuf::from("/src/undra"),
            },
            links_ports,
            local_dirs: vec![PathBuf::from("/proj/core")],
            workspace: None,
        }
    }

    #[test]
    fn projects_sharing_a_target_directory_get_their_own_crates() {
        let target = Path::new("/shared/target");
        let a = shim_dir(target, Path::new("/work/app"));
        let b = shim_dir(target, Path::new("/other/app"));
        assert_ne!(a, b, "same folder name, different projects");
        let text = a.to_string_lossy();
        assert!(
            text.starts_with("/shared/target/undra/app-") && text.ends_with("/shim"),
            "{a:?}"
        );
        assert_eq!(a, shim_dir(target, Path::new("/work/app")), "stable");
        assert_ne!(runner_dir(target, Path::new("/work/app")), a);
        assert!(project_key(Path::new("/w/My App!")).starts_with("My_App_-"));
    }

    #[test]
    fn the_host_library_builds_in_its_own_target_directory() {
        // Under the resolved target (so CARGO_TARGET_DIR is honoured), but separate from the
        // project's own deps so a stray incremental rlib cannot be reused (ADR-029).
        let target = Path::new("/shared/target");
        let host = host_lib_target_dir(target, Path::new("/work/app"));
        let text = host.to_string_lossy();
        assert!(
            text.starts_with("/shared/target/undra/app-") && text.ends_with("/host-lib"),
            "{host:?}"
        );
        assert_ne!(host, shim_dir(target, Path::new("/work/app")));
        assert_ne!(
            host,
            host_lib_target_dir(target, Path::new("/other/app")),
            "per project"
        );
    }

    #[test]
    fn blocks_are_kept_or_removed_whole() {
        let text = "a\n// @x:begin\nb\n  // @x:begin\nc\n// @x:end\nd\n// @x:end\ne\n";
        assert_eq!(strip_block(text, "x", true), "a\nb\nc\nd\ne\n");
        assert_eq!(strip_block(text, "x", false), "a\ne\n");
    }

    #[test]
    fn the_shim_depends_on_the_core_and_undra_ffi_from_the_same_source() {
        let dir = crate::fsutil::unique_temp_dir("shim-gen");
        let manifest = write_shim(&dir, Path::new("/nonexistent"), &core(false), "s").unwrap();
        let text = std::fs::read_to_string(&manifest).unwrap();
        assert!(
            text.contains("undra-ffi = { path = \"/src/undra/crates/undra-ffi\" }"),
            "{text}"
        );
        assert!(
            text.contains("app-core = { path = \"/proj/core\", package = \"todo-core\" }"),
            "{text}"
        );
        let expected = format!("name = \"{}\"", shim_lib_name(Path::new("/nonexistent")));
        assert!(text.contains(&expected), "{text}");
        assert!(
            !text.contains("name = \"undra_core\""),
            "the shim lib must be per-project, not the canonical name: {text}"
        );
        assert!(text.contains("opt-level = \"s\""), "{text}");
        assert!(text.contains("panic = \"abort\""), "{text}");
        // Regenerating identical content leaves the file alone (Cargo keys rebuilds on mtime).
        let before = std::fs::metadata(&manifest).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_shim(&dir, Path::new("/nonexistent"), &core(false), "s").unwrap();
        assert_eq!(
            std::fs::metadata(&manifest).unwrap().modified().unwrap(),
            before
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_runner_binds_native_ports_only_when_the_core_links_them() {
        let dir = crate::fsutil::unique_temp_dir("runner-gen");
        write_runner(&dir, Path::new("/nonexistent"), &core(false)).unwrap();
        let without = std::fs::read_to_string(
            runner_dir(&dir, Path::new("/nonexistent")).join("src/main.rs"),
        )
        .unwrap();
        let manifest =
            std::fs::read_to_string(runner_dir(&dir, Path::new("/nonexistent")).join("Cargo.toml"))
                .unwrap();
        assert!(
            !without.contains("NativeClock") && !without.contains("bind_native_ports"),
            "{without}"
        );
        assert!(!manifest.contains("undra-ports"), "{manifest}");
        assert!(
            manifest.contains("undra-transport = { path = \"/src/undra/crates/undra-transport\" }"),
            "{manifest}"
        );

        write_runner(&dir, Path::new("/nonexistent"), &core(true)).unwrap();
        let with = std::fs::read_to_string(
            runner_dir(&dir, Path::new("/nonexistent")).join("src/main.rs"),
        )
        .unwrap();
        let manifest =
            std::fs::read_to_string(runner_dir(&dir, Path::new("/nonexistent")).join("Cargo.toml"))
                .unwrap();
        assert!(
            with.contains("bind_native_ports(&runtime)") && with.contains("NativeClock"),
            "{with}"
        );
        assert!(
            manifest.contains("undra-ports = { path = \"/src/undra/crates/undra-ports\" }"),
            "{manifest}"
        );
        assert!(with.contains("collect_schema(\"todo-core\")"), "{with}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
