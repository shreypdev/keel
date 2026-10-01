//! [`Effect`]: a side effect that re-runs after the inputs it reads change.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use crate::deps::{Compute, Deps};
use crate::graph::Reactive;

/// A side effect that runs after every commit that changed one of its dependencies.
///
/// * The closure does **not** run when the effect is created; it runs after the first commit
///   that dirties a dependency.
/// * It runs **after** the commit's change-sets have been delivered to the sink, so it sees
///   committed state.
/// * However many dependencies change, and however many times, within one transaction it runs
///   once.
/// * It runs on the thread that committed. Signals it writes are committed afterwards as a new
///   transaction (never re-entrantly); an effect that keeps re-triggering itself is cut off
///   after 1000 rounds per outermost commit: the queued effects are dropped from the queue (a
///   later change queues them again), the changes already made are delivered, and the sink is
///   told through [`ChangeSink::round_cap_hit`](crate::ChangeSink::round_cap_hit).
/// * Dropping the `Effect` cancels it: it will not run again, even if a run was already queued.
///
/// The closure receives references to the dependencies' current values, exactly like
/// [`Computed::new`](crate::Computed::new). It holds no lock, so it may write any signal,
/// including one it reads (say, clamping a value back into range): the write is queued as a new
/// transaction and the effect runs again with the new value.
///
/// # Example
///
/// ```
/// use undra_signals::{Effect, Signal, txn};
/// use std::sync::{Arc, Mutex};
///
/// let seen = Arc::new(Mutex::new(Vec::new()));
/// let count = Signal::new(0);
/// let log = seen.clone();
/// let _effect = Effect::new(&count, move |n| log.lock().unwrap().push(*n));
///
/// count.set(1);
/// txn(|| {
///     count.set(2);
///     count.set(3); // one run for the whole transaction
/// });
/// assert_eq!(*seen.lock().unwrap(), vec![1, 3]);
/// ```
#[must_use = "dropping an Effect cancels it"]
pub struct Effect {
    inner: Arc<EffectInner>,
}

pub(crate) struct EffectInner {
    body: Box<dyn Compute<()>>,
    /// A run is queued (or about to be).
    dirty: AtomicBool,
    cancelled: AtomicBool,
    /// The last invalidation walk that reached this node (diamond de-duplication).
    visited: AtomicU64,
}

impl Effect {
    /// Creates an effect that runs `f` after commits that change any of `deps`.
    pub fn new<D: Deps>(
        deps: D,
        f: impl for<'a> Fn(D::Values<'a>) + Send + Sync + 'static,
    ) -> Effect {
        let inner = Arc::new(EffectInner {
            body: deps.into_compute(f),
            dirty: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            visited: AtomicU64::new(0),
        });
        let weak: Weak<dyn Reactive> = Arc::downgrade(&inner) as Weak<dyn Reactive>;
        inner.body.subscribe(weak);
        Effect { inner }
    }

    /// Cancels the effect. Equivalent to dropping it.
    pub fn cancel(self) {
        drop(self);
    }
}

impl EffectInner {
    /// Forgets a queued run (the commit loop was cut off): the next invalidation queues the
    /// effect again instead of assuming a run is already on its way.
    pub(crate) fn cancel_queued_run(&self) {
        self.dirty.store(false, Ordering::SeqCst);
    }

    /// Runs the body if a run is queued and the effect has not been cancelled. Called by the
    /// commit loop, with no locks held.
    pub(crate) fn run_if_dirty(&self) {
        if self.dirty.swap(false, Ordering::SeqCst) && !self.cancelled.load(Ordering::SeqCst) {
            self.body.run();
        }
    }
}

impl Reactive for EffectInner {
    fn invalidate(self: Arc<Self>, pass: u64) {
        if self.visited.swap(pass, Ordering::Relaxed) == pass {
            return;
        }
        if self.cancelled.load(Ordering::SeqCst) {
            return;
        }
        if !self.dirty.swap(true, Ordering::SeqCst) {
            crate::txn::push_effect(self);
        }
    }
}

impl Drop for Effect {
    fn drop(&mut self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
    }
}

