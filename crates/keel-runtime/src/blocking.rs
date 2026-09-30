//! The blocking pool (SPEC 5.1): `spawn_blocking(f)` runs `f` off the core lock and resumes
//! the awaiting task through the executor.
//!
//! * **Native**: up to `min(4, cores)` worker threads (or `RuntimeConfig::blocking_threads`),
//!   started on demand, named `keel-blocking-N`, alive until shutdown.
//! * **wasm and test runtimes**: there is no thread to run on, so `f` runs **inline,
//!   synchronously, inside the `spawn_blocking` call**, and the returned future is already
//!   complete. Code that is correct on wasm therefore never relies on the pool for
//!   concurrency.
//!
//! The closure runs with the runtime installed as current on its thread, so `Ctx::current()`
//! works, but *without the core lock*: it must not write signals or call the host entry
//! points. A panic in the closure is caught, and re-raised in the task that awaits the
//! result, where the ordinary panic guard reports it (status 2 for a call).

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::guard::{self, CarriedPanic, PanicReport};

type Job = Box<dyn FnOnce() + Send + 'static>;

struct SlotInner<T> {
    result: Option<Result<T, PanicReport>>,
    waker: Option<Waker>,
}

struct Slot<T> {
    inner: Mutex<SlotInner<T>>,
}

impl<T> Slot<T> {
    fn complete(&self, result: Result<T, PanicReport>) {
        let waker = {
            let mut inner = self.inner.lock();
            inner.result = Some(result);
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The result of [`Ctx::spawn_blocking`](crate::Ctx::spawn_blocking): resolves to the
/// closure's return value. Dropping it does not stop the closure; its result is discarded.
#[must_use = "a BlockingTask does nothing unless awaited"]
pub struct BlockingTask<T> {
    slot: Arc<Slot<T>>,
}

impl<T> Future for BlockingTask<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut inner = self.slot.inner.lock();
        match inner.result.take() {
            Some(Ok(value)) => Poll::Ready(value),
            Some(Err(report)) => {
                drop(inner);
                std::panic::resume_unwind(Box::new(CarriedPanic(report)))
            }
            None => {
                match &inner.waker {
                    Some(w) if w.will_wake(cx.waker()) => {}
                    _ => inner.waker = Some(cx.waker().clone()),
                }
                Poll::Pending
            }
        }
    }
}

/// Wraps `f` into a job that stores its outcome in a fresh slot.
fn make_job<T: Send + 'static>(
    ctx: crate::Ctx,
    f: impl FnOnce() -> T + Send + 'static,
) -> (Job, Arc<Slot<T>>) {
    let slot = Arc::new(Slot {
        inner: Mutex::new(SlotInner {
            result: None,
            waker: None,
        }),
    });
    let out = slot.clone();
    let job: Job = Box::new(move || {
        let _scope = ctx.enter();
        out.complete(guard::guarded(f));
    });
    (job, slot)
}

#[cfg(not(target_family = "wasm"))]
mod native {
    use super::*;
    use parking_lot::Condvar;
    use std::collections::VecDeque;
    use std::thread::JoinHandle;

    struct Queue {
        jobs: VecDeque<Job>,
        idle: usize,
        spawned: usize,
        shutdown: bool,
    }

    struct Shared {
        queue: Mutex<Queue>,
        cv: Condvar,
    }

    pub(crate) struct Pool {
        shared: Arc<Shared>,
        max: usize,
        workers: Mutex<Vec<JoinHandle<()>>>,
    }

