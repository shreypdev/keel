//! [`Backoff`]: the delay before reconnect attempt `n`, for cores that reconnect a `WebSocket`
//! or an `Sse` stream themselves (ADR-047 §6). Deterministic: the jitter comes from the `Rng`
//! port, so a test with `SeededRng` and `FakeClock` sees the same delays every run.

use core::time::Duration;

use crate::Rng;

/// Exponential backoff with jitter, the formula of the remote transport (SPEC 11.0, ADR-051):
/// attempt `n` (from 1) waits `min(max, initial * 2^(n-1))` less a random share of up to
/// `jitter` of that.
///
/// ```
/// use std::time::Duration;
/// use undra_ports::Backoff;
/// use undra_ports::fakes::SeededRng;
///
/// let backoff = Backoff { initial: Duration::from_millis(500), max: Duration::from_secs(30), jitter: 0.0 };
/// let rng = SeededRng::default();
/// assert_eq!(backoff.delay(1, &rng), Duration::from_millis(500));
/// assert_eq!(backoff.delay(3, &rng), Duration::from_secs(2));
/// assert_eq!(backoff.delay(20, &rng), Duration::from_secs(30));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backoff {
    /// The delay before the first attempt (before jitter).
    pub initial: Duration,
    /// The longest delay.
    pub max: Duration,
    /// The share of a delay that may be taken off at random, from 0.0 to 1.0.
    pub jitter: f64,
}

impl Default for Backoff {
    /// 500 ms, doubling up to 30 s, with up to half of each delay taken off at random.
    fn default() -> Backoff {
        Backoff {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(30),
            jitter: 0.5,
        }
    }
}

impl Backoff {
    /// The delay before attempt `attempt` (1 is the first retry; 0 is treated as 1).
    pub fn delay(&self, attempt: u32, rng: &dyn Rng) -> Duration {
        let shift = attempt.saturating_sub(1).min(31);
        let base = self
            .initial
            .checked_mul(1u32 << shift)
            .unwrap_or(self.max)
            .min(self.max);
        let jitter = self.jitter.clamp(0.0, 1.0);
        if jitter == 0.0 || base.is_zero() {
            return base;
        }
        let bytes = rng.fill(4).0;
        let mut word = [0u8; 4];
        for (slot, byte) in word.iter_mut().zip(bytes) {
            *slot = byte;
        }
        let unit = f64::from(u32::from_le_bytes(word)) / f64::from(u32::MAX);
        base.mul_f64(1.0 - jitter * unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::SeededRng;

    #[test]
    fn doubles_caps_and_jitters_within_bounds() {
        let backoff = Backoff::default();
        let rng = SeededRng::new(9);
        for attempt in 0..40 {
            let base = Backoff {
                jitter: 0.0,
                ..backoff
            }
            .delay(attempt, &rng);
            let jittered = backoff.delay(attempt, &rng);
            assert!(
                jittered <= base && jittered >= base / 2,
                "{attempt}: {jittered:?} vs {base:?}"
            );
        }
        assert_eq!(
            Backoff {
                jitter: 0.0,
                ..backoff
            }
            .delay(0, &rng),
            Duration::from_millis(500)
        );
        assert_eq!(
            Backoff {
                jitter: 0.0,
                ..backoff
            }
            .delay(u32::MAX, &rng),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn same_seed_same_delays() {
        let a: Vec<_> = (1..6)
            .map(|n| Backoff::default().delay(n, &SeededRng::new(3)))
            .collect();
        let b: Vec<_> = (1..6)
            .map(|n| Backoff::default().delay(n, &SeededRng::new(3)))
            .collect();
        assert_eq!(a, b);
    }
}
