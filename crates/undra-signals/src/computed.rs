//! [`Computed`]: a value derived from signals and other computeds.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::RwLock;

use std::cell::RefCell;

use crate::deps::{Compute, Deps};
use crate::graph::{Binding, Dependents, Reactive, add_dependent, propagate, record};
use crate::value::SignalValue;

/// A value derived from other signals and computeds, recomputed only when they change.
///
/// * **Lazy.** Nothing runs at construction. The closure runs on the first [`get`](Computed::get)
///   or [`with`](Computed::with), and again only after a dependency has been written.
/// * **Cached.** Between writes every read returns the cached value; a diamond (two computeds
///   sharing an input, both read by a third) recomputes each node once per change.
/// * **Eager when observed.** If the computed is attached to a store and the host observes it,
///   the commit that follows a change of its inputs recomputes it, so the new value can be
///   encoded into the change-set.
/// * **Composable.** A computed may depend on other computeds. Dependencies are fixed at
///   construction and can only refer to nodes that already exist, so the dependency graph the
///   crate tracks cannot contain a cycle. A closure can still *read* a computed it did not
///   declare (say, one reached through a `OnceLock` that is filled in later) and so form one
///   behind the crate's back. Such a cycle is detected the moment a computed is recomputed
///   again on the thread that is already recomputing it, and panics with "computed cycle
///   detected" instead of overflowing the stack (a panic can be caught at the dispatch
///   boundary, a stack overflow cannot).
///
/// [`Clone`] gives another handle to the same computed.
///
/// The closure should be deterministic in its inputs. It holds no value lock while it runs, so
/// it may write signals; a write made while a commit is in progress is queued and committed as
/// a separate transaction. (A computed that writes one of its own inputs invalidates itself; a
/// commit that keeps recomputing such a computed is cut off after 1000 rounds.) During a
/// commit the closure runs under its store's **delivery lock**, so it must not wait on another
/// thread that writes the same store — like a change sink, it would deadlock (ADR-020).
///
/// # Example
///
/// ```
/// use undra_signals::{Computed, Signal};
///
/// let width = Signal::new(3);
/// let height = Signal::new(4);
/// let area = Computed::new((&width, &height), |(w, h)| w * h);
/// assert_eq!(area.get(), 12);
/// width.set(5);
/// assert_eq!(area.get(), 20);
/// ```
pub struct Computed<T> {
    pub(crate) inner: Arc<ComputedInner<T>>,
}

pub(crate) struct ComputedInner<T> {
    compute: Box<dyn Compute<T>>,
    cache: RwLock<Cache<T>>,
    /// The cache may be stale (or empty). Cleared *before* a recompute starts so that a write
    /// racing with the recompute sets it again.
    dirty: AtomicBool,
    /// Numbers recomputes in start order; the newest installed result wins.
    tickets: AtomicU64,
    /// The last invalidation walk that reached this node (diamond de-duplication).
    visited: AtomicU64,
    pub(crate) dependents: Dependents,
    pub(crate) binding: OnceLock<Binding>,
}

