//! Facts about the machine a measurement ran on, with the standard library only.
//!
//! A number without its machine is not a number: [`baseline`](crate::baseline) files and the
//! per-scenario result files record these next to every measurement, so a figure on the landing
//! page or in `bench/RESULTS.md` traces to a file that says what it ran on, and how busy that
//! machine was. Everything here is best effort and returns `None` (or `"unknown"`) rather than
//! failing: a missing fact never fails a gate.

use std::process::Command;

/// Runs `program args` and returns its trimmed standard output, if it succeeded.
fn output_of(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The CPU model: `machdep.cpu.brand_string` on macOS, the first `model name` (or `Hardware`,
/// or `Processor`) of `/proc/cpuinfo` elsewhere.
///
/// # Example
///
/// ```
/// // Never panics; a platform that cannot say returns "unknown".
/// assert!(!undra_bench::hostinfo::cpu().is_empty());
/// ```
pub fn cpu() -> String {
    let brand = cfg!(target_os = "macos")
        .then(|| output_of("sysctl", &["-n", "machdep.cpu.brand_string"]))
        .flatten();
    if let Some(brand) = brand {
        return brand;
    }
    if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
        for key in ["model name", "Hardware", "Processor"] {
            let found = info.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                (name.trim() == key).then(|| value.trim().to_owned())
            });
            if let Some(model) = found.filter(|m| !m.is_empty()) {
                return model;
            }
        }
    }
    "unknown".to_owned()
}

/// Logical cores available to this process.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

/// The operating system and its version, as `uname -sr` or `sw_vers` says.
pub fn os() -> String {
    let version = cfg!(target_os = "macos")
        .then(|| output_of("sw_vers", &["-productVersion"]))
        .flatten();
    if let Some(version) = version {
        return format!("macOS {version}");
    }
    output_of("uname", &["-sr"]).unwrap_or_else(|| std::env::consts::OS.to_owned())
}

/// The compiler that built the benchmark: `rustc --version`, or `"unknown"`.
pub fn rustc() -> String {
    output_of("rustc", &["--version"]).unwrap_or_else(|| "unknown".to_owned())
}

/// The one-minute load average, which says how busy the machine was (a shared machine's numbers
/// are only as quiet as this). `None` where it cannot be read.
///
/// # Example
///
/// ```
/// if let Some(load) = undra_bench::hostinfo::load_average() {
///     assert!(load >= 0.0);
/// }
/// ```
pub fn load_average() -> Option<f64> {
    if let Ok(text) = std::fs::read_to_string("/proc/loadavg") {
        return text.split_whitespace().next()?.parse().ok();
    }
    // macOS: `vm.loadavg: { 1.77 3.88 7.09 }`.
    let text = output_of("sysctl", &["-n", "vm.loadavg"])?;
    text.trim_matches(|c: char| c == '{' || c == '}' || c.is_whitespace())
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// The commit the working tree is at (`git rev-parse --short HEAD`, `-dirty` when the tree has
/// uncommitted changes), or `"unknown"` outside a git checkout.
pub fn git_revision() -> String {
    let Some(sha) = output_of("git", &["rev-parse", "--short=12", "HEAD"]) else {
        return "unknown".to_owned();
    };
    let dirty = output_of("git", &["status", "--porcelain", "--untracked-files=no"]).is_some();
    if dirty { format!("{sha}-dirty") } else { sha }
}

/// Today's date as `YYYY-MM-DD`: `UNDRA_BENCH_DATE` if set, else `date +%F` (local time).
pub fn date() -> String {
    std::env::var("UNDRA_BENCH_DATE")
        .ok()
        .filter(|d| !d.is_empty())
        .or_else(|| output_of("date", &["+%F"]))
        .unwrap_or_else(|| "unknown-date".to_owned())
}

/// One line describing the machine: `cpu, N logical cores, os`.
pub fn machine() -> String {
    format!("{}, {} logical cores, {}", cpu(), cores(), os())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_facts_are_never_empty() {
        assert!(!cpu().is_empty());
        assert!(cores() >= 1);
        assert!(!os().is_empty());
        assert!(!rustc().is_empty());
        assert!(!git_revision().is_empty());
        assert!(!machine().is_empty());
    }

    #[test]
    fn the_date_is_never_empty() {
        // Whatever `UNDRA_BENCH_DATE` or `date` says, it is never empty.
        assert!(!date().is_empty());
    }

    #[test]
    fn a_load_average_is_a_non_negative_number_where_it_exists() {
        if let Some(load) = load_average() {
            assert!((0.0..100_000.0).contains(&load), "{load}");
        }
    }
}
