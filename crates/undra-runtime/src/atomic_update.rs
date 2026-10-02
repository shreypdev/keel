//! A compare-exchange loop for `AtomicU32` and `AtomicU64`, stable across the MSRV and the newest
//! stable.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Read-modify-write on an `AtomicU32` through a compare-exchange loop: `f` sees the current value
/// and returns the next one, or `None` to leave the value alone. Returns the previous value either
/// way (`Err` when `f` declined). This is `fetch_update` spelled out, because the standard method
/// was renamed to `try_update` in a stable newer than the MSRV (CLAUDE.md: 1.85) and the old name
/// is deprecated there; `-D warnings` would fail on one toolchain or the other.
pub(crate) fn cas_update(
    cell: &AtomicU32,
    set_order: Ordering,
    fetch_order: Ordering,
    mut f: impl FnMut(u32) -> Option<u32>,
) -> Result<u32, u32> {
    let mut previous = cell.load(fetch_order);
    loop {
        let Some(next) = f(previous) else {
            return Err(previous);
        };
        match cell.compare_exchange_weak(previous, next, set_order, fetch_order) {
            Ok(_) => return Ok(previous),
            Err(observed) => previous = observed,
        }
    }
}

/// [`cas_update`] for an `AtomicU64` (the handle generation counter, ADR-040).
pub(crate) fn cas_update_u64(
    cell: &AtomicU64,
    set_order: Ordering,
    fetch_order: Ordering,
    mut f: impl FnMut(u64) -> Option<u64>,
) -> Result<u64, u64> {
    let mut previous = cell.load(fetch_order);
    loop {
        let Some(next) = f(previous) else {
            return Err(previous);
        };
        match cell.compare_exchange_weak(previous, next, set_order, fetch_order) {
            Ok(_) => return Ok(previous),
            Err(observed) => previous = observed,
        }
    }
}

#[cfg(test)]
mod cas_update_tests {
    use super::cas_update;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn applies_and_returns_the_previous_value() {
        let cell = AtomicU32::new(5);
        let r = cas_update(&cell, Ordering::AcqRel, Ordering::Acquire, |v| {
            v.checked_add(1)
        });
        assert_eq!(r, Ok(5));
        assert_eq!(cell.load(Ordering::Acquire), 6);
    }

    #[test]
    fn declining_leaves_the_value_and_reports_it() {
        let cell = AtomicU32::new(0);
        let r = cas_update(&cell, Ordering::AcqRel, Ordering::Acquire, |v| {
            v.checked_sub(1)
        });
        assert_eq!(r, Err(0));
        assert_eq!(cell.load(Ordering::Acquire), 0);
    }

    #[test]
    fn saturates_at_the_top_like_the_generation_counter() {
        let cell = AtomicU32::new(u32::MAX);
        let r = cas_update(&cell, Ordering::AcqRel, Ordering::Acquire, |v| {
            v.checked_add(1)
        });
        assert_eq!(r, Err(u32::MAX));
    }

    #[test]
    fn survives_contention() {
        let cell = std::sync::Arc::new(AtomicU32::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let cell = cell.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        cas_update(&cell, Ordering::AcqRel, Ordering::Acquire, |v| Some(v + 1))
                            .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(cell.load(Ordering::Acquire), 8000);
    }
}
