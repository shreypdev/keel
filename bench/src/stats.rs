//! A fixed-size latency histogram for the sustained ("harsh conditions") scenarios.
//!
//! A sustained run times every operation, which is millions of samples: keeping them all and
//! sorting (what [`measure`](crate::measure) does for its few hundred batch means) would cost
//! memory and time that perturb the very thing being measured. [`Histogram`] records a sample
//! with no allocation and a handful of instructions, and answers percentiles afterwards.
//!
//! The buckets are log-linear, like an HDR histogram with five bits of precision: one bucket
//! per nanosecond below 64 ns, then 32 buckets per power of two, so a recorded value is never
//! more than 1/32 (3.1%) away from the bucket bound a percentile reports. Values up to about
//! 2.2 x 10^12 ns (36 minutes) are tracked; larger ones land in the last bucket.
//!
//! The same module holds the soak's **drift** test ([`drift`]): a straight line through a
//! per-second series, and how far it climbs.

/// Nanoseconds below this value get one bucket each.
const LINEAR: u64 = 64;
/// Bits of the value kept below the leading one: 32 sub-buckets per power of two.
const SUB_BITS: u32 = 5;
/// Sub-buckets per power of two.
const SUBS: usize = 1 << SUB_BITS;
/// Powers of two tracked: exponents 6 through 40.
const OCTAVES: usize = 35;

/// How many counters a [`Histogram`] holds.
pub const BUCKETS: usize = LINEAR as usize + OCTAVES * SUBS;

/// The largest value with a bucket of its own; bigger values are clamped into the last one.
const MAX_TRACKED: u64 = (1 << (6 + OCTAVES as u32)) - 1;

/// The bucket a value falls in.
fn bucket_of(ns: u64) -> usize {
    let ns = ns.min(MAX_TRACKED);
    if ns < LINEAR {
        return ns as usize;
    }
    let exponent = 63 - ns.leading_zeros();
    let sub = ((ns >> (exponent - SUB_BITS)) as usize) & (SUBS - 1);
    LINEAR as usize + (exponent as usize - 6) * SUBS + sub
}

/// The largest value that falls in `bucket`.
fn upper_bound(bucket: usize) -> u64 {
    if bucket < LINEAR as usize {
        return bucket as u64;
    }
    let offset = bucket - LINEAR as usize;
    let exponent = 6 + (offset / SUBS) as u32;
    let sub = (offset % SUBS) as u64;
    let width = 1_u64 << (exponent - SUB_BITS);
    ((SUBS as u64 + sub) << (exponent - SUB_BITS)) + width - 1
}

/// Latency histogram: 64 linear 1 ns buckets below 64 ns, then 32 sub-buckets per power of two
/// up to 2^41 ns, in one boxed array of [`BUCKETS`] (1,184) `u64` counters; the relative error
/// of a percentile is at most 1/32.
///
/// # Example
///
/// ```
/// use undra_bench::stats::Histogram;
///
/// let mut h = Histogram::new();
/// for ns in 1..=1000 {
///     h.record(ns);
/// }
/// assert_eq!(h.count(), 1000);
/// assert_eq!(h.max(), 1000);
/// let p50 = h.percentile(0.5);
/// assert!((500..=516).contains(&p50), "{p50}");
/// ```
#[derive(Clone, Debug)]
pub struct Histogram {
    counts: Box<[u64; BUCKETS]>,
    total: u64,
    max: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram::new()
    }
}

impl Histogram {
    /// An empty histogram.
    pub fn new() -> Histogram {
        let counts: Box<[u64; BUCKETS]> = vec![0_u64; BUCKETS]
            .into_boxed_slice()
            .try_into()
            .unwrap_or_else(|_| unreachable!("the vector has exactly BUCKETS elements"));
        Histogram {
            counts,
            total: 0,
            max: 0,
        }
    }

    /// Records one sample, in nanoseconds. Allocates nothing.
    pub fn record(&mut self, ns: u64) {
        self.counts[bucket_of(ns)] += 1;
        self.total += 1;
        self.max = self.max.max(ns);
    }

    /// How many samples have been recorded.
    pub fn count(&self) -> u64 {
        self.total
    }

    /// The largest sample recorded (exact, not bucketed); 0 when empty.
    pub fn max(&self) -> u64 {
        self.max
    }

