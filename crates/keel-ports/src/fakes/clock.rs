//! [`FakeClock`]: a settable clock that is also a timer wheel.

use core::time::Duration;
use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::{Clock, Timer};

/// Nanoseconds per millisecond.
const NS_PER_MS: u64 = 1_000_000;

type TimerHook = Arc<dyn Fn(u32) + Send + Sync>;

struct State {
    /// Wall-clock time in nanoseconds since the Unix epoch.
    wall_ns: i128,
    /// The monotonic counter.
    mono_ns: u64,
    /// Armed timers: `(deadline on the monotonic counter, arming order)` to timer id.
    timers: BTreeMap<(u64, u64), u32>,
    /// Arming counter: timers due at the same instant fire in the order they were set.
    seq: u64,
    hook: Option<TimerHook>,
}

/// A deterministic [`Clock`] and [`Timer`]: time only moves when the test says so.
///
/// * [`Clock::now_ms`] is a wall clock you can [`set_now_ms`](FakeClock::set_now_ms);
///   [`Clock::monotonic_ns`] starts at 0. [`advance`](FakeClock::advance) moves both by the
///   same amount.
/// * [`Timer::set`] arms a timer on the monotonic counter. [`advance`](FakeClock::advance)
///   fires every timer that comes due, in deadline order (ties in arming order), with the
///   clock reading exactly the deadline while each one fires, so a callback that arms another
///   timer inside the window is served in the same call.
/// * Firing calls the hook set with [`on_timer_fired`](FakeClock::on_timer_fired). Installing
///   the fakes into a runtime ([`Fakes::install`](crate::fakes::Fakes::install)) points it at
///   `Runtime::timer_fired`, which is what a platform timer does.
///
/// Nothing here reads the system clock (CLAUDE.md R12).
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use std::time::Duration;
/// use keel_ports::{Clock, Timer};
/// use keel_ports::fakes::FakeClock;
///
/// let clock = FakeClock::new();
/// let fired = Arc::new(Mutex::new(Vec::new()));
/// let sink = fired.clone();
/// clock.on_timer_fired(move |id| sink.lock().unwrap().push(id));
///
/// clock.set(7, 500);
/// clock.set(8, 100);
/// assert_eq!(clock.advance(Duration::from_millis(200)), [8]);
/// assert_eq!(clock.monotonic_ns(), 200_000_000);
/// assert_eq!(clock.advance(Duration::from_millis(300)), [7]);
/// assert_eq!(*fired.lock().unwrap(), [8, 7]);
/// ```
pub struct FakeClock {
    state: Mutex<State>,
}

impl FakeClock {
    /// The wall-clock reading of a new clock: 2023-11-14T22:13:20Z.
    pub const DEFAULT_NOW_MS: i64 = 1_700_000_000_000;

    /// A clock at [`DEFAULT_NOW_MS`](FakeClock::DEFAULT_NOW_MS) with the monotonic counter at 0.
    pub fn new() -> FakeClock {
        FakeClock::with_now_ms(FakeClock::DEFAULT_NOW_MS)
    }

    /// A clock whose wall clock reads `now_ms` and whose monotonic counter is 0.
    pub fn with_now_ms(now_ms: i64) -> FakeClock {
        FakeClock {
            state: Mutex::new(State {
                wall_ns: i128::from(now_ms) * i128::from(NS_PER_MS),
                mono_ns: 0,
                timers: BTreeMap::new(),
                seq: 0,
                hook: None,
            }),
        }
    }

    /// Sets the wall clock. The monotonic counter and the armed timers are not affected: a
    /// wall-clock jump is not the passage of time.
    pub fn set_now_ms(&self, now_ms: i64) {
        self.state.lock().wall_ns = i128::from(now_ms) * i128::from(NS_PER_MS);
    }

    /// Calls `hook(timer_id)` whenever a timer fires, replacing any previous hook. The hook runs
    /// on the thread that called [`advance`](FakeClock::advance), with no lock held, so it may
    /// arm more timers.
    pub fn on_timer_fired(&self, hook: impl Fn(u32) + Send + Sync + 'static) {
        self.state.lock().hook = Some(Arc::new(hook));
    }

    /// Removes the hook set by [`on_timer_fired`](FakeClock::on_timer_fired).
    pub fn clear_timer_hook(&self) {
        self.state.lock().hook = None;
    }

