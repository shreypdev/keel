//! Background runs: the OS grants a window, the core drains in it (ADR-046 decision 3).
//!
//! A *background task* is a unit of work worth finishing while the app is not on screen: replay the
//! offline queue, refetch stale queries, flush what is waiting to be persisted. Layered crates
//! register theirs on the runtime ([`Runtime::add_background_task`], what `undra-query` does when it
//! starts); an app registers its own with [`Runtime::add_background_task`] or, for a plain function,
//! by submitting a [`BackgroundTask`] with `inventory`.
//!
//! [`run`] is what the standard function `run_background(deadline_ms)` of `undra-ports` calls: it
//! starts every task at once and returns when they are all done or when the window is nearly over
//! (`deadline - `[`MARGIN`], so the host still has time to tell the OS it finished), whichever comes
//! first. Work a task did stays done (the offline queue persists per item, ADR-037), so a run that
//! is cut short, by the deadline or by the host cancelling the call, loses nothing; the report says
//! how much is still pending.
//!
//! The clock is a parameter ([`run`]'s `now`), not read here: `undra-ports` passes the `Clock`
//! port's monotonic reading, so a fake clock decides what a deadline means in a test (R12).

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::ctx::{Ctx, WeakCtx};
use crate::executor::TaskId;
use crate::runtime::Runtime;
use crate::stats::Stats;

/// How much of the window a run leaves to the host: it returns this long before the deadline.
pub const MARGIN: Duration = Duration::from_millis(500);

/// How a background task ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundOutcome {
    /// Nothing is left for this task to do.
    Done,
    /// The task stopped with work left (offline, a read that failed, the deadline).
    Incomplete,
}

/// What a background task's future is.
pub type BackgroundFuture = Pin<Box<dyn Future<Output = BackgroundOutcome> + Send + 'static>>;

/// The window a background run grants, and where a task says how much it got done.
#[derive(Clone)]
pub struct Deadline {
    window: Arc<Window>,
}

struct Window {
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
    at_ns: u64,
    replayed: AtomicU32,
    refetched: AtomicU32,
}

impl Deadline {
    fn new(now: Arc<dyn Fn() -> u64 + Send + Sync>, budget: Duration) -> Deadline {
        let at_ns = now().saturating_add(u64::try_from(budget.as_nanos()).unwrap_or(u64::MAX));
        Deadline {
            window: Arc::new(Window {
                now,
                at_ns,
                replayed: AtomicU32::new(0),
                refetched: AtomicU32::new(0),
            }),
        }
    }

    /// How long the run still has, by the `Clock` port's reading.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        Duration::from_nanos(self.window.at_ns.saturating_sub((self.window.now)()))
    }

    /// Whether the run has no time left.
    #[must_use]
    pub fn expired(&self) -> bool {
        self.remaining().is_zero()
    }

    /// Counts `n` queued mutations that were sent (the report's `replayed`). Counted as they
    /// happen, so a task the deadline cuts off still shows what it did.
    pub fn note_replayed(&self, n: u32) {
        self.window.replayed.fetch_add(n, Ordering::Relaxed);
    }

    /// Counts `n` queries that were fetched again (the report's `refetched`).
    pub fn note_refetched(&self, n: u32) {
        self.window.refetched.fetch_add(n, Ordering::Relaxed);
    }
}

/// A background task a plain function can be: submit it with `inventory::submit!`. Tasks that need
/// state register a closure with [`Runtime::add_background_task`] instead.
#[derive(Clone, Copy)]
pub struct BackgroundTask {
    /// A name, unique among the tasks (a second with the same name is ignored).
    pub name: &'static str,
    /// How much work is waiting, cheaply: `0` when there is none.
    pub pending: fn(&Ctx) -> u32,
    /// Starts the task. It must hold the runtime weakly across awaits, as `ctx.downgrade()`
    /// (ADR-034); it stops when `deadline` is [expired](Deadline::expired) or the future is dropped.
    pub run: fn(&Ctx, Deadline) -> BackgroundFuture,
}

inventory::collect!(BackgroundTask);

type PendingFn = Arc<dyn Fn(&Ctx) -> u32 + Send + Sync>;
type RunFn = Arc<dyn Fn(&Ctx, Deadline) -> BackgroundFuture + Send + Sync>;