    /// The `p`-quantile (`0.5` is the median, `0.99` the 99th percentile): the upper bound of
    /// the bucket holding it, never above [`max`](Histogram::max). 0 when empty. `p` is clamped
    /// to `0.0..=1.0`.
    pub fn percentile(&self, p: f64) -> u64 {
        if self.total == 0 {
            return 0;
        }
        let p = p.clamp(0.0, 1.0);
        let rank = ((p * self.total as f64).ceil() as u64).clamp(1, self.total);
        let mut seen = 0_u64;
        for (bucket, count) in self.counts.iter().enumerate() {
            seen += count;
            if seen >= rank {
                return upper_bound(bucket).min(self.max);
            }
        }
        self.max
    }

    /// Adds every sample of `other` to this histogram.
    pub fn merge(&mut self, other: &Histogram) {
        for (mine, theirs) in self.counts.iter_mut().zip(other.counts.iter()) {
            *mine += theirs;
        }
        self.total += other.total;
        self.max = self.max.max(other.max);
    }

    /// Forgets every sample.
    pub fn clear(&mut self) {
        self.counts.fill(0);
        self.total = 0;
        self.max = 0;
    }
}

/// The median of `values` (the mean of the two middle ones for an even count); `None` if empty.
///
/// # Example
///
/// ```
/// use undra_bench::stats::median;
///
/// assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
/// assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
/// assert_eq!(median(&[]), None);
/// ```
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    })
}

/// A straight line `y = intercept + slope * x`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trend {
    /// How much `y` changes per unit of `x`.
    pub slope: f64,
    /// The line's value at `x = 0`.
    pub intercept: f64,
}

/// The Theil-Sen line through `points` (`(x, y)`): the slope is the **median of the slopes
/// between every pair of points**, the intercept the median of what is left. It is a linear
/// regression that one bad point cannot tilt (a least-squares slope follows a single spike at
/// either end of the series), which is what a per-second tail latency needs: the soak has a
/// separate gate for the one bad second. `None` with fewer than two points or no two different
/// `x`.
///
/// # Example
///
/// ```
/// use undra_bench::stats::trend;
///
/// let t = trend(&[(0.0, 2.0), (1.0, 5.0), (2.0, 8.0), (3.0, 11.0)]).unwrap();
/// assert_eq!((t.slope, t.intercept), (3.0, 2.0));
/// ```
pub fn trend(points: &[(f64, f64)]) -> Option<Trend> {
    let mut slopes = Vec::with_capacity(points.len() * points.len().saturating_sub(1) / 2);
    for (i, a) in points.iter().enumerate() {
        for b in &points[i + 1..] {
            if b.0 != a.0 {
                slopes.push((b.1 - a.1) / (b.0 - a.0));
            }
        }
    }
    let slope = median(&slopes)?;
    let residuals: Vec<f64> = points.iter().map(|(x, y)| y - slope * x).collect();
    Some(Trend {
        slope,
        intercept: median(&residuals)?,
    })
}

/// How far a series climbed, in the soak's terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drift {
    /// The fitted slope, in the series' unit per unit of `x` (nanoseconds per second).
    pub slope: f64,
    /// The median of the series.
    pub median: f64,
    /// The fitted line's rise from the first point to the last, as a fraction of the median:
    /// `0.5` is a series whose trend climbs by half of its typical value over the window.
    pub rise: f64,
    /// How many points the fit used.
    pub points: usize,
}

impl Drift {
    /// Whether the series climbs by more than `limit` (a fraction of its median) over the window.
    pub fn exceeds(&self, limit: f64) -> bool {
        self.rise > limit
    }
}

/// The drift of `points` (`(second, value)`): a [`trend`] line through them, and its rise across
/// the window relative to the median value. `None` with fewer than `min_points` points (a trend
/// through a handful of noisy seconds says nothing), or when the median is not positive.
///
/// # Example
///
/// ```
/// use undra_bench::stats::drift;
///
/// // A p99 that doubles steadily over 30 seconds, 40 us to 80 us.
/// let climbing: Vec<(f64, f64)> = (0..30).map(|s| (s as f64, 40_000.0 + s as f64 * 1_380.0)).collect();
/// assert!(drift(&climbing, 6).unwrap().exceeds(0.5));
/// // A flat one does not.
/// let flat: Vec<(f64, f64)> = (0..30).map(|s| (s as f64, 40_000.0)).collect();
/// assert!(!drift(&flat, 6).unwrap().exceeds(0.5));
/// ```
pub fn drift(points: &[(f64, f64)], min_points: usize) -> Option<Drift> {
    if points.len() < min_points.max(2) {
        return None;
    }
    let line = trend(points)?;
    let ys: Vec<f64> = points.iter().map(|p| p.1).collect();
    let median = median(&ys)?;
    if median <= 0.0 {
        return None;
    }
    let span = points.last()?.0 - points.first()?.0;
    Some(Drift {
        slope: line.slope,
        median,
        rise: line.slope * span / median,
        points: points.len(),
    })
}

