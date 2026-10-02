//! Background runs: the OS grants a window, the core drains in it (ADR-046 decision 3).
//!
//! A *background task* is a unit of work worth finishing while the app is not on screen: replay the
//! offline queue, refetch stale queries, flush what is waiting to be persisted. Layered crates
//! register theirs on the runtime ([`Runtime::add_background_task`], what `undra-query` does when it
//! starts); an app registers its own the same way (from an [`InitHook`](crate::InitHook), or any
//! time it has a [`Ctx`]).
//!
//! **Linked by use** (ADR-052): the run machinery below, with the timer path it needs, is reachable
//! only through [`Runtime::add_background_task`], which is what installs it as the runtime's
//! runner. A core with no background task (a hello world) links none of it, and the standard
//! function answers an idle run (`finished`, nothing done).
//!
//! The runner ([`run`]) is what the standard function `run_background(deadline_ms)` of `undra-ports` calls: it
//! starts every task at once and returns when they are all done or when the window is nearly over
//! (`deadline - `[`MARGIN`], so the host still has time to tell the OS it finished), whichever comes
//! first. Work a task did stays done (the offline queue persists per item, ADR-037), so a run that
//! is cut short, by the deadline or by the host cancelling the call, loses nothing; the report says
//! how much is still pending.
//!
//! Time is the runtime's own (the monotonic clock its timers keep: the system's on native, the
//! test runtime's manual one in tests), and the window is enforced by a sleep on the `Timer` port,
//! so a fake clock decides what a deadline means in a test (R12). Nothing here calls the `Clock`
//! port: a core that never uses it does not link its proxy.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::ctx::Ctx;
use crate::runtime::Runtime;
use crate::stats::Stats;
use crate::timer::Timers;

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
    timers: Arc<Timers>,
    at_ns: u64,
    replayed: AtomicU32,
    refetched: AtomicU32,
}

impl Deadline {
    fn new(timers: Arc<Timers>, budget: Duration) -> Deadline {
        let at_ns = timers
            .now_ns()
            .saturating_add(u64::try_from(budget.as_nanos()).unwrap_or(u64::MAX));
        Deadline {
            window: Arc::new(Window {
                timers,
                at_ns,
                replayed: AtomicU32::new(0),
                refetched: AtomicU32::new(0),
            }),
        }
    }

    /// How long the run still has, by the runtime's monotonic clock.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        Duration::from_nanos(
            self.window
                .at_ns
                .saturating_sub(self.window.timers.now_ns()),
        )
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

type PendingFn = Arc<dyn Fn(&Ctx) -> u32 + Send + Sync>;
type RunFn = Arc<dyn Fn(&Ctx, Deadline) -> BackgroundFuture + Send + Sync>;

/// A task registered on one runtime.
#[derive(Clone)]
pub(crate) struct Registered {
    name: &'static str,
    pending: PendingFn,
    run: RunFn,
}

/// What runs the registered tasks (set when the first is registered).
pub type Runner =
    for<'a> fn(&'a Ctx, Duration) -> Pin<Box<dyn Future<Output = BackgroundTotals> + Send + 'a>>;

/// The tasks of one runtime, in registration order.
#[derive(Default)]
pub(crate) struct Registry {
    tasks: Mutex<Vec<Registered>>,
    runner: std::sync::OnceLock<Runner>,
}

impl Registry {
    /// Registers a task unless one of that name exists.
    pub(crate) fn add(&self, name: &'static str, pending: PendingFn, run: RunFn) {
        let _ = self.runner.set(run_boxed);
        let mut tasks = self.tasks.lock();
        if tasks.iter().all(|t| t.name != name) {
            tasks.push(Registered { name, pending, run });
        }
    }

    /// The registered tasks.
    fn all(&self) -> Vec<Registered> {
        self.tasks.lock().clone()
    }

