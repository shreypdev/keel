//! Resident set size, with the standard library only.
//!
//! The sustained scenarios and the soak assert that memory stays flat, which needs the process's
//! resident set size (RSS) sampled while it runs. A precise heap counter would need a custom
//! global allocator, which is `unsafe` and outside `undra-ffi` (constitution R2), and a crate
//! that has to build for every host; RSS needs neither:
//!
//! * Linux and Android: `VmRSS` from `/proc/self/status`;
//! * macOS and the other Unix systems: `ps -o rss= -p <pid>` (the same measure `bench/RESULTS.md`
//!   uses for "Runtime memory at idle"), which costs a few milliseconds per call;
//! * anywhere else: `None`, and the gates that need it are reported as skipped, never as passed.
//!
//! RSS is page-granular (16 KiB pages on Apple silicon), so the growth check is "at most X
//! percent **or** at most 64 KiB" from a baseline taken after a warm-up.

use std::time::Duration;

/// Growth below this many bytes is page noise and passes whatever the percentage.
const PAGE_NOISE: u64 = 64 * 1024;

/// Reads `VmRSS` (kilobytes) out of the text of `/proc/<pid>/status` and returns bytes.
///
/// Compiled everywhere so both parsers are unit-tested on every host; only one is used by
/// [`resident_bytes`] on any given platform.
#[allow(dead_code)]
fn parse_vm_rss(status: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kilobytes: u64 = line
        .trim_start_matches("VmRSS:")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kilobytes * 1024)
}

/// Reads the output of `ps -o rss= -p <pid>` (kilobytes) and returns bytes.
#[allow(dead_code)]
fn parse_ps(output: &str) -> Option<u64> {
    let kilobytes: u64 = output.trim().parse().ok()?;
    Some(kilobytes * 1024)
}

/// The resident set size of this process in bytes: `/proc/self/status` on Linux and Android,
/// `ps -o rss= -p <pid>` on macOS and other Unix systems, `None` elsewhere or when the
/// measurement fails.
///
/// Page-granular, and it costs milliseconds on macOS (a process is spawned), so sample at most
/// about once a second and never inside a timed region.
///
/// # Example
///
/// ```
/// if let Some(bytes) = undra_bench::rss::resident_bytes() {
///     assert!(bytes > 0);
/// }
/// ```
pub fn resident_bytes() -> Option<u64> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        parse_vm_rss(&std::fs::read_to_string("/proc/self/status").ok()?)
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
    {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_ps(std::str::from_utf8(&output.stdout).ok()?)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// How resident memory moved between the first sample after the warm-up and the last sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RssGrowth {
    /// RSS at the first sample taken at or after the warm-up, in bytes.
    pub baseline_bytes: u64,
    /// RSS at the last sample, in bytes.
    pub final_bytes: u64,
    /// The largest RSS of the whole series, in bytes.
    pub peak_bytes: u64,
    /// `(final - baseline) / baseline` in percent; negative when memory shrank.
    pub growth_pct: f64,
}

impl RssGrowth {
    /// Whether the growth is acceptable: at most `pct` percent, or at most 64 KiB (one or a few
    /// pages of allocator noise, whatever the percentage of a small process).
    pub fn within(&self, pct: f64) -> bool {
        self.growth_pct <= pct || self.final_bytes.saturating_sub(self.baseline_bytes) <= PAGE_NOISE
    }
}

/// RSS samples over a run, and the growth check the soak and the stream scenario use.
///
/// # Example
///
/// ```
/// use undra_bench::rss::RssSeries;
/// use std::time::Duration;
///
/// let mut series = RssSeries::new();
/// for (second, bytes) in [(1, 9_000_000), (2, 10_000_000), (3, 10_000_000), (4, 10_050_000)] {
///     series.push(Duration::from_secs(second), bytes);
/// }
/// // Skip the first 50% of the run: the baseline is the sample at 2 s.
/// let growth = series.growth(0.5).unwrap();
/// assert_eq!(growth.baseline_bytes, 10_000_000);
/// assert!(growth.within(1.0));
/// ```
#[derive(Clone, Debug, Default)]
pub struct RssSeries {
    samples: Vec<(Duration, u64)>,
}

impl RssSeries {
    /// An empty series.
    pub fn new() -> RssSeries {
        RssSeries::default()
    }

    /// Samples the process now and records it at `at` (time since the run started). Does
    /// nothing when [`resident_bytes`] has no answer on this platform.
    pub fn sample(&mut self, at: Duration) {
        if let Some(bytes) = resident_bytes() {
            self.push(at, bytes);
        }
    }

    /// Records a sample taken elsewhere (or synthetic, in a test).
    pub fn push(&mut self, at: Duration, bytes: u64) {
        self.samples.push((at, bytes));
    }

