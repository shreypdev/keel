//! The stress store: high-frequency data, produced by the core itself.
//!
//! A socket, a sensor or a simulation writes state far faster than any screen refreshes. This
//! store does that on demand, in two ways, and in both of them every update is **its own
//! transaction** (no [`Ctx::txn`]), so the platforms receive one change-set per update. Their
//! mirrors merge what arrives between two frames (ADR-031): `value` is applied to the UI once
//! per frame with its latest value, while `progress`, declared `#[undra(no_coalesce)]`, is
//! applied step by step, as a progress bar wants.
//!
//! * [`Stress::burst`] commits `n` updates now, in a tight loop. Contract scenario S18
//!   (`contract-tests/scenarios.md`) drives it on every platform.
//! * [`Stress::start`] runs a generator at a rate (updates per second) until [`Stress::stop`]:
//!   the stress screen of the playground, and what the landing page's "Push it" starts. It is
//!   **paced by the `Timer` port** (a task that sleeps 10 ms between ticks, so the host's timer
//!   wakes it, never a host loop) and **corrected by the `Clock` port**: each tick commits the
//!   updates the time since the previous tick earned (`rate x elapsed`, in integer nanoseconds,
//!   the remainder carried), so a timer that fires late or coarsely still averages the
//!   requested rate. A tick never commits more than 100 ms of work: after a long stall (a
//!   hidden tab, a suspended app) the generator forgets the backlog instead of bursting, and
//!   the store's `generated` signal shows what was really produced, which is the number the
//!   screen reports.
//!
//! The generator reads no wall clock, no random source and starts no thread (R12): the sequence
//! of committed states is a pure function of the mode, the rate and the times the timer fires,
//! and under `undra::ports::fakes` it is exact.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use undra::prelude::*;

/// The fastest generator [`Stress::start`] accepts, in updates per second.
pub const MAX_RATE: u32 = 1_000_000;

/// How long the generator sleeps between ticks.
const TICK: Duration = Duration::from_millis(10);

const NS_PER_SEC: u128 = 1_000_000_000;

/// Which signal `burst` and `start` write.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StressMode {
    /// `value` + 1 per transaction: a firehose the platforms apply once per frame.
    Firehose,
    /// `progress` + 1 per transaction: a `no_coalesce` signal the platforms apply step by step.
    Progress,
}

/// Why `start` refused to start.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StressError {
    /// The rate is zero or above the fastest generator there is (1,000,000 updates a second).
    #[error("{rate} updates per second is outside 1..={max}")]
    RateOutOfRange {
        /// The rate that was asked for.
        rate: u32,
        /// The fastest rate there is.
        max: u32,
    },
}

/// What the running generator task reads each tick; the store and the task share it.
struct Generator {
    /// Bumped when a generator starts or stops: a task whose epoch is not current ends.
    epoch: u64,
    /// Whether a task is running for the current epoch.
    active: bool,
    mode: StressMode,
    rate: u32,
}

fn lock(generator: &Mutex<Generator>) -> MutexGuard<'_, Generator> {
    generator.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A store that commits many transactions, for the coalesced-delivery scenarios and the
/// playground's stress screen.
#[undra::store(restore = "Self::assemble")]
pub struct Stress {
    ctx: Ctx,
    generator: Arc<Mutex<Generator>>,
    /// Written by the firehose mode: one more per transaction.
    value: Signal<u64>,
    /// Written by the progress mode: one more per transaction. Every value reaches the
    /// platforms' mirrors.
    #[undra(no_coalesce)]
    progress: Signal<u32>,
    /// How many updates the generator `start` runs has committed (to `value` or `progress`),
    /// written once per tick. `burst` does not count.
    generated: Signal<u64>,
    /// Whether the generator is running.
    running: Signal<bool>,
}