thread_local! {
    /// The computeds whose closures are running on this thread, innermost last (by address).
    static RECOMPUTING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Marks a computed as being recomputed on this thread until dropped; entering a computed that
/// is already being recomputed here is a cycle.
struct Recomputing;

impl Recomputing {
    fn enter(node: usize) -> Recomputing {
        let cycle = RECOMPUTING
            .try_with(|stack| {
                let mut stack = stack.borrow_mut();
                if stack.contains(&node) {
                    true
                } else {
                    stack.push(node);
                    false
                }
            })
            .unwrap_or(false);
        assert!(
            !cycle,
            "undra-signals: computed cycle detected: a computed's closure read the computed it \
             is computing, directly or through other computeds (or wrote its own input while \
             it was read outside a commit). Dependencies are fixed at construction, so this can \
             only happen through a handle the closure captured, such as a `OnceLock` filled in \
             later. Break the cycle: a computed must only read the dependencies it declared."
        );
        Recomputing
    }
}

impl Drop for Recomputing {
    fn drop(&mut self) {
        let _ = RECOMPUTING.try_with(|stack| stack.borrow_mut().pop());
    }
}

struct Cache<T> {
    value: Option<Arc<T>>,
    /// Ticket of the recompute that produced `value`; 0 while empty.
    ticket: u64,
}

impl<T> Clone for Computed<T> {
    fn clone(&self) -> Self {
        Computed {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: SignalValue> Computed<T> {
    /// Creates a computed that reads `deps` and derives its value with `f`.
    ///
    /// `deps` is a reference to one [`Signal`](crate::Signal) or [`Computed`], or a tuple of up
    /// to six. `f` receives references to their current values (a tuple of references for a
    /// tuple).
    pub fn new<D: Deps>(
        deps: D,
        f: impl for<'a> Fn(D::Values<'a>) -> T + Send + Sync + 'static,
    ) -> Computed<T> {
        let inner = Arc::new(ComputedInner {
            compute: deps.into_compute(f),
            cache: RwLock::new(Cache {
                value: None,
                ticket: 0,
            }),
            dirty: AtomicBool::new(true),
            tickets: AtomicU64::new(0),
            visited: AtomicU64::new(0),
            dependents: parking_lot::Mutex::new(Vec::new()),
            binding: OnceLock::new(),
        });
        // Subscribe only once the node is fully built: a `Weak` that cannot be upgraded yet
        // would be pruned by a concurrent write.
        let weak: Weak<dyn Reactive> = Arc::downgrade(&inner) as Weak<dyn Reactive>;
        inner.compute.subscribe(weak);
        Computed { inner }
    }

    /// Returns a clone of the current value, recomputing first if a dependency changed.
    pub fn get(&self) -> T {
        T::clone(&self.inner.current())
    }

    /// Calls `f` with a reference to the current value (recomputing first if a dependency
    /// changed) without cloning it. No lock is held while `f` runs.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.inner.current())
    }

    /// Returns `true` if `self` and `other` are handles to the same computed.
    pub fn ptr_eq(&self, other: &Computed<T>) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Returns `true` once the computed has been attached to a store.
    pub fn is_attached(&self) -> bool {
        self.inner.binding.get().is_some()
    }

    /// Returns `true` if the next read would recompute.
    #[cfg(test)]
    pub(crate) fn is_stale(&self) -> bool {
        self.inner.dirty.load(Ordering::SeqCst)
    }

    pub(crate) fn add_dependent(&self, dependent: Weak<dyn Reactive>) {
        add_dependent(&self.inner.dependents, dependent);
    }
}

impl<T: SignalValue> ComputedInner<T> {
    /// The up-to-date value.
    fn current(&self) -> Arc<T> {
        if !self.dirty.load(Ordering::SeqCst) {
            if let Some(value) = self.cache.read_recursive().value.clone() {
                return value;
            }
        }
        self.recompute()
    }

    fn recompute(&self) -> Arc<T> {
        // Before anything changes: a re-entrant recompute of this node is a cycle.
        let _running = Recomputing::enter(std::ptr::from_ref(self) as usize);
        let ticket = self.tickets.fetch_add(1, Ordering::SeqCst) + 1;
        self.dirty.store(false, Ordering::SeqCst);

        // If the closure panics the cache is still the old one, so the node must stay stale.
        let mut restale = RestaleOnUnwind {
            dirty: &self.dirty,
            armed: true,
        };
        let fresh = Arc::new(self.compute.run());
        restale.armed = false;

        let mut cache = self.cache.write();
        if ticket >= cache.ticket {
            cache.ticket = ticket;
            cache.value = Some(Arc::clone(&fresh));
            fresh
        } else {
            // A recompute that started later already installed its result; it saw inputs at
            // least as new as ours, so prefer it.
            cache.value.clone().unwrap_or(fresh)
        }
    }
}

struct RestaleOnUnwind<'a> {
    dirty: &'a AtomicBool,
    armed: bool,
}

impl Drop for RestaleOnUnwind<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.dirty.store(true, Ordering::SeqCst);
        }
    }
}

impl<T: SignalValue> Reactive for ComputedInner<T> {
    fn invalidate(self: Arc<Self>, pass: u64) {
        if self.visited.swap(pass, Ordering::Relaxed) == pass {
            return;
        }
        self.dirty.store(true, Ordering::SeqCst);
        if let Some(binding) = self.binding.get() {
            record(binding);
        }
        propagate(&self.dependents, pass);
    }
}

