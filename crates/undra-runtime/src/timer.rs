//! Timers (SPEC 5.8): `sleep(Duration)` through the Timer binding.
//!
//! A [`Sleep`] registers a *sleeper* under a fresh `timer_id` and then arms a timer:
//!
//! 1. the host is asked first, with [`Host::timer_set(timer_id, delay_ms)`](crate::Host);
//!    if it returns `true` it owns the timer (wasm's `setTimeout`) and must later call
//!    [`Runtime::timer_fired`](crate::Runtime::timer_fired);
//! 2. otherwise the runtime uses its **internal timer**: a `BinaryHeap` of deadlines served by
//!    one `undra-timer` thread (a `Condvar` wait until the earliest deadline; native only,
//!    started on first use).
//!
//! Test runtimes have no timer thread and a **manual clock** that only moves when
//! [`TestRuntime::advance`](crate::testing::TestRuntime::advance) says so; the heap is served
//! deterministically in deadline order, which makes chained sleeps reproducible.
//!
//! Firing a timer never takes the core lock: it completes the sleeper's slot and wakes its
//! task. Dropping a [`Sleep`] deregisters it; a late `timer_fired` for an unknown id is
//! ignored.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[cfg(not(target_family = "wasm"))]
use parking_lot::MutexGuard;
use parking_lot::{Condvar, Mutex};

/// Where "now" comes from.
enum TimeSource {
    /// The monotonic clock, measured from when the runtime started.
    #[cfg(not(target_family = "wasm"))]
    System(std::time::Instant),
    /// A clock that only moves when told to (tests, and wasm where the host owns timers).
    Manual(AtomicU64),
}

