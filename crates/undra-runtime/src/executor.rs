//! The executor (SPEC 5.2): a slab of tasks, wakers that push task ids onto a ready queue,
//! and the free functions user code calls from inside a dispatched call or a task.
//!
//! # Model
//!
//! Each in-flight async call is one task; [`spawn`] adds detached ones. A task is a
//! `Pin<Box<dyn Future<Output = ()> + Send>>` in a [`slab::Slab`]. Its [`Waker`] is an
//! `Arc<TaskWaker>` (std's `Wake` trait) that pushes the task id onto an MPSC ready queue,
//! deduplicated by a per-task `queued` flag, and nudges whoever runs the loop:
//!
//! * native: a `Condvar` wakes the `undra-core` thread;
//! * wasm and manually driven runtimes: [`Host::schedule`], at most
//!   once per turn.
//!
//! Waking never takes the core lock, so any thread (a blocking-pool worker, the timer thread,
//! a host thread delivering a port reply) can wake a task while the core thread is busy.
//!
//! A turn takes at most [`BATCH`] ready ids, locks the core, polls each, and unlocks fairly so
//! a waiting host call gets in. A task's future is taken out of the slab while it is polled,
//! so a task can spawn, cancel or wake others without touching a lock that is held.
//!
//! # Cancellation
//!
//! [`cancel`] on an idle task removes it and drops its future on the spot. On a task that is
//! being polled right now (only possible from inside that same poll) it sets a flag, and the
//! future is dropped as soon as the poll returns. A cancelled task is never polled again.
//! Futures are always dropped under the panic guard, on a thread that holds the core lock
//! when the cancel came from a host call.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::task::Wake;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use slab::Slab;
use undra_wire::Handle;

use crate::ctx::Ctx;
use crate::host::Host;

pub use crate::blocking::BlockingTask;
pub use crate::timer::Sleep;

/// Maximum number of polls in one turn of the core loop (SPEC 5.1).
pub const BATCH: usize = 64;

/// Identifies a spawned task. Ids are never reused while a stale copy could still refer to
/// the slot (a generation is embedded), so cancelling a finished task is a harmless no-op.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskId {
    key: u32,
    serial: u32,
}

impl TaskId {
    /// An id that names no task, ever: what `Ctx::spawn` returns after shutdown. Cancelling it
    /// is a no-op.
    pub(crate) const fn dead() -> TaskId {
        TaskId {
            key: u32::MAX,
            serial: 0,
        }
    }

    /// The id as a single number, for logs.
    pub fn as_u64(self) -> u64 {
        (u64::from(self.serial) << 32) | u64::from(self.key)
    }
}

pub(crate) type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// What a task is for; decides how its panic is reported.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TaskKind {
    /// Spawned through `Ctx::spawn`; a panic is only logged.
    Detached,
    /// An async call; a panic becomes a status 2 reply.
    Call { call_id: u32, handle: Handle },
    /// A stream driver; a panic becomes a stream error item.
    Stream { call_id: u32, handle: Handle },
}

struct TaskWaker {
    id: TaskId,
    queued: AtomicBool,
    shared: Weak<Shared>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if !self.queued.swap(true, Ordering::AcqRel) {
            if let Some(shared) = self.shared.upgrade() {
                shared.push_ready(self.id);
            }
        }
    }
}

enum TaskState {
    Idle(BoxFuture),
    Polling { cancel: bool },
}

struct TaskEntry {
    serial: u32,
    state: TaskState,
    wake: Arc<TaskWaker>,
    waker: Waker,
    kind: TaskKind,
}

struct Queue {
    ready: VecDeque<TaskId>,
    shutdown: bool,
    /// Inline mode: a `Host::schedule` request is outstanding.
    scheduled: bool,
}

pub(crate) struct Shared {
    tasks: Mutex<Slab<TaskEntry>>,
    next_serial: AtomicU32,
    queue: Mutex<Queue>,
    cv: Condvar,
    inline: bool,
    /// Set by [`Executor::shutdown`]: no task is accepted from then on. Checked under the
    /// `tasks` lock, the same lock [`Executor::clear`] drains under, so a spawn that races the
    /// teardown either lands before the drain (and is dropped by it) or is refused.
    closed: AtomicBool,
    /// The owning runtime's id, to mark `Host::schedule` as a host callback.
    runtime_id: u64,
    host: Arc<dyn Host>,
    live: AtomicUsize,
}