    /// How many samples there are.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether nothing was sampled (an unsupported platform, or a run too short to sample).
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The growth from the first sample at or after `warmup` (a fraction of the run, `0.2` for
    /// the first 20%) to the last sample. `None` when fewer than two samples fall after the
    /// warm-up, because one sample says nothing about growth.
    pub fn growth(&self, warmup: f64) -> Option<RssGrowth> {
        let run = self.samples.last()?.0;
        let from = run.mul_f64(warmup.clamp(0.0, 1.0));
        let start = self.samples.iter().position(|(at, _)| *at >= from)?;
        if start + 1 >= self.samples.len() {
            return None;
        }
        let baseline = self.samples[start].1;
        let last = self.samples.last()?.1;
        let peak = self.samples.iter().map(|(_, bytes)| *bytes).max()?;
        let growth_pct = if baseline == 0 {
            0.0
        } else {
            (last as f64 - baseline as f64) / baseline as f64 * 100.0
        };
        Some(RssGrowth {
            baseline_bytes: baseline,
            final_bytes: last,
            peak_bytes: peak,
            growth_pct,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(points: &[(u64, u64)]) -> RssSeries {
        let mut s = RssSeries::new();
        for (secs, bytes) in points {
            s.push(Duration::from_secs(*secs), *bytes);
        }
        s
    }

    #[test]
    fn this_process_has_a_resident_set_on_unix() {
        if cfg!(unix) {
            let bytes = resident_bytes().expect("RSS is available on Unix");
            assert!(bytes > 100 * 1024, "{bytes}");
            assert!(bytes < 1 << 40, "{bytes}");
        }
    }

    #[test]
    fn sampling_records_a_point_when_the_platform_answers() {
        let mut s = RssSeries::new();
        s.sample(Duration::from_secs(1));
        assert_eq!(s.len(), usize::from(resident_bytes().is_some()));
        assert_eq!(s.is_empty(), s.len() == usize::from(false));
    }

    #[test]
    fn the_proc_status_parser_reads_vmrss() {
        let status = "Name:\tbench\nVmPeak:\t  99999 kB\nVmRSS:\t   11296 kB\nThreads:\t4\n";
        assert_eq!(parse_vm_rss(status), Some(11_296 * 1024));
        assert_eq!(parse_vm_rss("Name:\tbench\n"), None);
        assert_eq!(parse_vm_rss("VmRSS:\tlots kB\n"), None);
    }

    #[test]
    fn the_ps_parser_reads_kilobytes() {
        assert_eq!(parse_ps("  11344\n"), Some(11_344 * 1024));
        assert_eq!(parse_ps(""), None);
        assert_eq!(parse_ps("RSS"), None);
    }

    #[test]
    fn growth_is_measured_from_the_first_sample_after_the_warmup() {
        let s = series(&[
            (1, 5_000_000),
            (2, 10_000_000),
            (3, 10_100_000),
            (10, 10_400_000),
        ]);
        // 20% of 10 s is 2 s: the baseline is the 2 s sample; the 1 s ramp-up is ignored.
        let g = s.growth(0.2).unwrap();
        assert_eq!(g.baseline_bytes, 10_000_000);
        assert_eq!(g.final_bytes, 10_400_000);
        assert_eq!(g.peak_bytes, 10_400_000);
        assert!((g.growth_pct - 4.0).abs() < 1e-9, "{g:?}");
        assert!(!g.within(1.0));
        assert!(g.within(4.0));
        // No warm-up: the ramp counts.
        assert!(s.growth(0.0).unwrap().growth_pct > 100.0);
    }

    #[test]
    fn a_shrinking_process_passes_and_one_sample_is_not_a_trend() {
        let s = series(&[(1, 10_000_000), (2, 9_000_000), (3, 8_000_000)]);
        let g = s.growth(0.0).unwrap();
        assert!(g.growth_pct < 0.0);
        assert!(g.within(0.0));
        assert_eq!(g.peak_bytes, 10_000_000);
        // Only one sample at or after the warm-up: no answer.
        assert_eq!(series(&[(1, 1), (10, 2)]).growth(0.5), None);
        assert_eq!(RssSeries::new().growth(0.2), None);
    }

    #[test]
    fn sixty_four_kib_is_page_noise_whatever_the_percentage() {
        // 2% of a 2 MB process is 40 KB: under the floor, so it passes a 1% gate.
        let small = RssGrowth {
            baseline_bytes: 2_000_000,
            final_bytes: 2_040_000,
            peak_bytes: 2_040_000,
            growth_pct: 2.0,
        };
        assert!(small.within(1.0));
        // 2% of a 100 MB process is 2 MB: over the floor and over the gate.
        let large = RssGrowth {
            baseline_bytes: 100_000_000,
            final_bytes: 102_000_000,
            peak_bytes: 102_000_000,
            growth_pct: 2.0,
        };
        assert!(!large.within(1.0));
        assert!(large.within(2.0));
        // The floor is inclusive.
        let edge = RssGrowth {
            baseline_bytes: 1_000_000,
            final_bytes: 1_000_000 + 64 * 1024,
            peak_bytes: 1_000_000 + 64 * 1024,
            growth_pct: 6.5,
        };
        assert!(edge.within(1.0));
    }
}
