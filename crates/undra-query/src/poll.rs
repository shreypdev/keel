//! Interval polling (ADR-043 decision 1): an observed entry refetches on its own, `interval`
//! after its previous fetch **ended**.
//!
//! # Semantics
//!
//! TanStack's `refetchInterval`, not `setInterval`:
//!
//! * While an entry has at least one observer, the client believes it is online and the app is
//!   `Active` (or the query polls in the background), the next refetch is scheduled `interval`
//!   after the end of the previous fetch: its success, or its failure after the retries. A fetch
//!   that takes longer than the interval therefore never overlaps the next one, and a struggling
//!   server is not hammered. A tick that finds a fetch in flight does nothing.
//! * `Background` and `Inactive` (unless the query polls in the background) and going offline
//!   **pause** polling: the timer is cancelled. `Active` and online resume it with the existing
//!   triggers (a stale entry refetches, going online refetches every observed entry) and the next
//!   poll is scheduled from the end of that fetch; an entry that is fresh on resume and has no
//!   fetch in flight gets a new timer, a full interval from the moment of the resume.
//! * Releasing the last observer cancels the timer.
//!
//! # The interval
//!
//! The attribute (`interval = "30s"`, [`QueryDef::INTERVAL_MS`](crate::QueryDef::INTERVAL_MS)) is
//! each new observer's default. [`QueryHandle::set_poll_interval`](crate::QueryHandle::set_poll_interval)
//! gives one observer its own, `None` clears it again; the entry polls at the **smallest**
//! interval among its observers, the default counting for each observer without an override of its
//! own. A query without the attribute is polled by an observer that sets one.
//!
//! An interval is never below [`MIN_POLL_INTERVAL_MS`] (what the attribute enforces at compile
//! time: use a stream for real-time data) nor above [`MAX_POLL_INTERVAL_MS`]; one outside is
//! brought inside, so a hostile or mistaken value can neither spin the core nor overflow a timer.
//! Changing the interval keeps the moment the interval counts from, so a storm of changes neither
//! starves the poll nor makes it fire early: the next poll is at `max(now, from + interval)`.
//!
//! # Determinism
//!
//! The timer is a task sleeping on the `Timer` port, holding only a `WeakCtx` (ADR-034); the
//! moments are read from the `Clock` port. Under `undra_ports::fakes` a test advances both and
//! sees every poll.

use core::time::Duration;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use undra_ports::AppState;
use undra_runtime::executor::TaskId;
use undra_runtime::{Ctx, WeakCtx};

use crate::key::QueryKey;
use crate::shared::{Entry, Fx, Shared};

/// The shortest polling interval, in milliseconds: one second. The `interval` attribute refuses
/// less at compile time; a smaller value set at run time is raised to this.
pub const MIN_POLL_INTERVAL_MS: u64 = 1_000;

/// The longest polling interval, in milliseconds: one week. A larger value is lowered to this.
pub const MAX_POLL_INTERVAL_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

/// An interval brought inside `MIN_POLL_INTERVAL_MS..=MAX_POLL_INTERVAL_MS`.
pub(crate) fn clamp_interval_ms(ms: u64) -> u64 {
    ms.clamp(MIN_POLL_INTERVAL_MS, MAX_POLL_INTERVAL_MS)
}

/// What the polling of one entry holds.
#[derive(Default)]
pub(crate) struct Polling {
    /// The intervals observers set for themselves: `(sink id, milliseconds)`, one per observer.
    overrides: Vec<(u64, u64)>,
    /// The timer of the next poll, while one is armed.
    timer: Option<PollTimer>,
    /// The moment the interval counts from: the end of the last fetch, or when polling (re)started.
    base: i64,
}

/// An armed poll.
struct PollTimer {
    /// Identifies the timer, so a tick of a timer that was replaced does nothing.
    serial: u64,
    task: TaskId,
    /// The interval the timer was armed with.
    interval_ms: u64,
}

impl Polling {
    /// The interval the entry polls at, in milliseconds: the smallest among its observers.
    fn effective(&self, default_ms: Option<u64>, observers: u32) -> Option<u64> {
        let mut best = self.overrides.iter().map(|(_, ms)| *ms).min();
        let defaulted = usize::try_from(observers).map_or(true, |n| n > self.overrides.len());
        if let (true, Some(ms)) = (defaulted, default_ms) {
            let ms = clamp_interval_ms(ms);
            best = Some(best.map_or(ms, |b| b.min(ms)));
        }
        best
    }

    /// Forgets the override of observer `sink_id`; whether it had one.
    pub(crate) fn clear_override(&mut self, sink_id: u64) -> bool {
        let before = self.overrides.len();
        self.overrides.retain(|(id, _)| *id != sink_id);
        self.overrides.len() != before
    }