impl Shared {
    /// Asks the host to poll soon. A panicking host is ignored: wakers run in arbitrary
    /// contexts and must not unwind.
    fn schedule_host(&self) {
        if let Err(report) = crate::guard::guarded(|| {
            let _call = crate::runtime::HostCall::enter(self.runtime_id);
            self.host.schedule();
        }) {
            crate::runtime::report_current("Host::schedule panicked", "Host::schedule", &report);
        }
    }

    fn push_ready(&self, id: TaskId) {
        let schedule = {
            let mut q = self.queue.lock();
            q.ready.push_back(id);
            let first = !q.scheduled;
            q.scheduled = true;
            first
        };
        if self.inline {
            if schedule {
                self.schedule_host();
            }
        } else {
            self.cv.notify_one();
        }
    }

    /// Blocks until work is ready; `None` once shut down. Native core thread only.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn wait_batch(&self, max: usize) -> Option<Vec<TaskId>> {
        let mut q = self.queue.lock();
        loop {
            if q.shutdown {
                return None;
            }
            if !q.ready.is_empty() {
                let n = q.ready.len().min(max);
                let batch: Vec<TaskId> = q.ready.drain(..n).collect();
                q.scheduled = false;
                return Some(batch);
            }
            self.cv.wait(&mut q);
        }
    }
}

/// The task slab and ready queue.
pub(crate) struct Executor {
    shared: Arc<Shared>,
}

/// What [`Executor::end_poll`] decided.
pub(crate) enum EndPoll {
    /// The task is parked, waiting to be woken.
    Parked,
    /// The task finished or was cancelled; the future must be dropped by the caller.
    Gone(Option<BoxFuture>),
}

/// What [`Executor::cancel`] did.
pub(crate) enum CancelOutcome {
    /// No such task (finished, already cancelled, or a stale id).
    NotFound,
    /// The task was idle; its future is returned for the caller to drop under the guard.
    Dropped(BoxFuture),
    /// The task is being polled; it will be dropped when the poll returns.
    Deferred,
}