/// The median and the largest of `values`: the soak's spike gate compares them (a single bad
/// second shows up as a worst far above the median). `None` if empty.
pub fn median_and_worst(values: &[f64]) -> Option<(f64, f64)> {
    Some((
        median(values)?,
        values.iter().copied().fold(f64::MIN, f64::max),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_histogram_answers_zero() {
        let h = Histogram::new();
        assert_eq!(
            (h.count(), h.max(), h.percentile(0.5), h.percentile(1.0)),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn the_bucket_layout_is_what_the_docs_say() {
        assert_eq!(BUCKETS, 1184);
        // One bucket per nanosecond below 64.
        for ns in 0..64 {
            assert_eq!(bucket_of(ns), ns as usize);
            assert_eq!(upper_bound(ns as usize), ns);
        }
        // 64 starts the first octave; each power of two starts a new group of 32.
        assert_eq!(bucket_of(64), 64);
        assert_eq!(bucket_of(65), 64);
        assert_eq!(bucket_of(66), 65);
        assert_eq!(bucket_of(128), 64 + 32);
        assert_eq!(bucket_of(1 << 40), 64 + 34 * 32);
        assert_eq!(bucket_of(MAX_TRACKED), BUCKETS - 1);
        // Larger values clamp into the last bucket instead of indexing out of range.
        assert_eq!(bucket_of(u64::MAX), BUCKETS - 1);
    }

    #[test]
    fn every_value_is_inside_its_bucket_and_within_one_thirty_second_of_its_bound() {
        let mut values: Vec<u64> = (0..5_000).collect();
        let mut v = 5_000_u64;
        while v < MAX_TRACKED {
            values.push(v);
            values.push(v + 1);
            v = v + v / 7 + 1;
        }
        for v in values {
            let bucket = bucket_of(v);
            let upper = upper_bound(bucket);
            assert!(upper >= v, "{v} is above the bound {upper} of its bucket");
            // The previous bucket's bound is below the value: the value is in this bucket.
            if bucket > 0 {
                assert!(
                    upper_bound(bucket - 1) < v,
                    "{v} belongs to an earlier bucket"
                );
            }
            assert!(
                (upper - v) as f64 <= v as f64 / 32.0,
                "{v}: bound {upper} is more than 1/32 away"
            );
        }
    }

    #[test]
    fn percentiles_of_a_known_distribution() {
        let mut h = Histogram::new();
        for ns in 1..=10_000 {
            h.record(ns);
        }
        assert_eq!(h.count(), 10_000);
        assert_eq!(h.max(), 10_000);
        for (p, expected) in [
            (0.5, 5_000.0),
            (0.9, 9_000.0),
            (0.99, 9_900.0),
            (0.999, 9_990.0),
        ] {
            let got = h.percentile(p) as f64;
            assert!(got >= expected, "p{p}: {got} is below {expected}");
            assert!(
                got <= expected * (1.0 + 1.0 / 32.0) + 1.0,
                "p{p}: {got} is over"
            );
        }
        // The top quantile is the exact maximum, not a bucket bound.
        assert_eq!(h.percentile(1.0), 10_000);
    }

    #[test]
    fn a_single_value_and_a_heavy_tail() {
        let mut h = Histogram::new();
        h.record(166);
        assert_eq!(h.percentile(0.0), h.percentile(1.0));
        assert_eq!(h.percentile(0.5), 166, "clamped to the exact maximum");

        let mut h = Histogram::new();
        for _ in 0..990 {
            h.record(100);
        }
        for _ in 0..10 {
            h.record(1_000_000);
        }
        // 100 shares its bucket with 101: the answer is the bucket's upper bound.
        assert_eq!(h.percentile(0.5), 101);
        assert_eq!(h.percentile(0.99), 101);
        assert_eq!(h.percentile(0.991), 1_000_000);
        assert_eq!(h.max(), 1_000_000);
    }

    #[test]
    fn merge_adds_counts_and_keeps_the_larger_maximum() {
        let (mut a, mut b) = (Histogram::new(), Histogram::new());
        for ns in 1..=100 {
            a.record(ns);
        }
        for ns in 101..=300 {
            b.record(ns);
        }
        a.merge(&b);
        assert_eq!(a.count(), 300);
        assert_eq!(a.max(), 300);
        let p50 = a.percentile(0.5);
        assert!((150..=157).contains(&p50), "{p50}");
        // Merging an empty histogram changes nothing.
        let before = a.percentile(0.9);
        a.merge(&Histogram::new());
        assert_eq!((a.count(), a.percentile(0.9)), (300, before));
    }

    #[test]
    fn clear_forgets_everything() {
        let mut h = Histogram::new();
        for ns in [5, 500, 50_000] {
            h.record(ns);
        }
        h.clear();
        assert_eq!((h.count(), h.max(), h.percentile(0.99)), (0, 0, 0));
        h.record(7);
        assert_eq!((h.count(), h.percentile(0.5)), (1, 7));
    }

    #[test]
    fn huge_values_do_not_panic() {
        let mut h = Histogram::new();
        h.record(u64::MAX);
        assert_eq!(h.count(), 1);
        assert_eq!(h.max(), u64::MAX, "the maximum stays exact");
        assert_eq!(
            h.percentile(0.5),
            MAX_TRACKED,
            "a percentile saturates at the last bucket"
        );
    }

    // -----------------------------------------------------------------------------------------
    // Drift
    // -----------------------------------------------------------------------------------------

    /// A deterministic jitter in `-1.0..1.0` (an LCG: the tests must not depend on a RNG crate).
    fn jitter(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((*seed >> 33) as f64 / (1_u64 << 31) as f64) * 2.0 - 1.0
    }

    /// `n` seconds of a p99 that starts at `from` and ends at `to` along a straight line, with
    /// `noise` (a fraction of the value) of jitter on every second.
    fn p99_series(n: usize, from: f64, to: f64, noise: f64) -> Vec<(f64, f64)> {
        let mut seed = 7;
        (0..n)
            .map(|s| {
                let line = from + (to - from) * s as f64 / (n - 1) as f64;
                (s as f64, line * (1.0 + noise * jitter(&mut seed)))
            })
            .collect()
    }

    /// The firehose's p99 in nanoseconds for seconds 30 to 60 of a real 60 s soak on the reference
    /// host (2026-09-30, a quiet machine), the second half the gate looks at. Mostly in a
    /// 105-125 us regime, with dips to 57-63 us: the host's scheduler moves the firehose thread
    /// between fast and slow cores, which shows as a step in every latency.
    const REAL_SOAK_SECOND_HALF: [u64; 31] = [
        118_783, 114_687, 104_447, 86_015, 112_639, 110_591, 118_783, 114_687, 98_303, 120_831,
        114_687, 108_543, 106_495, 112_639, 116_735, 63_487, 122_879, 118_783, 94_207, 57_343,
        124_927, 118_783, 114_687, 120_831, 88_063, 60_415, 108_543, 77_823, 96_255, 94_207,
        118_783,
    ];

    #[test]
    fn the_median_and_the_worst() {
        assert_eq!(median(&[5.0]), Some(5.0));
        assert_eq!(median(&[]), None);
        assert_eq!(median_and_worst(&[1.0, 9.0, 3.0]), Some((3.0, 9.0)));
        assert_eq!(median_and_worst(&[]), None);
    }

    #[test]
    fn theil_sen_recovers_a_line_and_survives_a_spike_that_tilts_least_squares() {
        let exact = trend(&[(0.0, 1.0), (2.0, 5.0), (4.0, 9.0)]).unwrap();
        assert_eq!((exact.slope, exact.intercept), (2.0, 1.0));
        // Flat at 40 us with one 400 us second at the very end.
        let mut spiked: Vec<(f64, f64)> = (0..30).map(|s| (s as f64, 40_000.0)).collect();
        spiked[29].1 = 400_000.0;
        let line = trend(&spiked).unwrap();
        assert_eq!(line.slope, 0.0, "one bad second does not tilt the line");
        // Least squares on the same points climbs by 40% of the median over the window: the
        // reason the gate does not use it.
        let n = spiked.len() as f64;
        let (mx, my) = (
            spiked.iter().map(|p| p.0).sum::<f64>() / n,
            spiked.iter().map(|p| p.1).sum::<f64>() / n,
        );
        let ols = spiked.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>()
            / spiked.iter().map(|p| (p.0 - mx).powi(2)).sum::<f64>();
        assert!(ols * 29.0 / 40_000.0 > 0.4, "{ols}");
        // Degenerate inputs have no line.
        assert_eq!(trend(&[]), None);
        assert_eq!(trend(&[(1.0, 1.0)]), None);
        assert_eq!(trend(&[(1.0, 1.0), (1.0, 5.0)]), None);
    }

    #[test]
    fn a_steadily_climbing_p99_is_drift() {
        // 40 us to 100 us over 30 seconds, 8% jitter on every second.
        let d = drift(&p99_series(30, 40_000.0, 100_000.0, 0.08), 6).unwrap();
        assert!(d.exceeds(0.5), "{d:?}");
        assert!(d.rise > 0.8 && d.rise < 1.4, "{d:?}");
        // It is a climb the spike gate cannot see: worst second 100 us over a 70 us median.
        let ys: Vec<f64> = p99_series(30, 40_000.0, 100_000.0, 0.08)
            .iter()
            .map(|p| p.1)
            .collect();
        let (mid, worst) = median_and_worst(&ys).unwrap();
        assert!(
            worst <= 3.0 * mid,
            "the old gate (worst <= 3x median) passes it: {worst} vs {mid}"
        );
    }

    #[test]
    fn a_p99_that_doubles_steadily_is_drift_though_the_old_gate_passed_it() {
        // The review's case: a p99 that doubles over the window has a worst/median of 1.33.
        let series = p99_series(30, 40_000.0, 80_000.0, 0.05);
        let ys: Vec<f64> = series.iter().map(|p| p.1).collect();
        let (mid, worst) = median_and_worst(&ys).unwrap();
        assert!(worst / mid < 1.5, "{}", worst / mid);
        let d = drift(&series, 6).unwrap();
        assert!(d.exceeds(0.5), "{d:?}");
    }

    #[test]
    fn a_flat_noisy_p99_and_a_mild_climb_are_not_drift() {
        let flat = drift(&p99_series(30, 60_000.0, 60_000.0, 0.25), 6).unwrap();
        assert!(!flat.exceeds(0.5), "{flat:?}");
        // 20% over the window is inside what a shared machine does.
        let mild = drift(&p99_series(30, 60_000.0, 72_000.0, 0.10), 6).unwrap();
        assert!(!mild.exceeds(0.5), "{mild:?}");
        // A falling series is not drift (and the rise is negative).
        let falling = drift(&p99_series(30, 100_000.0, 40_000.0, 0.05), 6).unwrap();
        assert!(!falling.exceeds(0.5) && falling.rise < 0.0, "{falling:?}");
    }

    #[test]
    fn one_bad_second_is_the_spike_gates_business_not_drift() {
        let mut series = p99_series(30, 60_000.0, 60_000.0, 0.05);
        series[28].1 = 400_000.0;
        series[29].1 = 400_000.0;
        let d = drift(&series, 6).unwrap();
        assert!(
            !d.exceeds(0.5),
            "two bad seconds at the end do not tilt a Theil-Sen line: {d:?}"
        );
    }

    #[test]
    fn the_real_soaks_second_half_is_not_drift() {
        let points: Vec<(f64, f64)> = REAL_SOAK_SECOND_HALF
            .iter()
            .enumerate()
            .map(|(i, p)| ((30 + i) as f64, *p as f64))
            .collect();
        let d = drift(&points, 6).unwrap();
        assert!(!d.exceeds(0.5), "{d:?}");
        assert!(d.rise.abs() < 0.2, "{d:?}");
    }

    #[test]
    fn a_step_inside_the_window_reads_as_drift_and_that_is_why_the_soak_retries() {
        // The host's scheduler can move the firehose thread to a slower core for good partway
        // through a run: 40 us, then 110 us. A line through that climbs, so the gate flags it. A
        // fresh process (`--attempts`) lands somewhere else; a real climb flags every attempt.
        let step: Vec<(f64, f64)> = (0..30)
            .map(|s| (s as f64, if s < 15 { 40_000.0 } else { 110_000.0 }))
            .collect();
        assert!(drift(&step, 6).unwrap().exceeds(0.5));
    }

    #[test]
    fn a_trend_through_a_handful_of_seconds_is_not_attempted() {
        let few = p99_series(5, 40_000.0, 100_000.0, 0.0);
        assert_eq!(drift(&few, 6), None);
        assert!(drift(&p99_series(6, 40_000.0, 100_000.0, 0.0), 6).is_some());
        let zeros: Vec<(f64, f64)> = (0..10).map(|s| (s as f64, 0.0)).collect();
        assert_eq!(
            drift(&zeros, 6),
            None,
            "a median of zero has no relative rise"
        );
    }
}
