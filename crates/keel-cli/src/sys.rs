//! The machine the CLI runs on, behind a trait so checks can be tested without one.
//!
//! [`RealSys`] asks the operating system. Tests use `FakeSys` (in the test build) to describe a
//! machine with or without Xcode, an NDK or `wasm-opt`, and assert what `keel doctor` and the
//! toolchain detection make of it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The host operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    /// macOS.
    Macos,
    /// Linux and other Unix systems.
    Linux,
    /// Windows.
    Windows,
}

impl Os {
    /// The operating system this binary runs on.
    #[must_use]
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// What a command printed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CmdOutput {
    /// Whether it exited with status 0.
    pub success: bool,
    /// Standard output, lossily decoded.
    pub stdout: String,
    /// Standard error, lossily decoded.
    pub stderr: String,
}

impl CmdOutput {
    /// Standard output and error joined, trimmed: tools print versions to either.
    #[must_use]
    pub fn text(&self) -> String {
        format!("{}{}", self.stdout, self.stderr).trim().to_owned()
    }
}

/// Everything the CLI asks of the machine.
pub trait Sys {
    /// The host operating system.
    fn os(&self) -> Os;
    /// An environment variable, `None` when unset or empty.
    fn env(&self, key: &str) -> Option<String>;
    /// The user's home directory.
    fn home(&self) -> Option<PathBuf>;
    /// Whether `path` is a file.
    fn is_file(&self, path: &Path) -> bool;
    /// Whether `path` is a directory.
    fn is_dir(&self, path: &Path) -> bool;
    /// The names of the entries of `dir`, sorted; empty when it cannot be read.
    fn list_dir(&self, dir: &Path) -> Vec<String>;
    /// Finds `program` on `PATH`, or in `extra` directories that are not on it yet.
    fn which(&self, program: &str, extra: &[PathBuf]) -> Option<PathBuf>;
    /// Runs `program args...` with `env` added, capturing its output. `None` when it cannot be
    /// started.
    fn run(&self, program: &Path, args: &[&str], env: &[(String, String)]) -> Option<CmdOutput>;
}

/// The real machine.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealSys;

impl Sys for RealSys {
    fn os(&self) -> Os {
        Os::current()
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }

    fn home(&self) -> Option<PathBuf> {
        self.env(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn list_dir(&self, dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn which(&self, program: &str, extra: &[PathBuf]) -> Option<PathBuf> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        dirs.extend(extra.iter().cloned());
        for dir in dirs {
            for candidate in executable_names(program) {
                let full = dir.join(candidate);
                if full.is_file() && is_executable(&full) {
                    return Some(full);
                }
            }
        }
        None
    }

    fn run(&self, program: &Path, args: &[&str], env: &[(String, String)]) -> Option<CmdOutput> {
        let output = Command::new(program)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .output()
            .ok()?;
        Some(CmdOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn executable_names(program: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![format!("{program}.exe"), format!("{program}.cmd"), format!("{program}.bat"), program.to_owned()]
    } else {
        vec![program.to_owned()]
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

#[cfg(test)]
pub(crate) mod fake {
    //! A scripted machine for tests.

    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    /// A machine described by data: environment, files, tools and what running them prints.
    #[derive(Default)]
    pub(crate) struct FakeSys {
        pub(crate) os: Option<Os>,
        pub(crate) env: BTreeMap<String, String>,
        pub(crate) home: Option<PathBuf>,
        pub(crate) files: BTreeSet<PathBuf>,
        pub(crate) dirs: BTreeSet<PathBuf>,
        /// Program name (as passed to `which`) to its location.
        pub(crate) tools: BTreeMap<String, PathBuf>,
        /// `"<program file name> <args joined by space>"` to its output.
        pub(crate) outputs: BTreeMap<String, CmdOutput>,
    }

    impl FakeSys {
        pub(crate) fn macos() -> FakeSys {
            FakeSys {
                os: Some(Os::Macos),
                home: Some(PathBuf::from("/Users/dev")),
                ..FakeSys::default()
            }
        }

        pub(crate) fn linux() -> FakeSys {
            FakeSys {
                os: Some(Os::Linux),
                home: Some(PathBuf::from("/home/dev")),
                ..FakeSys::default()
            }
        }

        pub(crate) fn with_env(mut self, key: &str, value: &str) -> FakeSys {
            self.env.insert(key.to_owned(), value.to_owned());
            self
        }

        pub(crate) fn with_dir(mut self, dir: &str) -> FakeSys {
            let mut path = PathBuf::from(dir);
            loop {
                self.dirs.insert(path.clone());
                if !path.pop() {
                    break;
                }
            }
            self
        }

        pub(crate) fn with_file(mut self, file: &str) -> FakeSys {
            let path = PathBuf::from(file);
            if let Some(parent) = path.parent() {
                self = self.with_dir(&parent.to_string_lossy());
            }
            self.files.insert(path);
            self
        }

        pub(crate) fn with_tool(mut self, name: &str, at: &str) -> FakeSys {
            self.tools.insert(name.to_owned(), PathBuf::from(at));
            self.with_file(at)
        }

        pub(crate) fn with_output(mut self, tool: &str, args: &str, stdout: &str) -> FakeSys {
            self.outputs.insert(
                format!("{tool} {args}").trim().to_owned(),
                CmdOutput {
                    success: true,
                    stdout: stdout.to_owned(),
                    stderr: String::new(),
                },
            );
            self
        }

        pub(crate) fn with_failing(mut self, tool: &str, args: &str, stderr: &str) -> FakeSys {
            self.outputs.insert(
                format!("{tool} {args}").trim().to_owned(),
                CmdOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: stderr.to_owned(),
                },
            );
            self
        }
    }

    impl Sys for FakeSys {
        fn os(&self) -> Os {
            self.os.unwrap_or(Os::Linux)
        }

        fn env(&self, key: &str) -> Option<String> {
            self.env.get(key).cloned()
        }

        fn home(&self) -> Option<PathBuf> {
            self.home.clone()
        }

        fn is_file(&self, path: &Path) -> bool {
            self.files.contains(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.contains(path)
        }

        fn list_dir(&self, dir: &Path) -> Vec<String> {
            let mut names = BTreeSet::new();
            for entry in self.dirs.iter().chain(self.files.iter()) {
                if entry.parent() == Some(dir) {
                    if let Some(name) = entry.file_name() {
                        names.insert(name.to_string_lossy().into_owned());
                    }
                }
            }
            names.into_iter().collect()
        }

        fn which(&self, program: &str, extra: &[PathBuf]) -> Option<PathBuf> {
            if let Some(found) = self.tools.get(program) {
                return Some(found.clone());
            }
            extra.iter().map(|d| d.join(program)).find(|p| self.files.contains(p))
        }

        fn run(&self, program: &Path, args: &[&str], _env: &[(String, String)]) -> Option<CmdOutput> {
            let name = program.file_name()?.to_string_lossy().into_owned();
            let key = format!("{name} {}", args.join(" ")).trim().to_owned();
            self.outputs.get(&key).cloned()
        }
    }
}
