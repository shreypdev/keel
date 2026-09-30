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
/// use keel_bench::stats::Histogram;
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
}