#[undra::api(store)]
impl Stress {
    /// A store with every signal at its start: zeros, and no generator running.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(
            ctx,
            Signal::new(0),
            Signal::new(0),
            Signal::new(0),
            Signal::new(false),
        )
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot. A
    // restored store has no generator task, so `running` is false whatever the snapshot said.
    fn assemble(
        ctx: Ctx,
        value: Signal<u64>,
        progress: Signal<u32>,
        generated: Signal<u64>,
        running: Signal<bool>,
    ) -> Self {
        if running.get() {
            running.set(false);
        }
        Self {
            ctx,
            generator: Arc::new(Mutex::new(Generator {
                epoch: 0,
                active: false,
                mode: StressMode::Firehose,
                rate: 0,
            })),
            value,
            progress,
            generated,
            running,
        }
    }

    /// Commits `transactions` transactions now, each one write of the signal `mode` names: one
    /// change-set per transaction, the way data that arrives in the core on its own does.
    pub fn burst(&self, mode: StressMode, transactions: u32) {
        for _ in 0..transactions {
            write(&self.value, &self.progress, mode);
        }
    }

    /// Starts generating updates of the signal the mode names, at the given number a second,
    /// each its own transaction, until `stop`. Called while a generator runs, it retunes that
    /// generator (new mode, new rate) instead of starting a second one. Fails, starting
    /// nothing, when the rate is zero or above 1,000,000.
    pub fn start(&self, mode: StressMode, per_second: u32) -> Result<(), StressError> {
        if per_second == 0 || per_second > MAX_RATE {
            return Err(StressError::RateOutOfRange {
                rate: per_second,
                max: MAX_RATE,
            });
        }
        let epoch = {
            let mut g = lock(&self.generator);
            g.mode = mode;
            g.rate = per_second;
            if g.active {
                return Ok(());
            }
            g.active = true;
            g.epoch += 1;
            g.epoch
        };
        // The task holds the signals and a weak reference to the shared state, never the store:
        // when the store goes away (released, or replaced by a restore) the next tick ends it.
        let task = Task {
            ctx: self.ctx.clone(),
            generator: Arc::downgrade(&self.generator),
            epoch,
            value: self.value.clone(),
            progress: self.progress.clone(),
            generated: self.generated.clone(),
        };
        self.ctx.spawn(task.run());
        self.running.set(true);
        Ok(())
    }

    /// Stops the generator: no update is committed after this returns. `generated` keeps its
    /// count. Does nothing when no generator runs.
    pub fn stop(&self) {
        {
            let mut g = lock(&self.generator);
            if !g.active {
                return;
            }
            g.active = false;
            g.epoch += 1;
        }
        self.running.set(false);
    }
}

/// One update of the signal `mode` names, in its own transaction.
fn write(value: &Signal<u64>, progress: &Signal<u32>, mode: StressMode) {
    match mode {
        StressMode::Firehose => value.update(|v| *v = v.wrapping_add(1)),
        StressMode::Progress => progress.update(|p| *p = p.wrapping_add(1)),
    }
}

/// The generator task: what [`Stress::start`] spawns.
struct Task {
    ctx: Ctx,
    generator: Weak<Mutex<Generator>>,
    epoch: u64,
    value: Signal<u64>,
    progress: Signal<u32>,
    generated: Signal<u64>,
}

