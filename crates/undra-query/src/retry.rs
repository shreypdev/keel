//! Retry with exponential backoff and jitter (SPEC 9), and the small helpers that keep the
//! query layer deterministic: time comes from the `Clock` port and randomness from `Rng`.

use core::time::Duration;
use std::panic::{AssertUnwindSafe, catch_unwind};

use undra_ports::CtxPorts;
use undra_runtime::Ctx;
use undra_runtime::log::WARN;
use undra_wire::Uuid;

use crate::defs::BoxFuture;

/// The first retry waits this long, before jitter.
pub const BACKOFF_BASE_MS: u64 = 1_000;
/// No retry waits longer than this, before jitter.
pub const BACKOFF_MAX_MS: u64 = 30_000;
/// The jitter is up to this many percent either way.
pub const JITTER_PERCENT: u64 = 20;

/// The delay before retry number `attempt` (0 for the first retry): `min(1000 x 2^attempt,
/// 30000)` ms, scaled by a factor between 0.8 and 1.2 chosen by `jitter`.
///
/// `jitter` is any random `u64`; equal values give equal delays, which is what makes retry
/// timing testable with a seeded generator.
///
/// ```
/// use undra_query::backoff_ms;
///
/// // Without jitter the delays double from one second to a cap of thirty.
/// let no_jitter = 200; // the value that maps to a factor of exactly 1.0
/// assert_eq!(
///     (0..7).map(|n| backoff_ms(n, no_jitter)).collect::<Vec<_>>(),
///     [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000]
/// );
/// // Jitter moves a delay by at most 20 percent either way.
/// assert_eq!(backoff_ms(0, 0), 800);
/// assert_eq!(backoff_ms(0, 400), 1_200);
/// ```
pub fn backoff_ms(attempt: u32, jitter: u64) -> u64 {
    let base = BACKOFF_BASE_MS
        .saturating_mul(1_u64 << attempt.min(20))
        .min(BACKOFF_MAX_MS);
    // A factor of (100 - 20)% .. (100 + 20)% in thousandths, 401 evenly spread steps.
    let span = 2 * JITTER_PERCENT * 10 + 1;
    let factor = (100 - JITTER_PERCENT) * 10 + jitter % span;
    base.saturating_mul(factor) / 1_000
}

/// Runs a port call that must not take the caller down: a panicking port (an unavailable
/// binding panics in its proxy, SPEC 8) is logged and answered with `fallback`.
pub(crate) fn guarded_port<T>(ctx: &Ctx, what: &str, fallback: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(_) => {
            ctx.runtime().log(
                WARN,
                "undra::query",
                &format!("the {what} port panicked; using a fallback"),
            );
            fallback
        }
    }
}

/// The wall clock in milliseconds.
pub(crate) fn now_ms(ctx: &Ctx, fallback: i64) -> i64 {
    guarded_port(ctx, "Clock", fallback, || ctx.clock().now_ms())
}

/// Random jitter for a backoff. The fallback lands on a factor of exactly 1.0.
pub(crate) fn jitter(ctx: &Ctx) -> u64 {
    guarded_port(ctx, "Rng", 200, || {
        let bytes = ctx.rng().fill(8).0;
        let mut word = [0_u8; 8];
        for (slot, byte) in word.iter_mut().zip(bytes) {
            *slot = byte;
        }
        u64::from_le_bytes(word)
    })
}

/// Sleeps for the backoff of retry number `attempt`.
pub(crate) async fn backoff_sleep(ctx: &Ctx, attempt: u32) {
    let delay = backoff_ms(attempt, jitter(ctx));
    ctx.sleep(Duration::from_millis(delay)).await;
}

/// Runs `attempt` until it succeeds, it fails with an error that is not `retryable`, or
/// `retries` retries have been spent, sleeping the [backoff](backoff_ms) between attempts.
pub(crate) async fn with_retries<T, E>(
    ctx: &Ctx,
    retries: u32,
    retryable: fn(&E) -> bool,
    mut attempt: impl FnMut() -> BoxFuture<Result<T, E>>,
) -> Result<T, E> {
    let mut retried = 0;
    loop {
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if retried >= retries || !retryable(&error) {
                    return Err(error);
                }
                backoff_sleep(ctx, retried).await;
                retried += 1;
            }
        }
    }
}