    /// Moves time forward by `duration` and fires the timers that come due. Returns the ids of
    /// the timers that fired, in the order they fired.
    pub fn advance(&self, duration: Duration) -> Vec<u32> {
        let step = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        let target = self.state.lock().mono_ns.saturating_add(step);
        let mut fired = Vec::new();
        loop {
            let (id, hook) = {
                let mut state = self.state.lock();
                let due = state
                    .timers
                    .first_key_value()
                    .filter(|((deadline, _), _)| *deadline <= target)
                    .map(|(key, _)| *key);
                let Some(key) = due else {
                    let moved = target - state.mono_ns;
                    state.mono_ns = target;
                    state.wall_ns += i128::from(moved);
                    return fired;
                };
                let id = state.timers.remove(&key).unwrap_or_default();
                // The clock reads the deadline while the timer fires.
                let moved = key.0.saturating_sub(state.mono_ns);
                state.mono_ns = state.mono_ns.max(key.0);
                state.wall_ns += i128::from(moved);
                (id, state.hook.clone())
            };
            fired.push(id);
            if let Some(hook) = hook {
                hook(id);
            }
        }
    }

    /// How many timers are armed and have not fired yet.
    pub fn pending_timers(&self) -> usize {
        self.state.lock().timers.len()
    }

    /// The ids of the armed timers, in the order they will fire.
    pub fn pending_timer_ids(&self) -> Vec<u32> {
        self.state.lock().timers.values().copied().collect()
    }

    /// How long until the next armed timer is due, or `None` if none is armed.
    pub fn next_due_in(&self) -> Option<Duration> {
        let state = self.state.lock();
        let ((deadline, _), _) = state.timers.first_key_value()?;
        Some(Duration::from_nanos(deadline.saturating_sub(state.mono_ns)))
    }
}

impl Default for FakeClock {
    fn default() -> FakeClock {
        FakeClock::new()
    }
}

impl core::fmt::Debug for FakeClock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let state = self.state.lock();
        f.debug_struct("FakeClock")
            .field("now_ms", &clamp_ms(state.wall_ns))
            .field("monotonic_ns", &state.mono_ns)
            .field("pending_timers", &state.timers.len())
            .finish_non_exhaustive()
    }
}

fn clamp_ms(wall_ns: i128) -> i64 {
    let ms = wall_ns.div_euclid(i128::from(NS_PER_MS));
    i64::try_from(ms).unwrap_or(if ms < 0 { i64::MIN } else { i64::MAX })
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        clamp_ms(self.state.lock().wall_ns)
    }

    fn monotonic_ns(&self) -> u64 {
        self.state.lock().mono_ns
    }
}

