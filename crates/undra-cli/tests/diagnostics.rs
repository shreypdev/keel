//! The command-line diagnostics a user meets first, as the binary prints them: locked in
//! `tests/golden/diagnostics/<code>.txt`, which the error-codes page of the site shows as the real
//! messages of those codes. Paths are replaced by `<dir>`; regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p undra-cli --test diagnostics` and review the diff.
//!
//! Only the diagnostics that need nothing but the binary are here (no toolchain, no build): the
//! others are raised by tools that run for a long time or exist on one platform, and are covered by
//! unit tests in `src/`.

mod common;

use std::path::Path;

use common::{TempDir, init_project, repo_root, run_err, undra};

/// What a user reads: stderr with the scratch directory replaced, so the golden is the same on
/// every machine.
fn shown(stderr: &str, dir: &Path) -> String {
    let dir = dir.to_string_lossy();
    // Progress lines (`==> Creating ..`) come before the diagnostic.
    let from = stderr.find("error[undra::").unwrap_or(0);
    let repo = repo_root();
    stderr[from..]
        .trim_end()
        .replace(dir.as_ref(), "<dir>")
        .replace(repo.to_string_lossy().as_ref(), "<repo>")
        + "\n"
}

fn check(code: &str, stderr: &str, dir: &Path) {
    check_text(code, shown(stderr, dir));
}

/// Like [`check`], without what follows the diagnostic (the output of the tool that failed).
fn check_diagnostic_only(code: &str, stderr: &str, dir: &Path) {
    let text = shown(stderr, dir);
    let end = text.find("\n\n").map_or(text.len(), |at| at + 1);
    check_text(code, text[..end].to_owned());
}

fn check_text(code: &str, text: String) {
    check_golden(code, code, text);
}

/// Like [`check_text`], for a second message of `code` kept in `tests/golden/diagnostics/<name>.txt`
/// (the error-codes page shows every golden of a code).
fn check_golden(code: &str, name: &str, text: String) {
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[0].starts_with(&format!("error[undra::{code}]: ")),
        "{text}"
    );
    assert!(lines.iter().any(|l| l.starts_with("  = note: ")), "{text}");
    assert!(lines.iter().any(|l| l.starts_with("  = help: ")), "{text}");
    assert!(
        lines.iter().any(|l| *l
            == format!("  = docs: https://shreypdev.github.io/undra/docs/errors.html#{code}")),
        "{text}"
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/diagnostics")
        .join(format!("{name}.txt"));
    if std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let golden = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(
        golden, text,
        "{code}: the message changed; run UPDATE_GOLDEN=1"
    );
}

#[test]
fn c0001_outside_a_project() {
    let dir = TempDir::new("c0001");
    let (_, stderr) = run_err(undra().arg("build").current_dir(dir.path()));
    check("C0001", &stderr, dir.path());
}

#[test]
fn c0002_a_config_the_cli_cannot_use() {
    let dir = TempDir::new("c0002");
    std::fs::write(dir.path().join("undra.toml"), "[project]\nname = 1\n").unwrap();
    let (_, stderr) = run_err(undra().arg("build").current_dir(dir.path()));
    check("C0002", &stderr, dir.path());
}

#[test]
fn c0008_a_directory_that_is_not_empty() {
    let dir = TempDir::new("c0008");
    std::fs::create_dir_all(dir.path().join("taken")).unwrap();
    std::fs::write(dir.path().join("taken/file.txt"), "mine").unwrap();
    let (_, stderr) = run_err(
        undra()
            .args(["init", "taken", "--platforms", "web", "--dir"])
            .arg(dir.path()),
    );
    check("C0008", &stderr, dir.path());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("taken/file.txt")).unwrap(),
        "mine",
        "nothing is written into it"
    );
}

#[test]
fn c0009_an_argument_the_command_cannot_use() {
    let dir = TempDir::new("c0009");
    let (_, stderr) = run_err(
        undra()
            .args(["init", "my app", "--platforms", "web", "--dir"])
            .arg(dir.path()),
    );
    check("C0009", &stderr, dir.path());
}

#[test]
fn c0010_a_path_the_operating_system_refuses() {
    let dir = TempDir::new("c0010");
    // The "parent directory" is a file, so the project directory cannot be created in it.
    let file = dir.path().join("afile");
    std::fs::write(&file, "not a directory").unwrap();
    let (_, stderr) = run_err(
        undra()
            .args(["init", "demo", "--platforms", "web", "--dir"])
            .arg(&file),
    );
    check("C0010", &stderr, dir.path());
}

