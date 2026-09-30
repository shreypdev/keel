//! Helpers shared by the integration tests: they drive the real `keel` binary.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

/// The root of the Keel repository this crate is part of.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root exists")
}

/// One target directory for every test, so the Keel crates compile once per run, not once per
/// test. It is below the repository's `target/`, so nothing is written outside it.
pub fn shared_target() -> PathBuf {
    repo_root().join("target/keel-cli-tests")
}

/// A scratch directory, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("keel-it-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir.canonicalize().unwrap())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Serializes the tests of one test binary that assert on what Cargo compiles: other tests building
/// into the shared target directory at the same time make "nothing was recompiled" unknowable.
pub fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The `keel` binary under test, with a hermetic environment.
pub fn keel() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_keel"));
    cmd.env("NO_COLOR", "1")
        .env("CARGO_TARGET_DIR", shared_target())
        .env_remove("KEEL_PATH")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .stdin(Stdio::null());
    cmd
}

/// Runs `cmd` to completion and returns its output; panics with everything it printed when it
/// fails.
pub fn run_ok(cmd: &mut Command) -> Output {
    let out = cmd.output().expect("the command starts");
    assert!(
        out.status.success(),
        "{cmd:?} failed with {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Runs `cmd` and expects it to fail; returns its stderr.
pub fn run_err(cmd: &mut Command) -> (i32, String) {
    let out = cmd.output().expect("the command starts");
    assert!(
        !out.status.success(),
        "{cmd:?} unexpectedly succeeded:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A project created with `keel init --keel-path <this repository>` in a fresh directory.
pub struct Project {
    pub dir: TempDir,
    pub root: PathBuf,
}

/// Creates a project named `name` for `platforms` ("web", "ios,android,web", ...).
pub fn init_project(name: &str, platforms: &str) -> Project {
    let dir = TempDir::new(name);
    run_ok(
        keel()
            .args(["init", name, "--platforms", platforms, "--keel-path"])
            .arg(repo_root())
            .arg("--dir")
            .arg(dir.path()),
    );
    let root = dir.path().join(name);
    // Pin the dependency versions the workspace itself was tested with, so the tests resolve the
    // same crates as everything else and need no index update.
    let _ = std::fs::copy(repo_root().join("Cargo.lock"), root.join("Cargo.lock"));
    Project { dir, root }
}

impl Project {
    /// `keel -C <root> <args>`.
    pub fn keel(&self) -> Command {
        let mut cmd = keel();
        cmd.arg("-C").arg(&self.root);
        cmd
    }
}

/// Whether the Rust standard library for `triple` is installed.
pub fn has_rust_target(triple: &str) -> bool {
    let Ok(out) = Command::new("rustc").args(["--print", "sysroot"]).output() else {
        return false;
    };
    Path::new(String::from_utf8_lossy(&out.stdout).trim())
        .join("lib/rustlib")
        .join(triple)
        .is_dir()
}

/// Whether `program --version` runs.
pub fn has_tool(program: &str, version_arg: &str) -> bool {
    Command::new(program)
        .arg(version_arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `true` when environment variable `name` is `1`: how the heavy platform tests are switched on.
pub fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}