    /// Gives observer `sink_id` the interval `ms`.
    fn set_override(&mut self, sink_id: u64, ms: u64) {
        match self.overrides.iter_mut().find(|(id, _)| *id == sink_id) {
            Some(slot) => slot.1 = ms,
            None => self.overrides.push((sink_id, ms)),
        }
    }

    /// Nothing is overridden and no timer is armed: an entry whose query declares no interval has
    /// nothing to reschedule.
    fn idle(&self) -> bool {
        self.overrides.is_empty() && self.timer.is_none()
    }
}

/// Where the interval counts from when the timer is armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Base {
    /// From now: a fetch just ended.
    Now,
    /// From where it was, unless no timer was armed (then from now).
    Keep,
}

fn app_state_code(state: AppState) -> u8 {
    match state {
        AppState::Active => 0,
        AppState::Inactive => 1,
        AppState::Background => 2,
    }
}

impl Shared {
    /// Whether the app is `Active`, as far as the client has been told (it starts `true`: an app
    /// that has heard nothing yet is assumed to be in front, like it is assumed online).
    fn is_active(&self) -> bool {
        self.app_state.load(Ordering::SeqCst) == 0
    }

    /// The lifecycle state changed: **the one place the pause or resume of polling is decided**
    /// (the lifecycle input of the query client enters here, from `undra_ports::on_lifecycle_changed`).
    ///
    /// `Active` refetches the observed entries that went stale while the app was away (and reads a
    /// queue that could not be read before); `Background` is another chance to read that queue.
    /// Every state change then reschedules every entry's poll: `Active` arms the timers that
    /// paused, `Background` and `Inactive` cancel those of the queries that do not poll in the
    /// background.
    pub(crate) fn on_lifecycle(self: &Arc<Self>, ctx: &Ctx, state: AppState) {
        self.app_state
            .store(app_state_code(state), Ordering::SeqCst);
        match state {
            AppState::Active => self.on_active(ctx),
            AppState::Background => self.retry_unreadable_queue(ctx),
            AppState::Inactive => {}
        }
        self.reschedule_all(ctx);
    }

    /// The interval entry `entry` should be polled at now, if it should be polled at all.
    fn poll_wanted(&self, entry: &Entry) -> Option<u64> {
        if entry.observers == 0 || !self.is_online() {
            return None;
        }
        let interval = entry
            .polling
            .effective(entry.vt.interval_ms, entry.observers)?;
        (self.is_active() || entry.vt.poll_in_background).then_some(interval)
    }

    /// Cancels the entry's timer, if one is armed (a fetch is starting: the next poll is
    /// scheduled when it ends).
    pub(crate) fn cancel_poll(&self, entry: &mut Entry, fx: &mut Fx) {
        if let Some(timer) = entry.polling.timer.take() {
            fx.cancel(timer.task);
        }
    }

    /// Brings the entry's timer in line with what is wanted: cancels it when the entry should not
    /// poll (no observer, offline, paused, a fetch in flight), arms it when it should, and
    /// re-arms it when the interval changed. `now` is the Clock port's reading.
    pub(crate) fn reschedule_poll(
        self: &Arc<Self>,
        ctx: &Ctx,
        key: &QueryKey,
        entry: &mut Entry,
        fx: &mut Fx,
        now: i64,
        base: Base,
    ) {
        if entry.polling.idle() && entry.vt.interval_ms.is_none() {
            return;
        }
        let wanted = if entry.inflight.is_some() {
            None
        } else {
            self.poll_wanted(entry)
        };
        let Some(interval) = wanted else {
            self.cancel_poll(entry, fx);
            return;
        };
        let armed = entry.polling.timer.as_ref().map(|t| t.interval_ms);
        if armed == Some(interval) && base == Base::Keep {
            return;
        }
        if base == Base::Now || armed.is_none() {
            entry.polling.base = now;
        }
        self.cancel_poll(entry, fx);
        // Counted from the base, and never longer than the interval (a wall clock that went
        // backwards must not make the wait longer).
        let due = entry.polling.base.saturating_add(to_i64(interval));
        let wait = u64::try_from(due.saturating_sub(now))
            .unwrap_or(0)
            .min(interval);
        let serial = self.new_serial();
        let task = ctx.spawn(run_poll(
            self.clone(),
            ctx.downgrade(),
            key.clone(),
            serial,
            Duration::from_millis(wait),
        ));
        entry.polling.timer = Some(PollTimer {
            serial,
            task,
            interval_ms: interval,
        });
    }