/// A random (version 4) UUID from the `Rng` port: the idempotency key of a mutation.
pub(crate) fn new_uuid(ctx: &Ctx) -> Uuid {
    let mut bytes = [0_u8; 16];
    let random = guarded_port(ctx, "Rng", Vec::new(), || ctx.rng().fill(16).0);
    for (slot, byte) in bytes.iter_mut().zip(random) {
        *slot = byte;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::{prop_assert, proptest};
    use undra_ports::{Rng, fakes};
    use undra_runtime::testing::TestRuntime;

    #[test]
    fn jitter_comes_from_the_rng_port_and_is_reproducible() {
        let t = TestRuntime::new();
        fakes::install(&t);
        let ctx = t.ctx();
        let shadow = fakes::SeededRng::default();
        for _ in 0..3 {
            let bytes: [u8; 8] = shadow.fill(8).0.try_into().unwrap();
            assert_eq!(jitter(&ctx), u64::from_le_bytes(bytes));
        }
    }

    #[test]
    fn uuids_are_version_4_variant_1_and_come_from_the_rng_port() {
        let t = TestRuntime::new();
        fakes::install(&t);
        let ctx = t.ctx();
        let a = new_uuid(&ctx);
        let b = new_uuid(&ctx);
        assert_ne!(a, b);
        for id in [a, b] {
            assert_eq!(id.0[6] >> 4, 4, "version 4");
            assert_eq!(id.0[8] >> 6, 0b10, "RFC 4122 variant");
        }
        // Same seed, same key: the sequence is reproducible.
        let t2 = TestRuntime::new();
        fakes::install(&t2);
        assert_eq!(new_uuid(&t2.ctx()), a);
    }

    #[test]
    fn missing_ports_fall_back_instead_of_panicking() {
        // No fakes and no host: the proxies panic, the helpers answer a fallback and log it.
        let t = TestRuntime::new();
        let ctx = t.ctx();
        assert_eq!(
            jitter(&ctx),
            200,
            "a jitter that gives a factor of exactly 1.0"
        );
        assert_eq!(now_ms(&ctx, 77), 77);
        let logs = t.host().take_logs();
        assert!(
            logs.iter()
                .any(|l| l.message.contains("Rng") && l.level == WARN),
            "{logs:?}"
        );
        assert!(logs.iter().any(|l| l.message.contains("Clock")));
        assert_eq!(new_uuid(&ctx).0[6] >> 4, 4);
    }

    #[test]
    fn delays_double_up_to_the_cap() {
        let unit = 200;
        let delays: Vec<u64> = (0..8).map(|n| backoff_ms(n, unit)).collect();
        assert_eq!(
            delays,
            [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000]
        );
        // Huge attempt numbers saturate instead of overflowing.
        assert_eq!(backoff_ms(u32::MAX, unit), 30_000);
    }

    #[test]
    fn jitter_reaches_both_ends_of_the_envelope() {
        assert_eq!(backoff_ms(2, 0), 3_200);
        assert_eq!(backoff_ms(2, 400), 4_800);
        assert_eq!(backoff_ms(2, 401), 3_200, "the jitter wraps around");
    }

    proptest! {
        #[test]
        fn every_delay_is_inside_the_plus_minus_20_percent_envelope(attempt in 0_u32..40, jitter: u64) {
            let base = BACKOFF_BASE_MS.saturating_mul(1_u64 << attempt.min(20)).min(BACKOFF_MAX_MS);
            let delay = backoff_ms(attempt, jitter);
            prop_assert!(delay >= base * 8 / 10, "{delay} < 80% of {base}");
            prop_assert!(delay <= base * 12 / 10, "{delay} > 120% of {base}");
        }
    }
}