impl Executor {
    /// `inline`: no core thread; wakes request `Host::schedule` instead of notifying a condvar.
    pub(crate) fn new(runtime_id: u64, host: Arc<dyn Host>, inline: bool) -> Executor {
        Executor {
            shared: Arc::new(Shared {
                tasks: Mutex::new(Slab::new()),
                next_serial: AtomicU32::new(1),
                queue: Mutex::new(Queue {
                    ready: VecDeque::new(),
                    shutdown: false,
                    scheduled: false,
                }),
                cv: Condvar::new(),
                inline,
                closed: AtomicBool::new(false),
                runtime_id,
                host,
                live: AtomicUsize::new(0),
            }),
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    /// Adds a task and marks it ready for its first poll. After [`shutdown`](Executor::shutdown)
    /// the task is refused and its future is handed back for the caller to drop.
    pub(crate) fn try_spawn(&self, future: BoxFuture, kind: TaskKind) -> Result<TaskId, BoxFuture> {
        let serial = self.shared.next_serial.fetch_add(1, Ordering::Relaxed);
        let id = {
            let mut tasks = self.shared.tasks.lock();
            if self.shared.closed.load(Ordering::SeqCst) {
                return Err(future);
            }
            let vacant = tasks.vacant_entry();
            let id = TaskId {
                key: u32::try_from(vacant.key()).unwrap_or(u32::MAX),
                serial,
            };
            let wake = Arc::new(TaskWaker {
                id,
                // Queued from birth: `push_ready` below is its first entry.
                queued: AtomicBool::new(true),
                shared: Arc::downgrade(&self.shared),
            });
            let waker = Waker::from(wake.clone());
            vacant.insert(TaskEntry {
                serial,
                state: TaskState::Idle(future),
                wake,
                waker,
                kind,
            });
            id
        };
        self.shared.live.fetch_add(1, Ordering::Relaxed);
        self.shared.push_ready(id);
        Ok(id)
    }

    /// [`try_spawn`](Executor::try_spawn) for tests, which never shut the executor down first.
    #[cfg(test)]
    pub(crate) fn spawn(&self, future: BoxFuture, kind: TaskKind) -> TaskId {
        match self.try_spawn(future, kind) {
            Ok(id) => id,
            Err(_) => panic!("the executor is shut down"),
        }
    }

    /// Takes up to `max` ready ids without blocking.
    pub(crate) fn take_ready(&self, max: usize) -> Vec<TaskId> {
        let mut q = self.shared.queue.lock();
        let n = q.ready.len().min(max);
        let batch: Vec<TaskId> = q.ready.drain(..n).collect();
        // The outstanding `Host::schedule` request is being served; a later wake asks again.
        q.scheduled = false;
        batch
    }

    /// After a bounded turn: if tasks are still ready, ask for another one (inline mode asks
    /// the host, threaded mode nudges the core thread).
    pub(crate) fn reschedule_if_ready(&self) {
        let schedule = {
            let mut q = self.shared.queue.lock();
            if q.ready.is_empty() {
                false
            } else {
                let first = !q.scheduled;
                q.scheduled = true;
                first
            }
        };
        if schedule {
            if self.shared.inline {
                self.shared.schedule_host();
            } else {
                self.shared.cv.notify_one();
            }
        }
    }

    /// Puts ids back at the front of the queue (a turn could not run).
    pub(crate) fn requeue(&self, ids: Vec<TaskId>) {
        if ids.is_empty() {
            return;
        }
        {
            let mut q = self.shared.queue.lock();
            for id in ids.into_iter().rev() {
                q.ready.push_front(id);
            }
            q.scheduled = true;
        }
        if self.shared.inline {
            self.shared.schedule_host();
        } else {
            self.shared.cv.notify_one();
        }
    }

    /// Takes the future out of task `id` to poll it. `None` if the task is gone, cancelled or
    /// already being polled.
    pub(crate) fn begin_poll(&self, id: TaskId) -> Option<(BoxFuture, Waker, TaskKind)> {
        let mut tasks = self.shared.tasks.lock();
        let entry = tasks.get_mut(id.key as usize)?;
        if entry.serial != id.serial {
            return None;
        }
        match std::mem::replace(&mut entry.state, TaskState::Polling { cancel: false }) {
            TaskState::Idle(future) => {
                // A wake that arrives from here on must queue the task again.
                entry.wake.queued.store(false, Ordering::Release);
                Some((future, entry.waker.clone(), entry.kind))
            }
            polling @ TaskState::Polling { .. } => {
                entry.state = polling;
                None
            }
        }
    }

    /// Records the outcome of a poll. `future` is the (still pending) future, or `None` when
    /// the task completed.
    pub(crate) fn end_poll(&self, id: TaskId, future: Option<BoxFuture>) -> EndPoll {
        let removed;
        {
            let mut tasks = self.shared.tasks.lock();
            let Some(entry) = tasks.get_mut(id.key as usize) else {
                return EndPoll::Gone(future);
            };
            if entry.serial != id.serial {
                return EndPoll::Gone(future);
            }
            let cancelled = matches!(entry.state, TaskState::Polling { cancel: true });
            match future {
                Some(future) if !cancelled => {
                    entry.state = TaskState::Idle(future);
                    return EndPoll::Parked;
                }
                other => {
                    removed = (tasks.remove(id.key as usize), other);
                }
            }
        }
        self.shared.live.fetch_sub(1, Ordering::Relaxed);
        // Dropped here, after the tasks lock is released.
        drop(removed.0);
        EndPoll::Gone(removed.1)
    }

    /// Cancels task `id` (see the module documentation).
    pub(crate) fn cancel(&self, id: TaskId) -> CancelOutcome {
        let removed;
        {
            let mut tasks = self.shared.tasks.lock();
            let Some(entry) = tasks.get_mut(id.key as usize) else {
                return CancelOutcome::NotFound;
            };
            if entry.serial != id.serial {
                return CancelOutcome::NotFound;
            }
            match &mut entry.state {
                TaskState::Polling { cancel } => {
                    *cancel = true;
                    return CancelOutcome::Deferred;
                }
                TaskState::Idle(_) => removed = tasks.remove(id.key as usize),
            }
        }
        self.shared.live.fetch_sub(1, Ordering::Relaxed);
        match removed.state {
            TaskState::Idle(future) => CancelOutcome::Dropped(future),
            TaskState::Polling { .. } => CancelOutcome::NotFound,
        }
    }

    /// Removes every task and returns the idle futures, for the caller to drop under the guard.
    pub(crate) fn clear(&self) -> Vec<BoxFuture> {
        let drained: Vec<TaskEntry> = {
            let mut tasks = self.shared.tasks.lock();
            tasks.drain().collect()
        };
        self.shared.live.store(0, Ordering::Relaxed);
        drained
            .into_iter()
            .filter_map(|e| match e.state {
                TaskState::Idle(future) => Some(future),
                TaskState::Polling { .. } => None,
            })
            .collect()
    }

    /// Number of live tasks.
    pub(crate) fn live(&self) -> usize {
        self.shared.live.load(Ordering::Relaxed)
    }

    /// Wakes the core for a turn with nothing to poll (a dead id in the ready queue), so that
    /// work queued for it (deferred drops) is not left waiting for unrelated tasks.
    pub(crate) fn nudge(&self) {
        self.shared.push_ready(TaskId::dead());
    }

    /// Stops `wait_batch` (the core thread exits) and refuses every task spawned from now on.
    pub(crate) fn shutdown(&self) {
        {
            // Under the `tasks` lock so that the flag is ordered against `try_spawn`'s check.
            let _tasks = self.shared.tasks.lock();
            self.shared.closed.store(true, Ordering::SeqCst);
        }
        self.shared.queue.lock().shutdown = true;
        self.shared.cv.notify_all();
    }
}

// ----- Notify ----------------------------------------------------------------------------

#[derive(Default)]
struct NotifyInner {
    permit: bool,
    waker: Option<Waker>,
}

/// A single-waiter wake-up primitive: [`notify_one`](Notify::notify_one) wakes the task
/// waiting in [`notified`](Notify::notified), or, if nobody waits yet, stores a permit so the
/// next wait completes immediately (no wake-up is lost).
///
/// Stream credit uses it: a stream task with no credit parks on a `Notify` until
/// `stream_credit` arrives.
///
/// ```
/// use undra_runtime::executor::Notify;
/// use undra_runtime::testing::TestRuntime;
///
/// let t = TestRuntime::new();
/// let n = Notify::new();
/// n.notify_one(); // permit stored
/// t.run_until(n.notified()); // completes at once
/// ```
#[derive(Default)]
pub struct Notify {
    inner: Mutex<NotifyInner>,
}

impl Notify {
    /// Creates a `Notify` with no permit.
    pub const fn new() -> Notify {
        Notify {
            inner: Mutex::new(NotifyInner {
                permit: false,
                waker: None,
            }),
        }
    }

    /// Stores one permit and wakes the waiting task, if any. The woken task consumes the
    /// permit when it polls [`notified`](Notify::notified) again, so a notification is never
    /// lost, whether it arrives before or after the wait begins.
    pub fn notify_one(&self) {
        let waker = {
            let mut inner = self.inner.lock();
            inner.permit = true;
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// A future that completes when notified (consuming a stored permit if there is one).
    pub fn notified(&self) -> Notified<'_> {
        Notified { notify: self }
    }
}

/// The future returned by [`Notify::notified`].
#[must_use = "futures do nothing unless awaited"]
pub struct Notified<'a> {
    notify: &'a Notify,
}

impl Future for Notified<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut inner = self.notify.inner.lock();
        if inner.permit {
            inner.permit = false;
            return Poll::Ready(());
        }
        match &inner.waker {
            Some(w) if w.will_wake(cx.waker()) => {}
            _ => inner.waker = Some(cx.waker().clone()),
        }
        Poll::Pending
    }
}

