//! The machine the CLI runs on, behind a trait so checks can be tested without one.
//!
//! [`RealSys`] asks the operating system. Tests use `FakeSys` (in the test build) to describe a
//! machine with or without Xcode, an NDK or `wasm-opt`, and assert what `undra doctor` and the
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
    /// Free space, in bytes, on the volume that holds `path` (or the nearest parent that exists);
    /// `None` when it cannot be told.
    fn free_disk_bytes(&self, path: &Path) -> Option<u64>;
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
        self.env(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
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

    fn free_disk_bytes(&self, path: &Path) -> Option<u64> {
        // `df -Pk` has one POSIX layout everywhere (macOS and Linux): a header, then
        // `filesystem 1024-blocks used available capacity mounted-on`.
        let existing = path.ancestors().find(|p| p.exists())?;
        let output = Command::new("df")
            .arg("-Pk")
            .arg(existing)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_df(&String::from_utf8_lossy(&output.stdout))
    }
}

/// The available bytes in the output of `df -Pk`.
fn parse_df(output: &str) -> Option<u64> {
    let line = output.lines().nth(1)?;
    let available: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    available.checked_mul(1024)
}

fn executable_names(program: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            format!("{program}.exe"),
            format!("{program}.cmd"),
            format!("{program}.bat"),
            program.to_owned(),
        ]
    } else {
        vec![program.to_owned()]
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
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
        /// Free disk space, when the machine says.
        pub(crate) free_disk: Option<u64>,
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
    }

    impl FakeSys {
        /// A command that runs and fails, printing `text` to stderr.
        pub(crate) fn with_failing_output(mut self, tool: &str, args: &str, text: &str) -> FakeSys {
            self.outputs.insert(
                format!("{tool} {args}").trim().to_owned(),
                CmdOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: text.to_owned(),
                },
            );
            self
        }

        /// A command whose output goes to stderr (as `java -version` and `kotlinc -version` do).
        pub(crate) fn with_stderr_output(mut self, tool: &str, args: &str, text: &str) -> FakeSys {
            self.outputs.insert(
                format!("{tool} {args}").trim().to_owned(),
                CmdOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: text.to_owned(),
                },
            );
            self
        }

        /// The free disk space the machine reports.
        pub(crate) fn with_free_disk(mut self, bytes: u64) -> FakeSys {
            self.free_disk = Some(bytes);
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
            extra
                .iter()
                .map(|d| d.join(program))
                .find(|p| self.files.contains(p))
        }

        fn run(
            &self,
            program: &Path,
            args: &[&str],
            _env: &[(String, String)],
        ) -> Option<CmdOutput> {
            let name = program.file_name()?.to_string_lossy().into_owned();
            let key = format!("{name} {}", args.join(" ")).trim().to_owned();
            self.outputs.get(&key).cloned()
        }

        fn free_disk_bytes(&self, _path: &Path) -> Option<u64> {
            self.free_disk
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn df_output_is_read() {
        let macos = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s1s1 1953595632 12582912 1394147328 1% /\n";
        assert_eq!(parse_df(macos), Some(1_394_147_328 * 1024));
        let linux = "Filesystem     1024-blocks     Used Available Capacity Mounted on\noverlay           61255492 22000000  39255492      36% /\n";
        assert_eq!(parse_df(linux), Some(39_255_492 * 1024));
        assert_eq!(parse_df("Filesystem\n"), None);
        assert_eq!(parse_df("h\nx y z notanumber\n"), None);
    }

    #[test]
    fn the_real_machine_reports_some_free_space_for_the_temp_directory() {
        // Not asserting a number: only that `df` is understood on this machine.
        assert!(RealSys.free_disk_bytes(&std::env::temp_dir()).is_some());
    }
}