impl Timer for FakeClock {
    fn set(&self, timer_id: u32, delay_ms: u64) {
        let mut state = self.state.lock();
        let deadline = state
            .mono_ns
            .saturating_add(delay_ms.saturating_mul(NS_PER_MS));
        state.seq += 1;
        let seq = state.seq;
        state.timers.insert((deadline, seq), timer_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installs a hook that records the id of every timer that fires.
    fn recording(clock: &FakeClock) -> Arc<Mutex<Vec<u32>>> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        clock.on_timer_fired(move |id| sink.lock().push(id));
        log
    }

    #[test]
    fn starts_at_the_default_time_and_zero_monotonic() {
        let clock = FakeClock::new();
        assert_eq!(clock.now_ms(), FakeClock::DEFAULT_NOW_MS);
        assert_eq!(clock.monotonic_ns(), 0);
        assert_eq!(FakeClock::default().now_ms(), FakeClock::DEFAULT_NOW_MS);
        assert_eq!(FakeClock::with_now_ms(-5).now_ms(), -5);
    }

    #[test]
    fn advance_moves_both_clocks_and_set_now_moves_only_the_wall_clock() {
        let clock = FakeClock::with_now_ms(1_000);
        clock.advance(Duration::from_millis(250));
        assert_eq!(clock.now_ms(), 1_250);
        assert_eq!(clock.monotonic_ns(), 250_000_000);
        clock.set_now_ms(9_000);
        assert_eq!(clock.now_ms(), 9_000);
        assert_eq!(clock.monotonic_ns(), 250_000_000);
        clock.advance(Duration::from_nanos(1_500_000));
        assert_eq!(clock.now_ms(), 9_001, "sub-millisecond time accumulates");
        assert_eq!(clock.monotonic_ns(), 251_500_000);
    }

    #[test]
    fn timers_fire_in_deadline_order_with_ties_in_arming_order() {
        let clock = FakeClock::new();
        let log = recording(&clock);
        clock.set(3, 30);
        clock.set(1, 10);
        clock.set(2, 10);
        clock.set(9, 100);
        assert_eq!(clock.pending_timers(), 4);
        assert_eq!(clock.pending_timer_ids(), [1, 2, 3, 9]);
        assert_eq!(clock.next_due_in(), Some(Duration::from_millis(10)));
        assert_eq!(clock.advance(Duration::from_millis(50)), [1, 2, 3]);
        assert_eq!(*log.lock(), [1, 2, 3]);
        assert_eq!(clock.pending_timer_ids(), [9]);
        assert_eq!(clock.next_due_in(), Some(Duration::from_millis(50)));
        assert_eq!(clock.advance(Duration::from_millis(49)), Vec::<u32>::new());
        assert_eq!(clock.advance(Duration::from_millis(1)), [9]);
        assert_eq!(clock.next_due_in(), None);
    }

    #[test]
    fn the_clock_reads_the_deadline_while_a_timer_fires() {
        let clock = Arc::new(FakeClock::with_now_ms(0));
        let readings = Arc::new(Mutex::new(Vec::new()));
        {
            let (clock2, readings) = (clock.clone(), readings.clone());
            clock.on_timer_fired(move |id| {
                readings
                    .lock()
                    .push((id, clock2.monotonic_ns(), clock2.now_ms()));
            });
        }
        clock.set(1, 40);
        clock.set(2, 90);
        clock.advance(Duration::from_millis(100));
        assert_eq!(*readings.lock(), [(1, 40_000_000, 40), (2, 90_000_000, 90)]);
        assert_eq!(clock.monotonic_ns(), 100_000_000);
        assert_eq!(clock.now_ms(), 100);
    }

    #[test]
    fn a_timer_armed_by_a_hook_fires_in_the_same_advance_if_due() {
        let clock = Arc::new(FakeClock::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        {
            let (clock2, seen) = (clock.clone(), seen.clone());
            clock.on_timer_fired(move |id| {
                seen.lock().push(id);
                if id < 3 {
                    clock2.set(id + 1, 10);
                }
            });
        }
        clock.set(1, 10);
        assert_eq!(clock.advance(Duration::from_millis(25)), [1, 2]);
        assert_eq!(clock.pending_timer_ids(), [3], "the third is due at 30 ms");
        assert_eq!(clock.advance(Duration::from_millis(5)), [3]);
        assert_eq!(*seen.lock(), [1, 2, 3]);
    }

    #[test]
    fn zero_delay_timers_fire_on_the_next_advance() {
        let clock = FakeClock::new();
        clock.set(5, 0);
        assert_eq!(clock.pending_timers(), 1);
        assert_eq!(clock.advance(Duration::ZERO), [5]);
    }

    #[test]
    fn the_hook_can_be_replaced_and_cleared() {
        let clock = FakeClock::new();
        let count = Arc::new(Mutex::new(0));
        let counter = count.clone();
        clock.on_timer_fired(move |_| *counter.lock() += 1);
        clock.set(1, 1);
        clock.advance(Duration::from_millis(1));
        clock.clear_timer_hook();
        clock.set(2, 1);
        assert_eq!(clock.advance(Duration::from_millis(1)), [2]);
        assert_eq!(*count.lock(), 1);
    }

    #[test]
    fn extreme_values_saturate_instead_of_wrapping() {
        let clock = FakeClock::new();
        clock.set(1, u64::MAX);
        assert_eq!(clock.next_due_in(), Some(Duration::from_nanos(u64::MAX)));
        clock.advance(Duration::MAX);
        assert_eq!(clock.monotonic_ns(), u64::MAX);
        assert_eq!(
            clock.pending_timers(),
            0,
            "the saturated deadline is now due"
        );
        clock.set_now_ms(i64::MAX);
        assert_eq!(clock.now_ms(), i64::MAX);
        clock.set_now_ms(i64::MIN);
        assert_eq!(clock.now_ms(), i64::MIN);
    }

    #[test]
    fn debug_shows_the_readings() {
        let text = format!("{:?}", FakeClock::with_now_ms(3));
        assert!(
            text.contains("now_ms: 3") && text.contains("pending_timers: 0"),
            "{text}"
        );
    }
}
