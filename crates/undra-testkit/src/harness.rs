//! [`Harness`]: a [`TestRuntime`] with every fake installed, started from a [`Seed`].

use core::time::Duration;

use undra_ports::fakes::{self, Fakes};
use undra_runtime::testing::TestRuntime;

use crate::seed::{Seed, SeedError};

/// A test runtime with the deterministic fakes bound as its ports and a manual clock.
///
/// ```
/// use std::time::Duration;
/// use undra_testkit::Harness;
///
/// let h = Harness::from_seed_json(r#"{"now_ms": 1000, "kv": {"greeting": "hi"}}"#).unwrap();
/// assert_eq!(h.fakes().kv.value("greeting"), Some(b"hi".to_vec()));
/// h.advance(Duration::from_secs(2)); // fires due timers, runs the tasks they wake
/// assert_eq!(undra_ports::Clock::now_ms(&*h.fakes().clock), 3000);
/// ```
pub struct Harness {
    runtime: TestRuntime,
    fakes: Fakes,
}

impl Harness {
    /// A runtime with fresh fakes.
    pub fn new() -> Harness {
        let runtime = TestRuntime::new();
        let fakes = fakes::install(&runtime);
        Harness { runtime, fakes }
    }

    /// A runtime whose fakes start in the state `seed` describes.
    ///
    /// # Errors
    ///
    /// [`SeedError`] if the seed names an `fs` path the fake refuses.
    pub fn from_seed(seed: &Seed) -> Result<Harness, SeedError> {
        let harness = Harness::new();
        seed.apply(&harness.fakes)?;
        Ok(harness)
    }

    /// [`from_seed`](Harness::from_seed) for the JSON text of a seed.
    ///
    /// # Errors
    ///
    /// [`SeedError`] for a document that does not read or does not apply.
    pub fn from_seed_json(text: &str) -> Result<Harness, SeedError> {
        Harness::from_seed(&Seed::from_json(text)?)
    }

    /// The test runtime.
    pub fn runtime(&self) -> &TestRuntime {
        &self.runtime
    }

    /// The fakes bound into it.
    pub fn fakes(&self) -> &Fakes {
        &self.fakes
    }

    /// Moves the manual clock forward and runs what it wakes (`Fakes::advance`): returns how many
    /// timers fired.
    pub fn advance(&self, by: Duration) -> usize {
        self.fakes.advance(&self.runtime, by)
    }
}

impl Default for Harness {
    fn default() -> Harness {
        Harness::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seeded_harness_serves_its_state_and_advances_its_clock() {
        let h = Harness::from_seed_json(
            r#"{"now_ms": 10, "kv": {"k": "v"}, "http": [{"status": 204}]}"#,
        )
        .unwrap();
        assert_eq!(h.fakes().kv.value("k"), Some(b"v".to_vec()));
        let ctx = h.runtime().ctx();
        let slept = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = slept.clone();
        let inner = ctx.clone();
        ctx.spawn(async move {
            inner.sleep(Duration::from_secs(5)).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        assert_eq!(h.advance(Duration::from_secs(4)), 0);
        assert!(!slept.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(h.advance(Duration::from_secs(1)), 1);
        assert!(slept.load(std::sync::atomic::Ordering::SeqCst));
        assert!(Harness::from_seed_json("{\"fs\": {\"..\": \"x\"}}").is_err());
    }
}