impl<T: SignalValue + fmt::Debug> fmt::Debug for Computed<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never recompute from `Debug`: that could run user code from a formatter.
        let cached = self.inner.cache.read_recursive().value.clone();
        let mut s = f.debug_struct("Computed");
        match cached {
            Some(value) => s.field("cached", &*value),
            None => s.field("cached", &format_args!("<not computed>")),
        };
        s.field("stale", &self.inner.dirty.load(Ordering::SeqCst))
            .field("attached", &self.is_attached())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Signal;
    use std::sync::atomic::AtomicUsize;

    /// A computed over `deps` that counts how often its closure runs.
    fn counted<D: Deps>(
        deps: D,
        f: impl for<'a> Fn(D::Values<'a>) -> i32 + Send + Sync + 'static,
    ) -> (Computed<i32>, Arc<AtomicUsize>) {
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        let c = Computed::new(deps, move |v| {
            counter.fetch_add(1, Ordering::SeqCst);
            f(v)
        });
        (c, runs)
    }

    fn runs(r: &Arc<AtomicUsize>) -> usize {
        r.load(Ordering::SeqCst)
    }

    #[test]
    fn nothing_runs_until_the_first_read() {
        let a = Signal::new(2);
        let (c, n) = counted(&a, |a| a * 10);
        assert_eq!(runs(&n), 0);
        assert!(c.is_stale());
        assert_eq!(c.get(), 20);
        assert_eq!(runs(&n), 1);
        assert!(!c.is_stale());
    }

    #[test]
    fn repeated_reads_use_the_cache() {
        let a = Signal::new(2);
        let (c, n) = counted(&a, |a| a + 1);
        assert_eq!(c.get(), 3);
        assert_eq!(c.get(), 3);
        assert_eq!(c.with(|v| *v), 3);
        assert_eq!(runs(&n), 1);
    }

    #[test]
    fn recomputes_after_a_dependency_is_written() {
        let a = Signal::new(2);
        let (c, n) = counted(&a, |a| a + 1);
        assert_eq!(c.get(), 3);
        a.set(10);
        assert!(c.is_stale());
        assert_eq!(runs(&n), 1, "invalidation must not recompute eagerly");
        assert_eq!(c.get(), 11);
        assert_eq!(runs(&n), 2);
        a.update(|v| *v += 1);
        assert_eq!(c.get(), 12);
        assert_eq!(runs(&n), 3);
    }

    #[test]
    fn writing_an_unrelated_signal_does_not_invalidate() {
        let a = Signal::new(1);
        let other = Signal::new(1);
        let (c, n) = counted(&a, |a| *a);
        assert_eq!(c.get(), 1);
        other.set(2);
        assert!(!c.is_stale());
        assert_eq!(c.get(), 1);
        assert_eq!(runs(&n), 1);
    }

    #[test]
    fn several_writes_before_a_read_recompute_once() {
        let a = Signal::new(1);
        let (c, n) = counted(&a, |a| *a);
        assert_eq!(c.get(), 1);
        a.set(2);
        a.set(3);
        a.set(4);
        assert_eq!(c.get(), 4);
        assert_eq!(runs(&n), 2);
    }

    #[test]
    fn two_dependencies() {
        let a = Signal::new(2);
        let b = Signal::new(3);
        let (c, n) = counted((&a, &b), |(a, b)| a * b);
        assert_eq!(c.get(), 6);
        a.set(4);
        assert_eq!(c.get(), 12);
        b.set(5);
        assert_eq!(c.get(), 20);
        assert_eq!(runs(&n), 3);
    }

    #[test]
    fn a_computed_can_depend_on_a_computed() {
        let a = Signal::new(1);
        let (double, double_runs) = counted(&a, |a| a * 2);
        let (plus_one, plus_one_runs) = counted(&double, |d| d + 1);
        assert_eq!(plus_one.get(), 3);
        a.set(10);
        assert!(
            double.is_stale() && plus_one.is_stale(),
            "invalidation is transitive"
        );
        assert_eq!(plus_one.get(), 21);
        assert_eq!(runs(&double_runs), 2);
        assert_eq!(runs(&plus_one_runs), 2);
    }

    #[test]
    fn diamond_recomputes_every_node_once_per_change() {
        //      a
        //     / \
        //    l   r
        //     \ /
        //      sum
        let a = Signal::new(1);
        let (l, l_runs) = counted(&a, |a| a + 1);
        let (r, r_runs) = counted(&a, |a| a * 10);
        let (sum, sum_runs) = counted((&l, &r), |(l, r)| l + r);
        assert_eq!(sum.get(), 12);
        a.set(2);
        assert_eq!(sum.get(), 23);
        a.set(3);
        assert_eq!(sum.get(), 34);
        assert_eq!(runs(&l_runs), 3);
        assert_eq!(runs(&r_runs), 3);
        assert_eq!(runs(&sum_runs), 3);
    }

    #[test]
    fn deep_diamond_chain_invalidates_in_linear_time() {
        // Each layer has two nodes reading both nodes of the previous layer, so the number of
        // paths doubles per layer. Invalidation must not walk paths.
        let src = Signal::new(1_i32);
        let (mut left, _) = counted(&src, |v| *v);
        let (mut right, _) = counted(&src, |v| *v);
        for _ in 0..40 {
            let (l, _) = counted((&left, &right), |(a, b)| a.wrapping_add(*b));
            let (r, _) = counted((&left, &right), |(a, b)| a.wrapping_sub(*b));
            left = l;
            right = r;
        }
        src.set(2); // would take ~2^40 steps if paths were walked
        assert!(left.is_stale() && right.is_stale());
        // Reading recomputes each node once and terminates quickly.
        let _ = left.get();
        let _ = right.get();
    }

    #[test]
    fn up_to_six_dependencies() {
        let s: Vec<Signal<i32>> = (1..=6).map(Signal::new).collect();
        let c1 = Computed::new(&s[0], |a| *a);
        let c2 = Computed::new((&s[0], &s[1]), |(a, b)| a + b);
        let c3 = Computed::new((&s[0], &s[1], &s[2]), |(a, b, c)| a + b + c);
        let c4 = Computed::new((&s[0], &s[1], &s[2], &s[3]), |(a, b, c, d)| a + b + c + d);
        let c5 = Computed::new((&s[0], &s[1], &s[2], &s[3], &s[4]), |(a, b, c, d, e)| {
            a + b + c + d + e
        });
        let c6 = Computed::new(
            (&s[0], &s[1], &s[2], &s[3], &s[4], &s[5]),
            |(a, b, c, d, e, f)| a + b + c + d + e + f,
        );
        assert_eq!(
            (c1.get(), c2.get(), c3.get(), c4.get(), c5.get(), c6.get()),
            (1, 3, 6, 10, 15, 21)
        );
        s[5].set(100);
        assert_eq!(c6.get(), 115);
        assert_eq!(c5.get(), 15, "c5 does not read the sixth signal");
        s[0].set(0);
        assert_eq!(c6.get(), 114);
        assert_eq!(c1.get(), 0);
    }

    #[test]
    fn mixed_signal_and_computed_dependencies() {
        let a = Signal::new(2);
        let double = Computed::new(&a, |a| a * 2);
        let mixed = Computed::new((&a, &double), |(a, d)| format!("{a}:{d}"));
        assert_eq!(mixed.get(), "2:4");
        a.set(5);
        assert_eq!(mixed.get(), "5:10");
    }

    #[test]
    fn closure_receives_references_not_clones() {
        #[derive(Debug)]
        struct Big(Arc<AtomicUsize>);
        impl Clone for Big {
            fn clone(&self) -> Self {
                self.0.fetch_add(1, Ordering::SeqCst);
                Big(self.0.clone())
            }
        }
        impl undra_wire::Encode for Big {
            fn encode(&self, _w: &mut undra_wire::Writer) {}
        }
        let clones = Arc::new(AtomicUsize::new(0));
        let s = Signal::new(Big(clones.clone()));
        let c = Computed::new(&s, |big: &Big| {
            u32::try_from(Arc::strong_count(&big.0)).expect("small")
        });
        assert_eq!(c.get(), 2);
        assert_eq!(clones.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn with_reads_without_cloning_the_value() {
        let a = Signal::new(3);
        let c = Computed::new(&a, |a| vec![*a; 4]);
        assert_eq!(c.with(|v| v.len()), 4);
        assert_eq!(c.with(|v| v[0]), 3);
    }

    #[test]
    fn clones_share_the_cache() {
        let a = Signal::new(1);
        let (c, n) = counted(&a, |a| *a);
        let c2 = c.clone();
        assert!(c.ptr_eq(&c2));
        assert_eq!(c.get(), 1);
        assert_eq!(c2.get(), 1);
        assert_eq!(runs(&n), 1);
        a.set(2);
        assert_eq!(c2.get(), 2);
        assert_eq!(c.get(), 2);
        assert_eq!(runs(&n), 2);
    }

    #[test]
    fn a_dropped_computed_is_no_longer_notified() {
        let a = Signal::new(1);
        {
            let c = Computed::new(&a, |a| *a);
            assert_eq!(c.get(), 1);
            assert_eq!(a.inner.dependents.lock().len(), 1);
        }
        a.set(2);
        assert_eq!(
            a.inner.dependents.lock().len(),
            0,
            "dead weak entry is pruned"
        );
    }

    #[test]
    fn a_computed_keeps_its_sources_alive() {
        let c = {
            let a = Signal::new(21);
            Computed::new(&a, |a| a * 2)
        };
        assert_eq!(c.get(), 42);
    }

    #[test]
    fn a_panicking_closure_leaves_the_computed_stale_and_recoverable() {
        let a = Signal::new(1);
        let c = Computed::new(&a, |a: &i32| {
            assert!(*a != 0, "zero is not allowed");
            10 / a
        });
        assert_eq!(c.get(), 10);
        a.set(0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.get()));
        assert!(result.is_err());
        assert!(c.is_stale(), "a failed recompute must not look fresh");
        a.set(5);
        assert_eq!(c.get(), 2);
    }

    #[test]
    fn a_closure_may_write_an_unrelated_signal() {
        let a = Signal::new(1);
        let log = Signal::new(Vec::<i32>::new());
        let log2 = log.clone();
        let c = Computed::new(&a, move |a: &i32| {
            log2.update(|l| l.push(*a));
            *a
        });
        assert_eq!(c.get(), 1);
        a.set(2);
        assert_eq!(c.get(), 2);
        assert_eq!(log.get(), vec![1, 2]);
    }

    #[test]
    fn a_closure_that_writes_its_own_input_does_not_deadlock() {
        let a = Signal::new(1);
        let a2 = a.clone();
        let c = Computed::new(&a, move |v: &i32| {
            a2.set(*v); // writes the signal it is reading: no lock is held, so this is safe
            *v * 2
        });
        assert_eq!(c.get(), 2);
        // The self-write invalidated the computed, so the next read recomputes; still no hang.
        assert!(c.is_stale());
        assert_eq!(c.get(), 2);
    }

    #[test]
    fn the_same_signal_may_appear_twice_in_a_dependency_tuple() {
        let a = Signal::new(2);
        let (c, n) = counted((&a, &a), |(x, y)| x * y);
        assert_eq!(c.get(), 4);
        a.set(5);
        assert_eq!(c.get(), 25);
        assert_eq!(
            runs(&n),
            2,
            "one recompute per change, not one per subscription"
        );
    }

    #[test]
    fn debug_does_not_recompute() {
        let a = Signal::new(1);
        let (c, n) = counted(&a, |a| *a);
        let before = format!("{c:?}");
        assert!(before.contains("<not computed>"), "{before}");
        assert_eq!(runs(&n), 0);
        let _ = c.get();
        let after = format!("{c:?}");
        assert!(after.contains("cached: 1"), "{after}");
    }

    #[test]
    fn computeds_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Computed<i32>>();
    }

    #[test]
    fn concurrent_readers_and_writers_converge_to_the_latest_value() {
        let a = Signal::new(0_i32);
        let c = Computed::new(&a, |a| a * 2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for i in 1..=2000 {
                    a.set(i);
                }
            });
            for _ in 0..3 {
                scope.spawn(|| {
                    for _ in 0..2000 {
                        let v = c.get();
                        assert_eq!(v % 2, 0);
                    }
                });
            }
        });
        // Once the writers are done, the computed must reflect the final input.
        assert_eq!(c.get(), 4000);
    }
}