impl Task {
    async fn run(self) {
        let clock = self.ctx.clock();
        let mut last = clock.monotonic_ns();
        // Fractions of an update earned and not yet committed, in update-nanoseconds.
        let mut carry: u128 = 0;
        loop {
            self.ctx.sleep(TICK).await;
            let Some(shared) = self.generator.upgrade() else {
                return;
            };
            let (mode, rate) = {
                let g = lock(&shared);
                if g.epoch != self.epoch {
                    return;
                }
                (g.mode, g.rate)
            };
            let now = clock.monotonic_ns();
            carry += u128::from(rate) * u128::from(now.saturating_sub(last));
            last = now;
            // At most 100 ms of work per tick: a stall is not paid back as a burst.
            let cap = u128::from((rate / 10).max(1));
            let due = (carry / NS_PER_SEC).min(cap);
            carry %= NS_PER_SEC;
            // `due <= cap <= MAX_RATE / 10`, so it fits.
            let due = u64::try_from(due).unwrap_or(0);
            for _ in 0..due {
                write(&self.value, &self.progress, mode);
            }
            if due > 0 {
                self.generated.update(|g| *g = g.wrapping_add(due));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use undra::meta::{collect_schema, ids};
    use undra::ports::fakes::{self, Fakes};
    use undra::runtime::testing::{ReplyRecord, TestRuntime};
    use undra::signals::ALL_SIGNALS;
    use undra::wire::payload::{CallTarget, ChangeOp, ReplyStatus};
    use undra::wire::{Decode, Encode, Writer};

    use super::*;

    const VALUE: u32 = 0;
    const PROGRESS: u32 = 1;
    const GENERATED: u32 = 2;
    const RUNNING: u32 = 3;

    fn construct(t: &TestRuntime) -> Handle {
        let reply = t.call_sync(
            CallTarget::Constructor {
                type_id: ids::type_id("Stress"),
                method_id: ids::method_id("Stress", "new"),
            },
            1,
            &[],
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        Handle::decode_exact(&reply.body).expect("a handle")
    }

    fn call(t: &TestRuntime, store: Handle, method: &str, args: &[u8]) -> ReplyRecord {
        t.call_sync(
            CallTarget::Method {
                handle: store,
                method_id: ids::method_id("Stress", method),
            },
            2,
            args,
        )
    }

    fn burst(t: &TestRuntime, store: Handle, mode: StressMode, transactions: u32) {
        let mut w = Writer::new();
        mode.encode(&mut w);
        transactions.encode(&mut w);
        let reply = call(t, store, "burst", w.as_slice());
        assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    }

    #[test]
    fn a_firehose_burst_is_one_change_set_per_transaction() {
        let t = TestRuntime::new();
        let store = construct(&t);
        t.runtime().observe(store.0, ALL_SIGNALS, true);
        t.host().take_decoded_change_sets();
        burst(&t, store, StressMode::Firehose, 1000);
        let sets = t.host().take_decoded_change_sets();
        assert_eq!(sets.len(), 1000);
        for (i, set) in sets.iter().enumerate() {
            assert_eq!(set.entries.len(), 1);
            let entry = &set.entries[0];
            assert_eq!((entry.signal_id, entry.op), (VALUE, ChangeOp::Full));
            assert_eq!(u64::decode_exact(&entry.value).unwrap(), i as u64 + 1);
        }
        let txns: Vec<u64> = sets.iter().map(|s| s.txn_id).collect();
        assert!(txns.windows(2).all(|w| w[0] < w[1]), "commit order");
    }

    #[test]
    fn progress_is_delivered_even_while_nobody_observes_it() {
        let t = TestRuntime::new();
        let store = construct(&t);
        t.host().take_decoded_change_sets();
        burst(&t, store, StressMode::Progress, 3);
        burst(&t, store, StressMode::Firehose, 3);
        let sets = t.host().take_decoded_change_sets();
        let progress: Vec<u32> = sets
            .iter()
            .flat_map(|s| &s.entries)
            .map(|e| {
                assert_eq!(
                    e.signal_id, PROGRESS,
                    "only the no_coalesce signal is delivered"
                );
                u32::decode_exact(&e.value).unwrap()
            })
            .collect();
        assert_eq!(progress, [1, 2, 3]);
    }

    #[test]
    fn the_schema_marks_progress_no_coalesce() {
        let schema = collect_schema("playground-core");
        let stress = schema.objects.iter().find(|o| o.name == "Stress").unwrap();
        let signals: Vec<(&str, bool)> = stress
            .store
            .as_ref()
            .unwrap()
            .signals
            .iter()
            .map(|s| (s.name.as_str(), s.no_coalesce))
            .collect();
        assert_eq!(
            signals,
            [
                ("value", false),
                ("progress", true),
                ("generated", false),
                ("running", false)
            ]
        );
    }

    /// What an observer of every signal has been told so far.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct Tally {
        value: u64,
        progress: u32,
        generated: u64,
        running: bool,
        /// Change-sets that carried each signal, since the observe.
        value_sets: usize,
        progress_sets: usize,
        generated_sets: usize,
        running_sets: usize,
        /// Every entry in delivery order: `(signal id, value bytes)`.
        entries: Vec<(u32, Vec<u8>)>,
    }

    /// A stress store on a test runtime whose time is a fake clock, observed from the start.
    struct App {
        t: TestRuntime,
        fakes: Fakes,
        store: Handle,
        tally: RefCell<Tally>,
    }

    impl App {
        fn new() -> App {
            let t = TestRuntime::new();
            let fakes = fakes::install(&t);
            let store = construct(&t);
            t.runtime().observe(store.0, ALL_SIGNALS, true);
            let app = App {
                t,
                fakes,
                store,
                tally: RefCell::new(Tally::default()),
            };
            // The initial values are not what the tests count.
            app.tally();
            *app.tally.borrow_mut() = Tally::default();
            app
        }

        /// Folds the change-sets delivered since the last call into the tally and returns it.
        fn tally(&self) -> Tally {
            let mut tally = self.tally.borrow_mut();
            for set in self.t.host().take_decoded_change_sets() {
                let mut seen = [false; 4];
                for entry in &set.entries {
                    assert_eq!(entry.op, ChangeOp::Full, "scalars are sent whole");
                    seen[entry.signal_id as usize] = true;
                    tally.entries.push((entry.signal_id, entry.value.clone()));
                    match entry.signal_id {
                        VALUE => tally.value = u64::decode_exact(&entry.value).unwrap(),
                        PROGRESS => tally.progress = u32::decode_exact(&entry.value).unwrap(),
                        GENERATED => tally.generated = u64::decode_exact(&entry.value).unwrap(),
                        RUNNING => tally.running = bool::decode_exact(&entry.value).unwrap(),
                        other => panic!("signal {other}"),
                    }
                }
                tally.value_sets += usize::from(seen[VALUE as usize]);
                tally.progress_sets += usize::from(seen[PROGRESS as usize]);
                tally.generated_sets += usize::from(seen[GENERATED as usize]);
                tally.running_sets += usize::from(seen[RUNNING as usize]);
            }
            tally.clone()
        }

        fn start(&self, mode: StressMode, per_second: u32) -> Result<(), StressError> {
            let mut w = Writer::new();
            mode.encode(&mut w);
            per_second.encode(&mut w);
            let reply = call(&self.t, self.store, "start", w.as_slice());
            match reply.status {
                ReplyStatus::Ok => Ok(()),
                ReplyStatus::Error => Err(StressError::decode_exact(&reply.body).unwrap()),
                other => panic!("start: {other:?} {reply:?}"),
            }
        }

        fn stop(&self) {
            let reply = call(&self.t, self.store, "stop", &[]);
            assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
        }

        /// Time passes; every timer that comes due fires at its deadline, on time.
        fn advance_ms(&self, ms: u64) {
            self.fakes.advance(&self.t, Duration::from_millis(ms));
        }

        /// Time passes but the host's timer is late: it fires at its deadline in the fake
        /// clock's books, yet the core only gets to run `ms` later, like a main thread that
        /// was busy.
        fn stall_ms(&self, ms: u64) {
            // The generator's first sleep is armed on the fake clock before time passes.
            self.t.run_pending();
            self.fakes.sync_timers(&self.t);
            self.fakes.clock.advance(Duration::from_millis(ms));
            self.t.run_pending();
            self.fakes.sync_timers(&self.t);
        }

        fn pending_timers(&self) -> usize {
            self.fakes.sync_timers(&self.t);
            self.fakes.clock.pending_timers()
        }
    }

    #[test]
    fn a_generator_commits_rate_times_seconds_updates() {
        let app = App::new();
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(2_000);
        let tally = app.tally();
        assert_eq!(tally.value, 2_000);
        assert_eq!(tally.generated, 2_000);
        assert!(tally.running);
        assert_eq!(tally.progress, 0);
    }

    #[test]
    fn every_update_is_its_own_change_set_and_the_counter_is_written_once_per_tick() {
        let app = App::new();
        app.start(StressMode::Firehose, 10_000).unwrap();
        app.advance_ms(1_000);
        let tally = app.tally();
        assert_eq!(tally.value, 10_000);
        // One change-set per update, in order: the platform mirror is what merges them.
        assert_eq!(tally.value_sets, 10_000);
        let values: Vec<u64> = tally
            .entries
            .iter()
            .filter(|(signal, _)| *signal == VALUE)
            .map(|(_, bytes)| u64::decode_exact(bytes).unwrap())
            .collect();
        assert!(
            values.iter().copied().eq(1..=10_000),
            "every value, in order"
        );
        // 100 ticks a second, one write of `generated` each; `running` was written once.
        assert_eq!(tally.generated_sets, 100);
        assert_eq!(tally.running_sets, 1);
        assert_eq!(tally.generated, 10_000);
    }

    #[test]
    fn progress_mode_writes_the_no_coalesce_signal() {
        let app = App::new();
        app.start(StressMode::Progress, 2_000).unwrap();
        app.advance_ms(1_000);
        let tally = app.tally();
        assert_eq!((tally.progress, tally.value), (2_000, 0));
        assert_eq!(tally.progress_sets, 2_000);
        assert_eq!(tally.generated, 2_000);
    }

    #[test]
    fn a_rate_that_is_not_a_multiple_of_a_hundred_carries_its_remainder() {
        let app = App::new();
        // 1.5 updates per tick: 1, then 2, then 1, ...
        app.start(StressMode::Firehose, 150).unwrap();
        app.advance_ms(10);
        assert_eq!(app.tally().value, 1);
        app.advance_ms(10);
        assert_eq!(app.tally().value, 3);
        app.advance_ms(1_980);
        assert_eq!(app.tally().value, 300, "150 a second for two seconds");
    }

    #[test]
    fn one_update_a_second_is_one_update_a_second() {
        let app = App::new();
        app.start(StressMode::Firehose, 1).unwrap();
        app.advance_ms(3_000);
        assert_eq!(app.tally().value, 3);
    }

    #[test]
    fn a_late_timer_is_paid_back_from_the_clock_and_a_long_stall_is_capped() {
        let app = App::new();
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(10);
        assert_eq!(app.tally().value, 10);
        // The timer fires 35 ms late: the tick commits the 35 ms the clock says passed.
        app.stall_ms(35);
        assert_eq!(app.tally().value, 45);
        // Half a second of nothing (a hidden tab): at most 100 ms of work, the rest forgotten.
        app.stall_ms(500);
        let tally = app.tally();
        assert_eq!(tally.value, 145);
        assert_eq!(
            tally.generated, 145,
            "the counter says what was really produced"
        );
        // And the next tick is on rate again: no carried debt.
        app.advance_ms(10);
        assert_eq!(app.tally().value, 155);
    }

    #[test]
    fn the_average_rate_tracks_the_target_under_jittery_timers() {
        let app = App::new();
        app.start(StressMode::Firehose, 5_000).unwrap();
        // Ticks that arrive 10 to 90 ms apart, never on the 10 ms beat: each commits what the
        // clock says it earned, so after any tick the total is exactly rate x elapsed.
        let mut elapsed_ms = 0;
        for _ in 0..20 {
            for ms in [10, 13, 17, 10, 20, 15, 12, 18, 90] {
                app.stall_ms(ms);
                elapsed_ms += ms;
                assert_eq!(app.tally().value, 5 * elapsed_ms, "after {elapsed_ms} ms");
            }
        }
    }

    #[test]
    fn stop_stops_and_a_stopped_store_has_no_timer_left() {
        let app = App::new();
        app.stop(); // nothing runs: nothing happens
        assert_eq!(app.tally(), Tally::default());
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(100);
        let before = app.tally();
        assert_eq!(
            (before.value, before.generated, before.running),
            (100, 100, true)
        );
        app.stop();
        assert!(!app.tally().running);
        app.advance_ms(5_000);
        let after = app.tally();
        assert_eq!(
            (after.value, after.generated),
            (100, 100),
            "nothing after stop"
        );
        assert!(!after.running);
        assert_eq!(app.pending_timers(), 0, "the task ended at its next tick");
    }

    #[test]
    fn a_start_while_running_retunes_the_one_generator() {
        let app = App::new();
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(1_000);
        app.start(StressMode::Progress, 4_000).unwrap();
        app.advance_ms(1_000);
        let tally = app.tally();
        assert_eq!((tally.value, tally.progress), (1_000, 4_000));
        assert_eq!(tally.generated, 5_000);
        assert_eq!(tally.running_sets, 1, "it never stopped running");
        assert_eq!(app.pending_timers(), 1, "one task, not two");
    }

    #[test]
    fn a_stop_and_start_inside_one_tick_leaves_one_generator() {
        let app = App::new();
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(5);
        app.stop();
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(1_000);
        let tally = app.tally();
        assert_eq!(tally.value, 1_000);
        assert_eq!(tally.generated, 1_000);
        assert!(tally.running);
        assert_eq!(app.pending_timers(), 1);
    }

    #[test]
    fn a_stopped_generator_starts_again_and_keeps_counting() {
        let app = App::new();
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(500);
        app.stop();
        app.advance_ms(500);
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(500);
        let tally = app.tally();
        assert_eq!((tally.value, tally.generated), (1_000, 1_000));
        assert_eq!(tally.running_sets, 3, "started, stopped, started");
    }

    #[test]
    fn rates_outside_one_to_the_maximum_are_typed_errors_and_start_nothing() {
        let app = App::new();
        for rate in [0, MAX_RATE + 1, u32::MAX] {
            assert_eq!(
                app.start(StressMode::Firehose, rate),
                Err(StressError::RateOutOfRange {
                    rate,
                    max: MAX_RATE
                })
            );
        }
        app.advance_ms(1_000);
        assert_eq!(app.tally(), Tally::default());
        assert_eq!(app.pending_timers(), 0);
        // The edges are accepted: one tick of the fastest generator is 10,000 updates.
        app.start(StressMode::Firehose, MAX_RATE).unwrap();
        app.advance_ms(10);
        assert_eq!(app.tally().value, 10_000);
    }

    #[test]
    fn the_error_message_says_the_range() {
        let message = StressError::RateOutOfRange {
            rate: 0,
            max: MAX_RATE,
        }
        .to_string();
        assert_eq!(message, "0 updates per second is outside 1..=1000000");
    }

    #[test]
    fn a_burst_does_not_count_as_generated() {
        let app = App::new();
        burst(&app.t, app.store, StressMode::Firehose, 1_000);
        let tally = app.tally();
        assert_eq!(
            (tally.value, tally.generated, tally.value_sets),
            (1_000, 0, 1_000)
        );
        assert_eq!(
            tally.generated_sets, 0,
            "a burst is one change-set per update and nothing else"
        );
    }

    #[test]
    fn releasing_the_store_ends_its_generator() {
        let app = App::new();
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(100);
        assert_eq!(app.tally().value, 100);
        app.t.runtime().release(app.store.0);
        app.advance_ms(1_000);
        assert_eq!(app.tally().value, 100);
        assert_eq!(app.pending_timers(), 0);
    }

    #[test]
    fn a_restored_store_is_stopped_and_the_old_generator_ends() {
        let app = App::new();
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(100);
        let snapshot = app.t.runtime().snapshot();
        app.t.runtime().restore(&snapshot).expect("restores");
        let tally = app.tally();
        assert_eq!(
            (tally.value, tally.generated),
            (100, 100),
            "the counters survive"
        );
        assert!(!tally.running, "a restored store has no generator");
        app.advance_ms(1_000);
        let after = app.tally();
        assert_eq!((after.value, after.generated), (100, 100));
        assert_eq!(app.pending_timers(), 0);
        // And it starts again from the restored counters.
        app.start(StressMode::Firehose, 1_000).unwrap();
        app.advance_ms(100);
        assert_eq!(app.tally().value, 200);
    }

    #[test]
    fn the_same_timeline_commits_the_same_states() {
        let run = || {
            let app = App::new();
            app.start(StressMode::Firehose, 7_300).unwrap();
            app.advance_ms(250);
            app.stall_ms(37);
            app.start(StressMode::Progress, 1_234).unwrap();
            app.advance_ms(400);
            app.stop();
            app.advance_ms(100);
            app.tally().entries
        };
        let first = run();
        assert!(!first.is_empty());
        assert_eq!(
            first,
            run(),
            "a pure function of the mode, the rate and the timer"
        );
    }
}