    impl Pool {
        pub(crate) fn new(max: usize) -> Pool {
            Pool {
                shared: Arc::new(Shared {
                    queue: Mutex::new(Queue {
                        jobs: VecDeque::new(),
                        idle: 0,
                        spawned: 0,
                        shutdown: false,
                    }),
                    cv: Condvar::new(),
                }),
                max: max.max(1),
                workers: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn max_threads(&self) -> usize {
            self.max
        }

        pub(crate) fn spawned_threads(&self) -> usize {
            self.shared.queue.lock().spawned
        }

        /// Workers currently waiting for a job.
        #[cfg(test)]
        pub(crate) fn shared_idle(&self) -> usize {
            self.shared.queue.lock().idle
        }

        /// Queues `job`; starts a worker if none is idle and the pool is below its maximum.
        /// Returns `false` (dropping the job) after shutdown.
        pub(crate) fn submit(&self, job: Job) -> bool {
            let start_worker = {
                let mut q = self.shared.queue.lock();
                if q.shutdown {
                    return false;
                }
                q.jobs.push_back(job);
                // Start a worker when more jobs are queued than idle workers can take. (An
                // idle worker that has been notified but has not yet woken still counts as
                // idle, so "no idle workers" alone would leave a second job waiting behind
                // the first.)
                if q.jobs.len() > q.idle && q.spawned < self.max {
                    q.spawned += 1;
                    Some(q.spawned)
                } else {
                    None
                }
            };
            match start_worker {
                Some(n) => {
                    let shared = self.shared.clone();
                    let spawned = std::thread::Builder::new()
                        .name(format!("keel-blocking-{n}"))
                        .spawn(move || worker(&shared));
                    match spawned {
                        Ok(handle) => self.workers.lock().push(handle),
                        Err(_) => {
                            // Could not start a thread. If it was the only one, queued jobs
                            // would never run, so run them here rather than lose them.
                            let orphaned: Vec<Job> = {
                                let mut q = self.shared.queue.lock();
                                q.spawned -= 1;
                                if q.spawned == 0 {
                                    q.jobs.drain(..).collect()
                                } else {
                                    Vec::new()
                                }
                            };
                            for job in orphaned {
                                job();
                            }
                            self.shared.cv.notify_one();
                        }
                    }
                }
                None => {
                    self.shared.cv.notify_one();
                }
            }
            true
        }

        /// Stops accepting jobs, lets workers finish what they are running and joins them.
        pub(crate) fn shutdown(&self) {
            {
                let mut q = self.shared.queue.lock();
                q.shutdown = true;
                q.jobs.clear();
            }
            self.shared.cv.notify_all();
            let workers: Vec<_> = self.workers.lock().drain(..).collect();
            let me = std::thread::current().id();
            for handle in workers {
                if handle.thread().id() != me {
                    let _ = handle.join();
                }
            }
        }
    }

    thread_local! {
        /// Set for the whole life of a pool worker thread.
        static WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Whether the calling thread is a blocking-pool worker.
    pub(super) fn on_worker_thread() -> bool {
        WORKER.try_with(std::cell::Cell::get).unwrap_or(false)
    }

    fn worker(shared: &Shared) {
        let _ = WORKER.try_with(|w| w.set(true));
        loop {
            let job = {
                let mut q = shared.queue.lock();
                loop {
                    if q.shutdown {
                        return;
                    }
                    if let Some(job) = q.jobs.pop_front() {
                        break job;
                    }
                    q.idle += 1;
                    shared.cv.wait(&mut q);
                    q.idle -= 1;
                }
            };
            job();
        }
    }
}

/// Whether the calling thread is a blocking-pool worker: it runs user closures without the core
/// lock, so it must not write signals (the runtime's write-context check refuses it).
pub(crate) fn on_worker_thread() -> bool {
    #[cfg(not(target_family = "wasm"))]
    {
        native::on_worker_thread()
    }
    #[cfg(target_family = "wasm")]
    {
        false
    }
}

/// Where blocking closures run.
pub(crate) enum Blocking {
    /// Run inline at the `spawn` call (wasm, test runtimes).
    Inline,
    /// A pool of worker threads.
    #[cfg(not(target_family = "wasm"))]
    Pool(native::Pool),
}

impl Blocking {
    /// The pool for a native runtime (`max` threads), or the inline runner.
    pub(crate) fn threaded(max: usize) -> Blocking {
        #[cfg(not(target_family = "wasm"))]
        {
            Blocking::Pool(native::Pool::new(max))
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = max;
            Blocking::Inline
        }
    }

    /// Runs `f` (see the module documentation for where).
    pub(crate) fn spawn<T: Send + 'static>(
        &self,
        ctx: crate::Ctx,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> BlockingTask<T> {
        let (job, slot) = make_job(ctx, f);
        match self {
            Blocking::Inline => job(),
            #[cfg(not(target_family = "wasm"))]
            Blocking::Pool(pool) => {
                // After shutdown the job is dropped and the task never completes; the
                // awaiting task is being torn down anyway.
                let _accepted = pool.submit(job);
            }
        }
        BlockingTask { slot }
    }

    pub(crate) fn shutdown(&self) {
        match self {
            Blocking::Inline => {}
            #[cfg(not(target_family = "wasm"))]
            Blocking::Pool(pool) => pool.shutdown(),
        }
    }

    /// `(worker threads started, maximum)`; `(0, 0)` for the inline runner.
    pub(crate) fn threads(&self) -> (usize, usize) {
        match self {
            Blocking::Inline => (0, 0),
            #[cfg(not(target_family = "wasm"))]
            Blocking::Pool(pool) => (pool.spawned_threads(), pool.max_threads()),
        }
    }
}

/// The default pool size: `min(4, available cores)`.
pub(crate) fn default_pool_size() -> usize {
    #[cfg(not(target_family = "wasm"))]
    {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(4)
    }
    #[cfg(target_family = "wasm")]
    {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRuntime;
    #[cfg(not(target_family = "wasm"))]
    use parking_lot::Condvar;

    #[test]
    fn inline_runner_completes_before_returning() {
        let t = TestRuntime::new();
        let task = t.ctx().spawn_blocking(|| 21 * 2);
        assert_eq!(t.run_until(task), 42);
    }

    #[test]
    fn inline_runner_reraises_a_panic_in_the_awaiting_task() {
        let t = TestRuntime::new();
        let task = t.ctx().spawn_blocking(|| -> u8 { panic!("boom in pool") });
        let report = guard::guarded(|| t.run_until(task)).unwrap_err();
        assert_eq!(report.message, "boom in pool");
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn pool_runs_jobs_on_named_worker_threads_and_wakes_the_waiter() {
        let pool = native::Pool::new(2);
        let (tx, rx) = std::sync::mpsc::channel();
        for i in 0..6 {
            let tx = tx.clone();
            assert!(pool.submit(Box::new(move || {
                let name = std::thread::current().name().unwrap_or("").to_owned();
                tx.send((i, name)).unwrap();
            })));
        }
        let mut seen: Vec<_> = (0..6).map(|_| rx.recv().unwrap()).collect();
        seen.sort();
        assert_eq!(
            seen.iter().map(|s| s.0).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        assert!(
            seen.iter().all(|s| s.1.starts_with("keel-blocking-")),
            "{seen:?}"
        );
        assert!(pool.spawned_threads() <= 2);
        pool.shutdown();
        assert!(!pool.submit(Box::new(|| {})), "no jobs after shutdown");
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn two_jobs_submitted_back_to_back_to_an_idle_pool_run_concurrently() {
        // One idle worker exists; two jobs that need each other (a barrier) arrive together.
        // If the second waited behind the first the barrier could never be crossed.
        for _ in 0..50 {
            let pool = native::Pool::new(2);
            let (warm_tx, warm_rx) = std::sync::mpsc::channel();
            pool.submit(Box::new(move || warm_tx.send(()).unwrap()));
            warm_rx.recv().unwrap();
            while pool.shared_idle() == 0 {
                std::thread::yield_now();
            }
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let (tx, rx) = std::sync::mpsc::channel();
            for _ in 0..2 {
                let (barrier, tx) = (barrier.clone(), tx.clone());
                pool.submit(Box::new(move || {
                    barrier.wait();
                    tx.send(()).unwrap();
                }));
            }
            for _ in 0..2 {
                rx.recv_timeout(std::time::Duration::from_secs(20))
                    .expect("both jobs ran at the same time");
            }
            pool.shutdown();
        }
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn pool_size_is_bounded_by_max() {
        let pool = native::Pool::new(3);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for _ in 0..8 {
            let (gate, started) = (gate.clone(), started.clone());
            pool.submit(Box::new(move || {
                started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut open = gate.0.lock();
                while !*open {
                    gate.1.wait(&mut open);
                }
            }));
        }
        // Exactly `max` jobs can run at once.
        for _ in 0..200 {
            if started.load(std::sync::atomic::Ordering::SeqCst) == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert_eq!(pool.spawned_threads(), 3);
        *gate.0.lock() = true;
        gate.1.notify_all();
        // Shutdown drops queued jobs but lets running ones finish.
        pool.shutdown();
    }
}