#[test]
fn c0003_a_tool_that_is_not_installed() {
    let project = init_project("c0003", "web");
    // A PATH with nothing on it: `cargo` cannot be found.
    let empty = TempDir::new("c0003-path");
    let (_, stderr) = run_err(
        project
            .undra()
            .args(["build", "--platform", "web"])
            // The CLI also looks in `$CARGO_HOME/bin` (or `~/.cargo/bin`), and `cargo test` exports
            // `CARGO`: with all three out of the way, `cargo` is nowhere.
            .env_remove("CARGO")
            .env_remove("CARGO_HOME")
            .env("HOME", empty.path())
            .env("PATH", empty.path()),
    );
    check("C0003", &stderr, project.dir.path());
}

#[test]
fn c0005_a_core_that_is_not_an_undra_core() {
    let project = init_project("c0005", "web");
    // The core no longer depends on Undra, so there is nothing to describe.
    let manifest = project.root.join("core/Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let at = text
        .find("[dependencies]")
        .expect("the core has dependencies");
    std::fs::write(&manifest, format!("{}[dependencies]\n", &text[..at])).unwrap();
    let (_, stderr) = run_err(project.undra().arg("bindgen"));
    check("C0005", &stderr, project.dir.path());
}

#[test]
fn c0007_a_schema_that_cannot_become_bindings() {
    let dir = TempDir::new("c0007");
    // The schema fixture with its first record declared twice: E0050 follows the diagnostic.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stores.schema.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture).unwrap()).unwrap();
    let first = value["records"][0].clone();
    value["records"].as_array_mut().unwrap().push(first);
    let bad = dir.path().join("dup.json");
    std::fs::write(&bad, serde_json::to_string(&value).unwrap()).unwrap();
    let (_, stderr) = run_err(
        undra()
            .args(["bindgen", "--schema"])
            .arg(&bad)
            .arg("--out")
            .arg(dir.path()),
    );
    check("C0007", &stderr, dir.path());
}

#[test]
fn c0014_a_project_on_a_newer_undra_than_this_one() {
    // `undra upgrade` moves a project forward only: a core pinned past this CLI is refused.
    let dir = TempDir::new("c0014-ahead");
    let root = dir.path().join("ahead");
    std::fs::create_dir_all(root.join("core/src")).unwrap();
    std::fs::write(
        root.join("undra.toml"),
        "[project]\nname = \"ahead\"\nid = \"com.example.ahead\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("core/Cargo.toml"),
        "[package]\nname = \"ahead-core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nundra = { git = \"https://github.com/shreypdev/undra\", tag = \"v999.0.0\" }\n",
    )
    .unwrap();
    let (_, stderr) = run_err(undra().arg("-C").arg(&root).arg("upgrade"));
    let text = shown(&stderr, dir.path()).replace(env!("CARGO_PKG_VERSION"), "<version>");
    check_golden("C0014", "C0014-upgrade", text);
}

#[test]
fn c0014_a_core_and_a_project_that_disagree_about_undra() {
    let project = init_project("c0014", "web");
    // undra.toml says Undra is somewhere else than the core's `undra` dependency.
    let config = project.root.join("undra.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    let at = text.find("[undra]").expect("an [undra] table");
    let (head, tail) = text.split_at(at);
    let tail = tail
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("path = ") {
                "path = \"/somewhere/else\"".to_owned()
            } else {
                l.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&config, format!("{head}{tail}\n")).unwrap();
    let (_, stderr) = run_err(project.undra().arg("bindgen"));
    check("C0014", &stderr, project.dir.path());
}

#[test]
fn c0004_a_build_tool_that_failed() {
    let project = init_project("c0004", "web");
    // `cargo metadata` cannot read a manifest that is not TOML; what it says follows the diagnostic.
    let manifest = project.root.join("core/Cargo.toml");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str("this is = not [toml\n");
    std::fs::write(&manifest, text).unwrap();
    let (_, stderr) = run_err(project.undra().arg("bindgen"));
    check_diagnostic_only("C0004", &stderr, project.dir.path());
}

/// The first `rustc` on the PATH.
#[cfg(unix)]
fn real_rustc() -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("rustc"))
        .find(|candidate| candidate.is_file())
}

#[cfg(unix)]
#[test]
fn c0011_a_rust_target_that_is_not_installed() {
    use std::os::unix::fs::PermissionsExt;

    let Some(real) = real_rustc() else {
        return; // no Rust toolchain on the PATH to stand in for
    };
    let project = init_project("c0011", "web");
    // A `rustc` that reports a sysroot with no targets, and is the real one for everything else
    // (cargo needs it).
    let tools = TempDir::new("c0011-bin");
    let sysroot = tools.path().join("sysroot");
    std::fs::create_dir_all(sysroot.join("lib/rustlib")).unwrap();
    let stub = tools.path().join("rustc");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--print\" ] && [ \"$2\" = \"sysroot\" ]; then echo '{}'; else exec '{}' \"$@\"; fi\n",
            sysroot.display(),
            real.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(tools.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let (_, stderr) = run_err(
        project
            .undra()
            .args(["build", "--platform", "web"])
            .env("PATH", path),
    );
    check("C0011", &stderr, project.dir.path());
}