// ----- free functions --------------------------------------------------------------------

/// Spawns a detached task on the current runtime. See [`Ctx::spawn`].
///
/// # Panics
///
/// Panics if called outside a dispatched call, a task or a [`Ctx::enter`] scope.
pub fn spawn(future: impl Future<Output = ()> + Send + 'static) -> TaskId {
    Ctx::current().spawn(future)
}

/// Runs `f` on the blocking pool of the current runtime. See [`Ctx::spawn_blocking`].
///
/// # Panics
///
/// Panics if there is no current runtime.
pub fn spawn_blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> BlockingTask<T> {
    Ctx::current().spawn_blocking(f)
}

/// Completes after `duration`, through the Timer binding. See [`Ctx::sleep`].
///
/// # Panics
///
/// Panics if there is no current runtime.
pub fn sleep(duration: Duration) -> Sleep {
    Ctx::current().sleep(duration)
}

/// Cancels a task spawned with [`spawn`]. See [`Ctx::cancel_task`].
///
/// # Panics
///
/// Panics if there is no current runtime.
pub fn cancel(id: TaskId) {
    Ctx::current().cancel_task(id);
}

/// Yields to the executor once: the task is polled again after the other ready tasks.
pub fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}

/// The future returned by [`yield_now`].
#[must_use = "futures do nothing unless awaited"]
pub struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct NullHost {
        schedules: AtomicUsize,
    }
    impl Host for NullHost {
        fn reply(&self, _: u32, _: &[u8]) {}
        fn change_set(&self, _: &[u8]) {}
        fn stream_item(&self, _: u32, _: &[u8]) {}
        fn port_call(&self, _: u32, _: u32, _: u32, _: &[u8]) -> crate::PortCallOutcome {
            crate::PortCallOutcome::Unavailable
        }
        fn log(&self, _: u8, _: &str, _: &str) {}
        fn schedule(&self) {
            self.schedules.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn inline_executor() -> (Executor, Arc<NullHost>) {
        let host = Arc::new(NullHost {
            schedules: AtomicUsize::new(0),
        });
        (Executor::new(0, host.clone(), true), host)
    }

    /// Polls every ready task once, like a turn without the core lock.
    fn turn(exec: &Executor) -> usize {
        let mut polled = 0;
        for id in exec.take_ready(BATCH) {
            let Some((mut fut, waker, _)) = exec.begin_poll(id) else {
                continue;
            };
            polled += 1;
            let mut cx = Context::from_waker(&waker);
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(()) => {
                    exec.end_poll(id, None);
                }
                Poll::Pending => {
                    if let EndPoll::Gone(Some(f)) = exec.end_poll(id, Some(fut)) {
                        drop(f);
                    }
                }
            }
        }
        polled
    }

    #[test]
    fn spawned_task_runs_once_and_is_removed() {
        let (exec, _) = inline_executor();
        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();
        exec.spawn(
            Box::pin(async move {
                r.fetch_add(1, Ordering::SeqCst);
            }),
            TaskKind::Detached,
        );
        assert_eq!(exec.live(), 1);
        assert_eq!(turn(&exec), 1);
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert_eq!(exec.live(), 0);
        assert_eq!(turn(&exec), 0);
    }

    #[test]
    fn wakes_are_deduplicated_and_request_one_schedule_per_turn() {
        let (exec, host) = inline_executor();
        let notify = Arc::new(Notify::new());
        let n = notify.clone();
        exec.spawn(
            Box::pin(async move {
                n.notified().await;
                n.notified().await;
            }),
            TaskKind::Detached,
        );
        assert_eq!(
            host.schedules.load(Ordering::SeqCst),
            1,
            "spawn schedules once"
        );
        assert_eq!(turn(&exec), 1); // parks on the first notified()
        // Many wakes before the next turn: one queue entry, one schedule request.
        notify.notify_one();
        notify.notify_one();
        notify.notify_one();
        assert_eq!(host.schedules.load(Ordering::SeqCst), 2);
        assert_eq!(exec.take_ready(BATCH).len(), 1);
    }

    #[test]
    fn a_wake_during_a_poll_queues_the_task_again() {
        let (exec, _) = inline_executor();
        exec.spawn(
            Box::pin(async {
                yield_now().await;
                yield_now().await;
            }),
            TaskKind::Detached,
        );
        assert_eq!(turn(&exec), 1);
        assert_eq!(exec.live(), 1);
        assert_eq!(turn(&exec), 1);
        assert_eq!(turn(&exec), 1);
        assert_eq!(exec.live(), 0);
    }

    #[test]
    fn cancel_drops_an_idle_task_and_ignores_stale_ids() {
        let (exec, _) = inline_executor();
        struct Flag(Arc<AtomicBool>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = Flag(dropped.clone());
        let id = exec.spawn(
            Box::pin(async move {
                let _keep = flag;
                std::future::pending::<()>().await;
            }),
            TaskKind::Detached,
        );
        turn(&exec);
        assert!(!dropped.load(Ordering::SeqCst));
        match exec.cancel(id) {
            CancelOutcome::Dropped(f) => drop(f),
            _ => panic!("idle task should be dropped"),
        }
        assert!(dropped.load(Ordering::SeqCst));
        assert!(matches!(exec.cancel(id), CancelOutcome::NotFound));
        assert!(exec.begin_poll(id).is_none());
        assert_eq!(exec.live(), 0);
    }

    #[test]
    fn stale_id_does_not_touch_a_task_that_reuses_the_slot() {
        let (exec, _) = inline_executor();
        let first = exec.spawn(Box::pin(async {}), TaskKind::Detached);
        turn(&exec);
        let second = exec.spawn(Box::pin(std::future::pending::<()>()), TaskKind::Detached);
        assert_eq!(first.key, second.key, "the slot is reused");
        assert_ne!(first, second);
        assert!(matches!(exec.cancel(first), CancelOutcome::NotFound));
        assert_eq!(exec.live(), 1);
    }

    #[test]
    fn cancel_during_a_poll_is_deferred_until_the_poll_returns() {
        let (exec, _) = inline_executor();
        let id = exec.spawn(Box::pin(std::future::pending::<()>()), TaskKind::Detached);
        let (fut, _waker, _) = exec.begin_poll(id).unwrap();
        assert!(matches!(exec.cancel(id), CancelOutcome::Deferred));
        assert!(
            exec.begin_poll(id).is_none(),
            "a running task cannot be polled twice"
        );
        match exec.end_poll(id, Some(fut)) {
            EndPoll::Gone(Some(f)) => drop(f),
            _ => panic!("a cancelled task must not be parked"),
        }
        assert_eq!(exec.live(), 0);
    }

    #[test]
    fn clear_returns_the_idle_futures() {
        let (exec, _) = inline_executor();
        exec.spawn(Box::pin(std::future::pending::<()>()), TaskKind::Detached);
        exec.spawn(Box::pin(std::future::pending::<()>()), TaskKind::Detached);
        assert_eq!(exec.clear().len(), 2);
        assert_eq!(exec.live(), 0);
    }

    #[test]
    fn requeue_puts_ids_back_in_order() {
        let (exec, _) = inline_executor();
        let a = exec.spawn(Box::pin(async {}), TaskKind::Detached);
        let b = exec.spawn(Box::pin(async {}), TaskKind::Detached);
        let ids = exec.take_ready(BATCH);
        assert_eq!(ids, [a, b]);
        exec.requeue(ids);
        assert_eq!(exec.take_ready(BATCH), [a, b]);
    }

    #[test]
    fn take_ready_respects_the_batch_limit() {
        let (exec, _) = inline_executor();
        for _ in 0..(BATCH + 5) {
            exec.spawn(Box::pin(async {}), TaskKind::Detached);
        }
        assert_eq!(exec.take_ready(BATCH).len(), BATCH);
        assert_eq!(exec.take_ready(BATCH).len(), 5);
    }

    #[test]
    fn notify_stores_a_permit_and_wakes_a_waiter() {
        let (exec, _) = inline_executor();
        let notify = Arc::new(Notify::new());
        let done = Arc::new(AtomicBool::new(false));
        let (n, d) = (notify.clone(), done.clone());
        exec.spawn(
            Box::pin(async move {
                n.notified().await;
                d.store(true, Ordering::SeqCst);
            }),
            TaskKind::Detached,
        );
        turn(&exec);
        assert!(!done.load(Ordering::SeqCst));
        notify.notify_one();
        turn(&exec);
        assert!(done.load(Ordering::SeqCst));

        // Permit stored before anybody waits.
        let n2 = Arc::new(Notify::new());
        n2.notify_one();
        let done2 = Arc::new(AtomicBool::new(false));
        let (n, d) = (n2.clone(), done2.clone());
        exec.spawn(
            Box::pin(async move {
                n.notified().await;
                d.store(true, Ordering::SeqCst);
            }),
            TaskKind::Detached,
        );
        turn(&exec);
        assert!(done2.load(Ordering::SeqCst));
    }

    #[test]
    fn threaded_wait_batch_wakes_on_a_cross_thread_wake() {
        let host = Arc::new(NullHost {
            schedules: AtomicUsize::new(0),
        });
        let exec = Executor::new(0, host, false);
        let shared = exec.shared();
        let waiter = std::thread::spawn(move || shared.wait_batch(BATCH));
        std::thread::sleep(Duration::from_millis(20));
        let id = exec.spawn(Box::pin(async {}), TaskKind::Detached);
        assert_eq!(waiter.join().unwrap(), Some(vec![id]));
    }

    #[test]
    fn shutdown_releases_the_waiting_thread() {
        let host = Arc::new(NullHost {
            schedules: AtomicUsize::new(0),
        });
        let exec = Executor::new(0, host, false);
        let shared = exec.shared();
        let waiter = std::thread::spawn(move || shared.wait_batch(BATCH));
        std::thread::sleep(Duration::from_millis(20));
        exec.shutdown();
        assert_eq!(waiter.join().unwrap(), None);
    }
}