/// A task registered on one runtime.
#[derive(Clone)]
pub(crate) struct Registered {
    name: &'static str,
    pending: PendingFn,
    run: RunFn,
}

/// The tasks of one runtime, in registration order.
#[derive(Default)]
pub(crate) struct Registry {
    tasks: Mutex<Vec<Registered>>,
}

impl Registry {
    /// Registers a task unless one of that name exists.
    pub(crate) fn add(&self, name: &'static str, pending: PendingFn, run: RunFn) {
        let mut tasks = self.tasks.lock();
        if tasks.iter().all(|t| t.name != name) {
            tasks.push(Registered { name, pending, run });
        }
    }

    /// The registered tasks, then the submitted ones that no registered task shadows.
    fn all(&self) -> Vec<Registered> {
        let mut out = self.tasks.lock().clone();
        for task in inventory::iter::<BackgroundTask> {
            if out.iter().all(|t| t.name != task.name) {
                let (pending, run) = (task.pending, task.run);
                out.push(Registered {
                    name: task.name,
                    pending: Arc::new(pending),
                    run: Arc::new(run),
                });
            }
        }
        out
    }

    /// How many tasks there are.
    pub(crate) fn count(&self) -> usize {
        self.all().len()
    }

    /// The work waiting across every task (a panicking probe counts as none).
    pub(crate) fn pending(&self, ctx: &Ctx) -> u32 {
        self.all()
            .iter()
            .map(|t| crate::guard::guarded(|| (t.pending)(ctx)).unwrap_or(0))
            .fold(0_u32, u32::saturating_add)
    }
}

/// What a run did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackgroundTotals {
    /// Every task ran to completion before the deadline and nothing is pending.
    pub finished: bool,
    /// Queued mutations sent.
    pub replayed: u32,
    /// Queries fetched again.
    pub refetched: u32,
    /// Work still waiting.
    pub still_pending: u32,
}

/// Where a task leaves its outcome.
struct Slot {
    state: Mutex<(Option<BackgroundOutcome>, Option<Waker>)>,
}

