//! [`SeededRng`]: a deterministic random number generator.

use parking_lot::Mutex;
use undra_wire::Bytes;

use crate::Rng;

/// Substituted for a seed of 0, which is the one fixed point of xorshift.
const ZERO_SEED_REPLACEMENT: u64 = 0x9E37_79B9_7F4A_7C15;
/// The xorshift64* output multiplier.
const MULTIPLIER: u64 = 0x2545_F491_4F6C_DD1D;

/// A deterministic [`Rng`]: xorshift64\*, the same seed always yields the same bytes.
///
/// Not cryptographically secure, on purpose: a test wants repeatable "randomness".
///
/// * A seed of 0 is replaced by a fixed non-zero constant (xorshift never leaves state 0).
/// * [`Rng::fill`]`(n)` consumes `ceil(n / 8)` outputs of the generator, little-endian, and
///   drops the unused tail of the last one, so a sequence of fills is reproducible however
///   the lengths are split across calls *of the same lengths*.
/// * A fill never returns more than [`MAX_FILL`](SeededRng::MAX_FILL) bytes, the limit the
///   platform adapters enforce; a larger request gets exactly that many.
///
/// ```
/// use undra_ports::Rng;
/// use undra_ports::fakes::SeededRng;
///
/// let a = SeededRng::new(42);
/// let b = SeededRng::new(42);
/// assert_eq!(a.fill(16), b.fill(16));
/// assert_ne!(a.fill(16), SeededRng::new(43).fill(16));
/// ```
#[derive(Debug)]
pub struct SeededRng {
    state: Mutex<u64>,
}

impl SeededRng {
    /// The most bytes one [`Rng::fill`] returns: 16 MiB.
    pub const MAX_FILL: u32 = 1 << 24;

    /// The seed of [`SeededRng::default`].
    pub const DEFAULT_SEED: u64 = 0x4B45_454C_5F52_4E47; // "UNDRA_RNG"

    /// A generator started from `seed`.
    pub fn new(seed: u64) -> SeededRng {
        SeededRng {
            state: Mutex::new(initial_state(seed)),
        }
    }

    /// Restarts the sequence from `seed`.
    pub fn reseed(&self, seed: u64) {
        *self.state.lock() = initial_state(seed);
    }

    /// The next 64-bit output.
    pub fn next_u64(&self) -> u64 {
        next(&mut self.state.lock())
    }
}

fn initial_state(seed: u64) -> u64 {
    if seed == 0 {
        ZERO_SEED_REPLACEMENT
    } else {
        seed
    }
}

fn next(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(MULTIPLIER)
}

impl Default for SeededRng {
    fn default() -> SeededRng {
        SeededRng::new(SeededRng::DEFAULT_SEED)
    }
}

impl Rng for SeededRng {
    fn fill(&self, len: u32) -> Bytes {
        let len = len.min(SeededRng::MAX_FILL) as usize;
        let mut out = Vec::with_capacity(len);
        let mut state = self.state.lock();
        while out.len() < len {
            let word = next(&mut state).to_le_bytes();
            let take = (len - out.len()).min(word.len());
            out.extend_from_slice(&word[..take]);
        }
        Bytes(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference implementation, written independently of `next`.
    fn reference(seed: u64, count: usize) -> Vec<u64> {
        let mut x = if seed == 0 {
            ZERO_SEED_REPLACEMENT
        } else {
            seed
        };
        let mut out = Vec::new();
        for _ in 0..count {
            x = x ^ (x >> 12);
            x = x ^ (x << 25);
            x = x ^ (x >> 27);
            out.push(x.wrapping_mul(0x2545_F491_4F6C_DD1D));
        }
        out
    }

    #[test]
    fn matches_the_published_xorshift64_star_recurrence() {
        for seed in [1, 42, u64::MAX, 0x1234_5678_9ABC_DEF0] {
            let rng = SeededRng::new(seed);
            let got: Vec<u64> = (0..8).map(|_| rng.next_u64()).collect();
            assert_eq!(got, reference(seed, 8), "seed {seed}");
        }
    }

    #[test]
    fn known_answers_for_seed_1() {
        // Locked values: a change to the generator changes every test that uses it.
        let rng = SeededRng::new(1);
        assert_eq!(rng.next_u64(), 0x47E4_CE4B_896C_DD1D);
        assert_eq!(rng.next_u64(), 0xABCF_A6A8_E079_651D);
        assert_eq!(rng.next_u64(), 0xB9D1_0D8F_EB73_1F57);
    }

    #[test]
    fn zero_seed_is_replaced_and_never_sticks_at_zero() {
        let rng = SeededRng::new(0);
        assert_eq!(rng.next_u64(), reference(0, 1)[0]);
        assert_ne!(SeededRng::new(0).next_u64(), 0);
        assert_eq!(
            SeededRng::new(0).fill(8),
            SeededRng::new(ZERO_SEED_REPLACEMENT).fill(8)
        );
    }

    #[test]
    fn fill_is_little_endian_words_truncated_to_the_length() {
        let words = reference(7, 3);
        let rng = SeededRng::new(7);
        let bytes = rng.fill(20);
        assert_eq!(bytes.len(), 20);
        let mut expected = Vec::new();
        for w in &words {
            expected.extend_from_slice(&w.to_le_bytes());
        }
        expected.truncate(20);
        assert_eq!(bytes.0, expected);
        // Three words were consumed for 20 bytes: the next output is the fourth.
        assert_eq!(rng.next_u64(), reference(7, 4)[3]);
    }

    #[test]
    fn fill_of_zero_bytes_consumes_nothing() {
        let rng = SeededRng::new(9);
        assert!(rng.fill(0).is_empty());
        assert_eq!(rng.next_u64(), reference(9, 1)[0]);
    }

    #[test]
    fn fill_is_capped_at_the_platform_limit() {
        let rng = SeededRng::new(3);
        assert_eq!(rng.fill(u32::MAX).len(), SeededRng::MAX_FILL as usize);
        assert_eq!(rng.fill(SeededRng::MAX_FILL + 1).len(), 1 << 24);
    }

    #[test]
    fn reseed_restarts_the_sequence() {
        let rng = SeededRng::new(5);
        let first = rng.fill(32);
        rng.reseed(5);
        assert_eq!(rng.fill(32), first);
    }

    #[test]
    fn default_uses_the_documented_seed() {
        assert_eq!(
            SeededRng::default().next_u64(),
            SeededRng::new(SeededRng::DEFAULT_SEED).next_u64()
        );
    }
}