    /// How many tasks there are.
    pub(crate) fn count(&self) -> usize {
        self.tasks.lock().len()
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

impl BackgroundTotals {
    /// A run of a runtime that has no background task: nothing to do, so it is finished.
    pub const IDLE: BackgroundTotals = BackgroundTotals {
        finished: true,
        replayed: 0,
        refetched: 0,
        still_pending: 0,
    };
}

/// Every task's future, polled together under the panic guard until each has an outcome. A task
/// that panics is contained, reported (operation `task`) and stays incomplete; the others go on.
struct Tasks {
    futures: Vec<Option<BackgroundFuture>>,
    outcomes: Vec<BackgroundOutcome>,
}

impl Future for Tasks {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        let mut pending = false;
        for (slot, outcome) in this.futures.iter_mut().zip(this.outcomes.iter_mut()) {
            let Some(future) = slot else { continue };
            match crate::guard::guarded(|| future.as_mut().poll(cx)) {
                Ok(Poll::Pending) => pending = true,
                Ok(Poll::Ready(done)) => {
                    *outcome = done;
                    *slot = None;
                }
                Err(report) => {
                    crate::runtime::report_current("a background task panicked", "task", &report);
                    *slot = None;
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

/// Runs every registered background task of `ctx`'s runtime inside `deadline` (less the
/// [`MARGIN`]).
///
/// Never panics: a task that panics is contained, reported like any panic (operation `task`) and
/// counts as incomplete.
pub async fn run(ctx: &Ctx, deadline: Duration) -> BackgroundTotals {
    let runtime: &Runtime = ctx.runtime();
    Stats::inc(&runtime.stats.background_runs);
    let budget = deadline.saturating_sub(MARGIN);
    let window = Deadline::new(runtime.timers.clone(), budget);
    let registered = runtime.background.all();
    // Built under the guard: a task whose `run` panics before it returns a future is incomplete,
    // like one that panics while it runs. Nothing is started when no time is left.
    let futures: Vec<Option<BackgroundFuture>> = if budget.is_zero() {
        Vec::new()
    } else {
        registered
            .iter()
            .map(|task| crate::guard::guarded(|| (task.run)(ctx, window.clone())).ok())
            .collect()
    };
    let mut tasks = Tasks {
        outcomes: vec![BackgroundOutcome::Incomplete; futures.len()],
        futures,
    };
    // The tasks race the window; losing drops them (dropping `tasks` is what cancels them, also
    // when the host cancels the call and this future is dropped).
    let mut sleep = core::pin::pin!(ctx.sleep(budget));
    let in_time = core::future::poll_fn(|cx| {
        if Pin::new(&mut tasks).poll(cx).is_ready() {
            return Poll::Ready(true);
        }
        if sleep.as_mut().poll(cx).is_ready() {
            return Poll::Ready(false);
        }
        Poll::Pending
    })
    .await;
    let all_done = in_time && tasks.outcomes.iter().all(|o| *o == BackgroundOutcome::Done);
    drop(tasks);
    let pending = runtime.background.pending(ctx);
    let totals = BackgroundTotals {
        finished: pending == 0 && (registered.is_empty() || (!budget.is_zero() && all_done)),
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

/// The `background` section of `stats_json`: how many tasks there are, how much work they say is
/// waiting (what a platform reads to decide whether to ask the OS for a window) and what the runs
/// did.
fn stats_section(runtime: &Runtime) -> Option<String> {
    let s = &runtime.stats;
    Some(format!(
        "{{\"tasks\":{},\"pending\":{},\"runs\":{},\"finished\":{},\"replayed\":{},\"refetched\":{}}}",
        runtime.background.count(),
        runtime.background.pending(&runtime.ctx()),
        Stats::get(&s.background_runs),
        Stats::get(&s.background_finished),
        Stats::get(&s.background_replayed),
        Stats::get(&s.background_refetched),
    ))
}

/// [`run`] as a [`Runner`].
fn run_boxed<'a>(
    ctx: &'a Ctx,
    deadline: Duration,
) -> Pin<Box<dyn Future<Output = BackgroundTotals> + Send + 'a>> {
    Box::pin(run(ctx, deadline))
}

impl Runtime {
    /// The runner of this runtime's background tasks, `None` while none is registered: what the
    /// standard function `run_background` runs.
    #[must_use]
    pub fn background_runner(&self) -> Option<Runner> {
        self.background.runner.get().copied()
    }

    /// Registers a background task on this runtime (a second of the same `name` is ignored).
    /// `pending` says cheaply how much work is waiting (`0` for none): the runtimes read it through
    /// `stats_json`'s `background.pending` to decide whether to ask the OS for a window. `run`
    /// starts one run of the task: it must hold the runtime weakly across awaits, as `ctx.downgrade()`
    /// (ADR-034), and stop when its [`Deadline`] is expired or its future is dropped.
    pub fn add_background_task(
        &self,
        name: &'static str,
        pending: impl Fn(&Ctx) -> u32 + Send + Sync + 'static,
        run: impl Fn(&Ctx, Deadline) -> BackgroundFuture + Send + Sync + 'static,
    ) {
        self.background.add(name, Arc::new(pending), Arc::new(run));
        self.add_stats_section(crate::StatsSection {
            name: "background",
            json: stats_section,
        });
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

    #[test]
    fn a_window_counts_down_by_the_runtimes_clock() {
        let t = TestRuntime::new();
        let timers = t.runtime().timers.clone();
        let deadline = Deadline::new(timers, Duration::from_millis(100));
        assert_eq!(deadline.remaining(), Duration::from_millis(100));
        t.advance(Duration::from_millis(60));
        assert_eq!(deadline.remaining(), Duration::from_millis(40));
        assert!(!deadline.expired());
        t.advance(Duration::from_millis(500));
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