impl TimeSource {
    fn now_ns(&self) -> u64 {
        match self {
            #[cfg(not(target_family = "wasm"))]
            TimeSource::System(start) => {
                u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
            }
            TimeSource::Manual(clock) => clock.load(Ordering::Acquire),
        }
    }
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// Rounds a delay up to whole milliseconds (the granularity of the Timer port), at least 1.
/// (Without a 128-bit division: a core that sleeps nowhere else need not link one.)
pub(crate) fn delay_ms(d: Duration) -> u64 {
    d.as_secs()
        .saturating_mul(1000)
        .saturating_add(u64::from(d.subsec_nanos().div_ceil(1_000_000)))
        .max(1)
}

#[derive(Default)]
struct SlotInner {
    fired: bool,
    waker: Option<Waker>,
}

/// The rendezvous between a sleeper and its timer.
pub(crate) struct SleepSlot {
    inner: Mutex<SlotInner>,
}

impl SleepSlot {
    fn fire(&self) {
        let waker = {
            let mut inner = self.inner.lock();
            inner.fired = true;
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

struct State {
    sleepers: HashMap<u32, Arc<SleepSlot>>,
    /// `(deadline_ns, sequence, timer_id)`: ties fire in arming order.
    heap: BinaryHeap<Reverse<(u64, u64, u32)>>,
    seq: u64,
    next_id: u32,
    shutdown: bool,
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    thread_started: bool,
}

/// The sleeper registry, the deadline heap and the optional timer thread.
pub(crate) struct Timers {
    source: TimeSource,
    state: Mutex<State>,
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    cv: Condvar,
    /// Whether an `undra-timer` thread may serve the heap (native, non-manual).
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    use_thread: bool,
    #[cfg(not(target_family = "wasm"))]
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Timers {
    /// `manual`: a clock driven by [`advance_manual`](Timers::advance_manual), no thread.
    pub(crate) fn new(manual: bool) -> Arc<Timers> {
        #[cfg(not(target_family = "wasm"))]
        let (source, use_thread) = if manual {
            (TimeSource::Manual(AtomicU64::new(0)), false)
        } else {
            (TimeSource::System(std::time::Instant::now()), true)
        };
        #[cfg(target_family = "wasm")]
        let (source, use_thread) = {
            let _ = manual;
            (TimeSource::Manual(AtomicU64::new(0)), false)
        };
        Arc::new(Timers {
            source,
            state: Mutex::new(State {
                sleepers: HashMap::new(),
                heap: BinaryHeap::new(),
                seq: 0,
                next_id: 0,
                shutdown: false,
                thread_started: false,
            }),
            cv: Condvar::new(),
            use_thread,
            #[cfg(not(target_family = "wasm"))]
            thread: Mutex::new(None),
        })
    }

    /// The current time in nanoseconds on this timer's clock.
    pub(crate) fn now_ns(&self) -> u64 {
        self.source.now_ns()
    }

    /// Registers a sleeper under a fresh nonzero id.
    pub(crate) fn register(&self) -> (u32, Arc<SleepSlot>) {
        let mut st = self.state.lock();
        let id = loop {
            st.next_id = st.next_id.wrapping_add(1);
            let candidate = st.next_id;
            if candidate != 0 && !st.sleepers.contains_key(&candidate) {
                break candidate;
            }
        };
        let slot = Arc::new(SleepSlot {
            inner: Mutex::new(SlotInner::default()),
        });
        st.sleepers.insert(id, slot.clone());
        (id, slot)
    }

    /// Arms the internal timer for sleeper `id`. Returns `false` if no timer source can ever
    /// fire it (wasm without a host timer), so the caller can warn.
    pub(crate) fn arm(self: &Arc<Self>, id: u32, delay: Duration) -> bool {
        {
            let mut st = self.state.lock();
            let deadline = self.source.now_ns().saturating_add(nanos(delay));
            st.seq += 1;
            let seq = st.seq;
            st.heap.push(Reverse((deadline, seq, id)));
        }
        self.cv.notify_all();
        self.ensure_thread();
        cfg!(not(target_family = "wasm"))
    }

    #[cfg(not(target_family = "wasm"))]
    fn ensure_thread(self: &Arc<Self>) {
        if !self.use_thread {
            return;
        }
        {
            let mut st = self.state.lock();
            if st.thread_started || st.shutdown {
                return;
            }
            st.thread_started = true;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("undra-timer".to_owned())
            .spawn(move || {
                let _alive = crate::testing::ThreadMark::enter();
                me.serve();
            });
        if let Ok(handle) = spawned {
            *self.thread.lock() = Some(handle);
        } else {
            self.state.lock().thread_started = false;
        }
    }

    #[cfg(target_family = "wasm")]
    fn ensure_thread(self: &Arc<Self>) {}

    /// The timer thread: sleep until the earliest deadline, fire, repeat.
    #[cfg(not(target_family = "wasm"))]
    fn serve(&self) {
        let mut st = self.state.lock();
        loop {
            if st.shutdown {
                return;
            }
            let now = self.source.now_ns();
            let next = st.heap.peek().map(|Reverse((deadline, _, _))| *deadline);
            match next {
                None => self.cv.wait(&mut st),
                Some(deadline) if deadline > now => {
                    let _ = self
                        .cv
                        .wait_for(&mut st, Duration::from_nanos(deadline - now));
                }
                Some(_) => {
                    let Some(Reverse((_, _, id))) = st.heap.pop() else {
                        continue;
                    };
                    let slot = st.sleepers.remove(&id);
                    MutexGuard::unlocked(&mut st, || {
                        if let Some(slot) = slot {
                            // Waking runs a waker, which is arbitrary code; the timer thread
                            // must not die because one misbehaves.
                            if let Err(report) = crate::guard::guarded(|| slot.fire()) {
                                crate::runtime::report_current(
                                    "a timer's waker panicked",
                                    "timer",
                                    &report,
                                );
                            }
                        }
                    });
                }
            }
        }
    }

    /// Completes sleeper `id` (the host's `timer_fired`). Returns whether it existed.
    pub(crate) fn fire(&self, id: u32) -> bool {
        let slot = self.state.lock().sleepers.remove(&id);
        match slot {
            Some(slot) => {
                slot.fire();
                true
            }
            None => false,
        }
    }

    /// Deregisters sleeper `id` (its [`Sleep`] was dropped).
    ///
    /// Its heap entry is left to expire (removal from the middle of a heap is linear), but
    /// once dead entries outnumber live sleepers two to one the heap is compacted, so a
    /// program that keeps cancelling long sleeps does not grow it without bound.
    pub(crate) fn cancel(&self, id: u32) {
        let mut st = self.state.lock();
        st.sleepers.remove(&id);
        if st.heap.len() > 64 && st.heap.len() > st.sleepers.len() * 2 {
            let State { sleepers, heap, .. } = &mut *st;
            heap.retain(|Reverse((_, _, id))| sleepers.contains_key(id));
        }
    }

    /// Number of registered sleepers.
    pub(crate) fn pending(&self) -> usize {
        self.state.lock().sleepers.len()
    }

    /// Moves the manual clock forward by `d`, firing every timer that comes due, in deadline
    /// order. The clock stops at each deadline before `after_each` runs, so timers armed by
    /// the woken tasks are measured from that moment and fire in the same call if they fall
    /// inside the window. Returns how many sleepers fired. Does nothing on a system clock.
    #[cfg_attr(target_family = "wasm", allow(clippy::infallible_destructuring_match))]
    pub(crate) fn advance_manual(&self, d: Duration, mut after_each: impl FnMut()) -> usize {
        let clock = match &self.source {
            TimeSource::Manual(clock) => clock,
            #[cfg(not(target_family = "wasm"))]
            TimeSource::System(_) => return 0,
        };
        let target = clock.load(Ordering::Acquire).saturating_add(nanos(d));
        let mut fired = 0;
        loop {
            let due = {
                let mut st = self.state.lock();
                let next = st.heap.peek().map(|Reverse((deadline, _, _))| *deadline);
                match next {
                    Some(deadline) if deadline <= target => match st.heap.pop() {
                        Some(Reverse((deadline, _, id))) => {
                            Some((deadline, st.sleepers.remove(&id)))
                        }
                        None => None,
                    },
                    _ => None,
                }
            };
            let Some((deadline, slot)) = due else { break };
            clock.fetch_max(deadline, Ordering::AcqRel);
            if let Some(slot) = slot {
                slot.fire();
                fired += 1;
            }
            after_each();
        }
        clock.fetch_max(target, Ordering::AcqRel);
        fired
    }

    /// Stops the timer thread and drops every sleeper.
    pub(crate) fn shutdown(&self) {
        let sleepers: Vec<_> = {
            let mut st = self.state.lock();
            st.shutdown = true;
            st.heap.clear();
            st.sleepers.drain().map(|(_, slot)| slot).collect()
        };
        self.cv.notify_all();
        drop(sleepers);
        #[cfg(not(target_family = "wasm"))]
        {
            let handle = self.thread.lock().take();
            if let Some(handle) = handle {
                if handle.thread().id() != std::thread::current().id() {
                    let _ = handle.join();
                }
            }
        }
    }
}

/// The future returned by [`Ctx::sleep`](crate::Ctx::sleep): completes after the delay.
///
/// The delay is measured from when `sleep` was called, not from the first poll. A zero delay
/// completes immediately. Dropping the future cancels the sleep.
#[must_use = "futures do nothing unless awaited"]
pub struct Sleep {
    armed: Option<(Arc<Timers>, u32, Arc<SleepSlot>)>,
}

impl Sleep {
    pub(crate) fn ready() -> Sleep {
        Sleep { armed: None }
    }

    pub(crate) fn armed(timers: Arc<Timers>, id: u32, slot: Arc<SleepSlot>) -> Sleep {
        Sleep {
            armed: Some((timers, id, slot)),
        }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let Some((_, _, slot)) = &self.armed else {
            return Poll::Ready(());
        };
        let mut inner = slot.inner.lock();
        if inner.fired {
            return Poll::Ready(());
        }
        match &inner.waker {
            Some(w) if w.will_wake(cx.waker()) => {}
            _ => inner.waker = Some(cx.waker().clone()),
        }
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some((timers, id, slot)) = &self.armed {
            if !slot.inner.lock().fired {
                timers.cancel(*id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_rounds_up_to_whole_milliseconds() {
        assert_eq!(delay_ms(Duration::from_nanos(1)), 1);
        assert_eq!(delay_ms(Duration::from_micros(999)), 1);
        assert_eq!(delay_ms(Duration::from_millis(1)), 1);
        assert_eq!(delay_ms(Duration::from_micros(1001)), 2);
        assert_eq!(delay_ms(Duration::from_secs(3)), 3000);
        assert_eq!(delay_ms(Duration::MAX), u64::MAX);
    }

    #[test]
    fn manual_clock_fires_in_deadline_order_and_ties_in_arming_order() {
        let timers = Timers::new(true);
        let fired = Arc::new(Mutex::new(Vec::new()));
        let mut slots = Vec::new();
        for (delay, tag) in [(30, 'c'), (10, 'a'), (10, 'b'), (50, 'd')] {
            let (id, slot) = timers.register();
            timers.arm(id, Duration::from_millis(delay));
            slots.push((tag, id, slot));
        }
        assert_eq!(timers.pending(), 4);
        let f = fired.clone();
        let snapshot: Vec<_> = slots.iter().map(|(t, _, s)| (*t, s.clone())).collect();
        let count = timers.advance_manual(Duration::from_millis(30), || {
            for (tag, slot) in &snapshot {
                if slot.inner.lock().fired && !f.lock().contains(tag) {
                    f.lock().push(*tag);
                }
            }
        });
        assert_eq!(count, 3);
        assert_eq!(*fired.lock(), ['a', 'b', 'c']);
        assert_eq!(timers.pending(), 1);
        assert_eq!(timers.now_ns(), 30_000_000);
        assert_eq!(timers.advance_manual(Duration::from_millis(19), || {}), 0);
        assert_eq!(timers.advance_manual(Duration::from_millis(1), || {}), 1);
    }

    #[test]
    fn cancelled_sleepers_do_not_fire_but_the_clock_still_advances() {
        let timers = Timers::new(true);
        let (id, _slot) = timers.register();
        timers.arm(id, Duration::from_secs(1));
        timers.cancel(id);
        assert_eq!(timers.advance_manual(Duration::from_secs(2), || {}), 0);
        assert_eq!(timers.now_ns(), 2_000_000_000);
    }

    #[test]
    fn cancelling_many_long_sleeps_compacts_the_heap() {
        let timers = Timers::new(true);
        let mut live = Vec::new();
        for i in 0..1000_u64 {
            let (id, slot) = timers.register();
            timers.arm(id, Duration::from_secs(3600 + i));
            live.push((id, slot));
        }
        // Cancel all but ten.
        for (id, _) in live.iter().skip(10) {
            timers.cancel(*id);
        }
        assert_eq!(timers.pending(), 10);
        let heap_len = timers.state.lock().heap.len();
        assert!(heap_len <= 65, "heap compacted, still {heap_len}");
        // The survivors still fire, in order.
        assert_eq!(
            timers.advance_manual(Duration::from_secs(3600 + 9), || {}),
            10
        );
    }

    #[test]
    fn host_fired_ids_complete_and_unknown_ids_are_ignored() {
        let timers = Timers::new(true);
        let (id, slot) = timers.register();
        assert!(timers.fire(id));
        assert!(slot.inner.lock().fired);
        assert!(!timers.fire(id));
        assert!(!timers.fire(9999));
    }

    #[test]
    fn ids_are_nonzero_distinct_and_skip_live_ones_after_wrapping() {
        let timers = Timers::new(true);
        timers.state.lock().next_id = u32::MAX;
        let (a, _sa) = timers.register();
        assert_eq!(a, 1, "wraps past zero");
        timers.state.lock().next_id = 0;
        let (b, _sb) = timers.register();
        assert_eq!(b, 2, "skips the live id 1");
    }

    #[test]
    fn system_clock_thread_fires_a_real_timer() {
        let timers = Timers::new(false);
        let (id, slot) = timers.register();
        assert!(timers.arm(id, Duration::from_millis(20)));
        for _ in 0..400 {
            if slot.inner.lock().fired {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(slot.inner.lock().fired);
        timers.shutdown();
    }

    #[test]
    fn system_clock_thread_is_woken_by_an_earlier_deadline() {
        let timers = Timers::new(false);
        let (late, late_slot) = timers.register();
        timers.arm(late, Duration::from_secs(30));
        let (soon, soon_slot) = timers.register();
        timers.arm(soon, Duration::from_millis(10));
        for _ in 0..400 {
            if soon_slot.inner.lock().fired {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(soon_slot.inner.lock().fired);
        assert!(!late_slot.inner.lock().fired);
        timers.shutdown();
        assert_eq!(timers.pending(), 0);
    }

    #[test]
    fn shutdown_is_idempotent_and_joins_the_thread() {
        let timers = Timers::new(false);
        let (id, _slot) = timers.register();
        timers.arm(id, Duration::from_secs(60));
        timers.shutdown();
        timers.shutdown();
    }
}