impl Slot {
    fn complete(&self, outcome: BackgroundOutcome) {
        let waker = {
            let mut state = self.state.lock();
            if state.0.is_none() {
                state.0 = Some(outcome);
            }
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// Completes the slot as incomplete if the task ended without an outcome (it panicked, or was
/// cancelled).
struct SlotGuard(Arc<Slot>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0.complete(BackgroundOutcome::Incomplete);
    }
}

/// Resolves to `true` when every slot has an outcome, `false` when the budget's sleep ends first.
struct Race {
    slots: Vec<Arc<Slot>>,
    sleep: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl Future for Race {
    type Output = bool;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<bool> {
        let mut all = true;
        for slot in &self.slots {
            let mut state = slot.state.lock();
            if state.0.is_none() {
                all = false;
                state.1 = Some(cx.waker().clone());
            }
        }
        if all {
            return Poll::Ready(true);
        }
        if self.sleep.as_mut().poll(cx).is_ready() {
            return Poll::Ready(false);
        }
        Poll::Pending
    }
}

/// Cancels the tasks a run started when the run ends or is dropped (the host cancelled the call).
struct CancelOnDrop {
    ctx: WeakCtx,
    tasks: Vec<TaskId>,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Ok(ctx) = self.ctx.upgrade() {
            for id in self.tasks.drain(..) {
                ctx.cancel_task(id);
            }
        }
    }
}

/// Runs every registered background task of `ctx`'s runtime inside `deadline` (less the
/// [`MARGIN`]), reading time from `now` (nanoseconds, monotonic: the `Clock` port's).
///
/// Never panics: a task that panics is contained, reported like any panic (operation `task`) and
/// counts as incomplete.
pub async fn run(
    ctx: &Ctx,
    deadline: Duration,
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
) -> BackgroundTotals {
    let runtime: &Runtime = ctx.runtime();
    Stats::inc(&runtime.stats.background_runs);
    let budget = deadline.saturating_sub(MARGIN);
    let window = Deadline::new(now, budget);
    let tasks = runtime.background.all();
    let mut slots = Vec::with_capacity(tasks.len());
    let mut cancel = CancelOnDrop {
        ctx: ctx.downgrade(),
        tasks: Vec::with_capacity(tasks.len()),
    };
    if !budget.is_zero() {
        for task in &tasks {
            let slot = Arc::new(Slot {
                state: Mutex::new((None, None)),
            });
            slots.push(slot.clone());
            // Built under the guard: a task whose `run` panics before it returns a future is
            // incomplete, like one that panics while it runs.
            let Ok(future) = crate::guard::guarded(|| (task.run)(ctx, window.clone())) else {
                slot.complete(BackgroundOutcome::Incomplete);
                continue;
            };
            let guard = SlotGuard(slot);
            cancel.tasks.push(ctx.spawn(async move {
                let outcome = future.await;
                guard.0.complete(outcome);
            }));
        }
    }
    let sleep = ctx.sleep(budget);
    let in_time = Race {
        slots: slots.clone(),
        sleep: Box::pin(sleep),
    }
    .await;
    drop(cancel);
    let all_done = in_time
        && slots
            .iter()
            .all(|s| s.state.lock().0 == Some(BackgroundOutcome::Done));
    let pending = runtime.background.pending(ctx);
    let totals = BackgroundTotals {
        finished: pending == 0 && (tasks.is_empty() || (!budget.is_zero() && all_done)),
        replayed: window.window.replayed.load(Ordering::Relaxed),
        refetched: window.window.refetched.load(Ordering::Relaxed),
        still_pending: pending,
    };
    if totals.finished {
        Stats::inc(&runtime.stats.background_finished);
    }
    Stats::add(
        &runtime.stats.background_replayed,
        u64::from(totals.replayed),
    );
    Stats::add(
        &runtime.stats.background_refetched,
        u64::from(totals.refetched),
    );
    totals
}

impl Runtime {
    /// Registers a background task on this runtime (a second of the same `name` is ignored).
    /// `pending` says cheaply how much work is waiting (`0` for none): the runtimes read it through
    /// `stats_json`'s `background.pending` to decide whether to ask the OS for a window. `run`
    /// starts one run of the task (see [`BackgroundTask::run`]).
    pub fn add_background_task(
        &self,
        name: &'static str,
        pending: impl Fn(&Ctx) -> u32 + Send + Sync + 'static,
        run: impl Fn(&Ctx, Deadline) -> BackgroundFuture + Send + Sync + 'static,
    ) {
        self.background.add(name, Arc::new(pending), Arc::new(run));
    }

    /// The work the background tasks say is waiting.
    #[must_use]
    pub fn background_pending(&self) -> u32 {
        self.background.pending(&self.ctx())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRuntime;
    use std::sync::atomic::AtomicU64;

    fn fixed_clock() -> (Arc<AtomicU64>, Arc<dyn Fn() -> u64 + Send + Sync>) {
        let time = Arc::new(AtomicU64::new(0));
        let reader = time.clone();
        (time, Arc::new(move || reader.load(Ordering::SeqCst)))
    }

    #[test]
    fn a_window_counts_down_by_the_clock_it_is_given() {
        let (time, now) = fixed_clock();
        let deadline = Deadline::new(now, Duration::from_millis(100));
        assert_eq!(deadline.remaining(), Duration::from_millis(100));
        time.store(60_000_000, Ordering::SeqCst);
        assert_eq!(deadline.remaining(), Duration::from_millis(40));
        assert!(!deadline.expired());
        time.store(500_000_000, Ordering::SeqCst);
        assert!(deadline.expired());
        deadline.note_replayed(2);
        deadline.note_refetched(1);
        assert_eq!(deadline.window.replayed.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn tasks_are_unique_by_name() {
        let t = TestRuntime::new();
        let rt = t.runtime();
        let ran = |_: &Ctx, _: Deadline| -> BackgroundFuture {
            Box::pin(async { BackgroundOutcome::Done })
        };
        rt.add_background_task("a", |_| 3, ran);
        rt.add_background_task("a", |_| 100, ran);
        rt.add_background_task("b", |_| 4, ran);
        assert_eq!(rt.background.count(), 2);
        assert_eq!(rt.background_pending(), 7);
    }
}