impl fmt::Debug for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Effect")
            .field("queued", &self.inner.dirty.load(Ordering::SeqCst))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CaptureSink;
    use crate::{Computed, Signal, txn, with_sink};
    use parking_lot::Mutex;

    fn log_effect<D: Deps>(
        deps: D,
        f: impl for<'a> Fn(D::Values<'a>) -> String + Send + Sync + 'static,
    ) -> (Effect, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        let effect = Effect::new(deps, move |v| sink.lock().push(f(v)));
        (effect, log)
    }

    #[test]
    fn does_not_run_on_creation() {
        let a = Signal::new(1);
        let (_e, log) = log_effect(&a, |a| a.to_string());
        assert!(log.lock().is_empty());
    }

    #[test]
    fn runs_after_a_write() {
        let a = Signal::new(1);
        let (_e, log) = log_effect(&a, |a| a.to_string());
        a.set(2);
        a.set(3);
        assert_eq!(*log.lock(), vec!["2", "3"]);
    }

    #[test]
    fn runs_once_per_transaction() {
        let a = Signal::new(0);
        let (_e, log) = log_effect(&a, |a| a.to_string());
        txn(|| {
            a.set(1);
            a.set(2);
            a.update(|v| *v += 1);
            assert!(log.lock().is_empty(), "nothing runs inside the transaction");
        });
        assert_eq!(*log.lock(), vec!["3"]);
    }

    #[test]
    fn nested_transactions_run_the_effect_once() {
        let a = Signal::new(0);
        let (_e, log) = log_effect(&a, |a| a.to_string());
        txn(|| {
            a.set(1);
            txn(|| a.set(2));
            a.set(3);
        });
        assert_eq!(log.lock().len(), 1);
    }

    #[test]
    fn two_dependencies_changed_together_run_once() {
        let a = Signal::new(1);
        let b = Signal::new(2);
        let (_e, log) = log_effect((&a, &b), |(a, b)| format!("{a},{b}"));
        txn(|| {
            a.set(10);
            b.set(20);
        });
        assert_eq!(*log.lock(), vec!["10,20"]);
    }

    #[test]
    fn a_change_to_either_dependency_triggers_it() {
        let a = Signal::new(1);
        let b = Signal::new(2);
        let (_e, log) = log_effect((&a, &b), |(a, b)| format!("{a},{b}"));
        a.set(5);
        b.set(6);
        assert_eq!(*log.lock(), vec!["5,2", "5,6"]);
    }

    #[test]
    fn an_unrelated_write_does_not_trigger_it() {
        let a = Signal::new(1);
        let other = Signal::new(1);
        let (_e, log) = log_effect(&a, |a| a.to_string());
        other.set(2);
        assert!(log.lock().is_empty());
    }

    #[test]
    fn depends_on_a_computed() {
        let a = Signal::new(1);
        let double = Computed::new(&a, |a| a * 2);
        let (_e, log) = log_effect(&double, |d| d.to_string());
        a.set(4);
        assert_eq!(*log.lock(), vec!["8"]);
    }

    #[test]
    fn dropping_cancels() {
        let a = Signal::new(1);
        let (e, log) = log_effect(&a, |a| a.to_string());
        a.set(2);
        drop(e);
        a.set(3);
        assert_eq!(*log.lock(), vec!["2"]);
    }

    #[test]
    fn cancel_is_the_same_as_drop() {
        let a = Signal::new(1);
        let (e, log) = log_effect(&a, |a| a.to_string());
        e.cancel();
        a.set(2);
        assert!(log.lock().is_empty());
    }

    #[test]
    fn cancelling_inside_the_transaction_prevents_the_queued_run() {
        let a = Signal::new(1);
        let (e, log) = log_effect(&a, |a| a.to_string());
        txn(|| {
            a.set(2);
            drop(e);
        });
        assert!(log.lock().is_empty());
    }

    #[test]
    fn runs_after_the_change_set_is_delivered() {
        // The sink and the effect append to one log; delivery must come first.
        struct LogSink(Arc<Mutex<Vec<String>>>);
        impl crate::ChangeSink for LogSink {
            fn deliver(&self, _change_set: &[u8]) {
                self.0.lock().push("sink".to_string());
            }
        }
        let order = Arc::new(Mutex::new(Vec::new()));
        let cell = crate::StoreCell::new(1);
        let a = Signal::new(0_u32);
        cell.attach(&a, 0).expect("attach");
        cell.set_handle(1);
        cell.observe(0, true, &mut undra_wire::Writer::new());
        let sink_log = order.clone();
        let effect_log = order.clone();
        let _e = Effect::new(&a, move |v| effect_log.lock().push(format!("effect {v}")));
        with_sink(Arc::new(LogSink(sink_log)), || a.set(9));
        assert_eq!(*order.lock(), vec!["sink", "effect 9"]);
    }

    #[test]
    fn an_effect_sees_committed_values() {
        let a = Signal::new(0);
        let b = Signal::new(0);
        let (_e, log) = log_effect((&a, &b), |(a, b)| format!("{a}/{b}"));
        txn(|| {
            a.set(1);
            b.set(1);
        });
        assert_eq!(*log.lock(), vec!["1/1"], "never a half-written state");
    }

    #[test]
    fn writes_from_an_effect_are_committed_as_a_new_transaction() {
        let sink = CaptureSink::new();
        let cell = crate::StoreCell::new(1);
        let a = Signal::new(0_u32);
        let b = Signal::new(0_u32);
        cell.attach(&a, 0).expect("attach");
        cell.attach(&b, 1).expect("attach");
        cell.set_handle(1);
        cell.observe(crate::ALL_SIGNALS, true, &mut undra_wire::Writer::new());

        let b2 = b.clone();
        let _e = Effect::new(&a, move |a| b2.set(*a * 10));
        with_sink(sink.clone(), || a.set(1));
        let sets = sink.take_decoded();
        assert_eq!(sets.len(), 2, "one change-set per transaction");
        assert_eq!(sets[0].entries.len(), 1);
        assert_eq!(sets[0].entries[0].signal_id, 0);
        assert_eq!(sets[1].entries.len(), 1);
        assert_eq!(sets[1].entries[0].signal_id, 1);
        assert_eq!(sets[1].entries[0].value, 10_u32.to_le_bytes());
        assert!(sets[1].txn_id > sets[0].txn_id);
        assert_eq!(b.get(), 10);
    }

    #[test]
    fn a_panicking_effect_does_not_stop_the_others_and_is_reraised() {
        let a = Signal::new(0);
        let ran = Arc::new(Mutex::new(Vec::new()));
        let r1 = ran.clone();
        let r2 = ran.clone();
        let _bad = Effect::new(&a, |_: &i32| panic!("effect failure"));
        let _good = Effect::new(&a, move |v: &i32| r1.lock().push(*v));
        let _good2 = Effect::new(&a, move |v: &i32| r2.lock().push(*v + 100));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.set(1)));
        assert!(
            result.is_err(),
            "the first panic is re-raised after the commit"
        );
        let mut seen = ran.lock().clone();
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 101]);
        // The thread is clean afterwards.
        assert_eq!(crate::txn::depth(), 0);
        assert_eq!(crate::txn::pending_len(), 0);
    }

    #[test]
    fn a_self_triggering_effect_cycle_is_cut_off() {
        // x -> y -> x: each effect writes the signal the other one reads, so without the
        // round cap this commit would never return.
        let x = Signal::new(0_u32);
        let y = Signal::new(0_u32);
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (y1, hits1) = (y.clone(), hits.clone());
        let _ex = Effect::new(&x, move |v: &u32| {
            hits1.fetch_add(1, Ordering::SeqCst);
            y1.set(*v + 1);
        });
        let x2 = x.clone();
        let _ey = Effect::new(&y, move |v: &u32| x2.set(*v + 1));
        x.set(1); // returns: the loop is bounded
        let n = hits.load(Ordering::SeqCst);
        assert!(
            (1..=crate::txn::MAX_COMMIT_ROUNDS).contains(&n),
            "ran {n} times"
        );
        assert_eq!(crate::txn::depth(), 0);
    }

    #[test]
    fn an_effect_may_write_the_signal_it_reads() {
        // A clamp: whenever the value leaves 0..=100, the effect writes it back into range.
        let value = Signal::new(50_i32);
        let writer = value.clone();
        let runs = Arc::new(Mutex::new(Vec::new()));
        let log = runs.clone();
        let _clamp = Effect::new(&value, move |v: &i32| {
            log.lock().push(*v);
            if *v > 100 {
                writer.set(100);
            }
        });
        value.set(250);
        assert_eq!(value.get(), 100);
        assert_eq!(
            *runs.lock(),
            vec![250, 100],
            "runs again with the corrected value"
        );
        value.set(7);
        assert_eq!(value.get(), 7);
    }

    #[test]
    fn an_effect_may_update_the_signal_it_reads_in_place() {
        let list = Signal::new(vec![3, 1, 2]);
        let writer = list.clone();
        let _sort = Effect::new(&list, move |l: &Vec<i32>| {
            if !l.windows(2).all(|w| w[0] <= w[1]) {
                writer.update(|l| l.sort_unstable());
            }
        });
        list.update(|l| l.push(0));
        assert_eq!(list.get(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn effects_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Effect>();
    }

    #[test]
    fn debug_output() {
        let a = Signal::new(1);
        let e = Effect::new(&a, |_: &i32| {});
        assert_eq!(format!("{e:?}"), "Effect { queued: false }");
    }
}