    /// Reschedules the poll of one entry, reading the clock first (the lock is a leaf).
    pub(crate) fn reschedule_one(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey) {
        let now = self.now(ctx);
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            if let Some(entry) = state.entries.get_mut(key) {
                self.reschedule_poll(ctx, key, entry, &mut fx, now, Base::Keep);
            }
        }
        fx.run(ctx);
    }

    /// Reschedules the poll of every entry (the app changed state, or the client went on or
    /// offline).
    pub(crate) fn reschedule_all(self: &Arc<Self>, ctx: &Ctx) {
        let now = self.now(ctx);
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            for (key, entry) in &mut state.entries {
                self.reschedule_poll(ctx, key, entry, &mut fx, now, Base::Keep);
            }
        }
        fx.run(ctx);
    }

    /// Gives the observer `sink_id` of `key` its own polling interval, or (`None`) takes it away
    /// again. An observer that is gone has no effect.
    pub(crate) fn set_poll_interval(
        self: &Arc<Self>,
        ctx: &Ctx,
        key: &QueryKey,
        sink_id: u64,
        interval: Option<Duration>,
    ) {
        let now = self.now(ctx);
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let Some(entry) = state.entries.get_mut(key) else {
                return;
            };
            if !entry.sinks.iter().any(|(id, _)| *id == sink_id) {
                // Released while the call was on its way: nothing to attach it to.
                return;
            }
            match interval {
                Some(interval) => {
                    let ms = u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
                    entry.polling.set_override(sink_id, clamp_interval_ms(ms));
                }
                None => {
                    entry.polling.clear_override(sink_id);
                }
            }
            self.reschedule_poll(ctx, key, entry, &mut fx, now, Base::Keep);
        }
        fx.run(ctx);
    }

    /// The timer of `serial` fired: refetch, unless the entry no longer wants to poll or a fetch
    /// is already running (then the next poll is scheduled when that one ends).
    fn poll_tick(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey, serial: u64) {
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let Some(entry) = state.entries.get_mut(key) else {
                return;
            };
            if entry.polling.timer.as_ref().map(|t| t.serial) != Some(serial) {
                return;
            }
            entry.polling.timer = None;
            if entry.inflight.is_some() || self.poll_wanted(entry).is_none() {
                return;
            }
            self.start_fetch(ctx, key, entry, &mut fx);
        }
        fx.run(ctx);
    }
}

fn to_i64(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

/// The timer task of one poll: sleeps on the `Timer` port holding only a weak context.
async fn run_poll(shared: Arc<Shared>, weak: WeakCtx, key: QueryKey, serial: u64, wait: Duration) {
    if weak.sleep(wait).await.is_err() {
        return;
    }
    if let Ok(ctx) = weak.upgrade() {
        shared.poll_tick(&ctx, &key, serial);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_are_brought_inside_the_bounds() {
        assert_eq!(clamp_interval_ms(0), MIN_POLL_INTERVAL_MS);
        assert_eq!(clamp_interval_ms(999), MIN_POLL_INTERVAL_MS);
        assert_eq!(clamp_interval_ms(1_000), 1_000);
        assert_eq!(clamp_interval_ms(30_000), 30_000);
        assert_eq!(clamp_interval_ms(u64::MAX), MAX_POLL_INTERVAL_MS);
    }

    #[test]
    fn the_entry_polls_at_the_smallest_interval_among_its_observers() {
        let mut polling = Polling::default();
        assert_eq!(
            polling.effective(None, 1),
            None,
            "no attribute, no override"
        );
        assert_eq!(polling.effective(Some(30_000), 1), Some(30_000));

        polling.set_override(1, 5_000);
        // One observer, and it overrides: the attribute is not in play.
        assert_eq!(polling.effective(Some(30_000), 1), Some(5_000));
        assert_eq!(polling.effective(Some(2_000), 1), Some(5_000));
        // A second observer without an override brings the default back in.
        assert_eq!(polling.effective(Some(30_000), 2), Some(5_000));
        assert_eq!(polling.effective(Some(2_000), 2), Some(2_000));
        // A query without the attribute is polled by the one observer that sets an interval.
        assert_eq!(polling.effective(None, 3), Some(5_000));

        polling.set_override(1, 9_000);
        assert_eq!(polling.overrides.len(), 1, "an observer has one override");
        polling.set_override(2, 4_000);
        assert_eq!(polling.effective(None, 2), Some(4_000));
        assert!(polling.clear_override(2));
        assert!(!polling.clear_override(2), "nothing left to clear");
        assert_eq!(polling.effective(None, 2), Some(9_000));
    }

    #[test]
    fn an_attribute_below_the_floor_is_raised_too() {
        // A hand-written `QueryDef` can say `INTERVAL_MS = Some(0)`: it must not spin the core.
        let polling = Polling::default();
        assert_eq!(polling.effective(Some(0), 1), Some(MIN_POLL_INTERVAL_MS));
        assert_eq!(
            polling.effective(Some(u64::MAX), 1),
            Some(MAX_POLL_INTERVAL_MS)
        );
    }

    #[test]
    fn app_states_have_distinct_codes_and_active_is_zero() {
        assert_eq!(app_state_code(AppState::Active), 0);
        assert_ne!(
            app_state_code(AppState::Inactive),
            app_state_code(AppState::Background)
        );
    }
}
