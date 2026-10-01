//! Derived keyed lists (ADR-039): filtered, sorted and mapped views of a `Signal<Vec<T>>` kept up
//! to date from the source's recorded operations, at O(log n) per changed row.
//!
//! A [`DerivedList`] is built from a list signal by a pipeline ([`Signal::derive`], then
//! [`filter`](Derive::filter), [`filter_with`](Derive::filter_with), [`map`](Derive::map),
//! [`sort_by_key`](Derive::sort_by_key), [`sort_by_key_with`](Derive::sort_by_key_with), and
//! [`build`](Derive::build) or [`count`](Derive::count)). Its value is always
//!
//! ```text
//! view(source) = stable_sort_by_key( [ out(t) for t in source if passes(t) ] )
//! ```
//!
//! but it is not recomputed: it replays the source's recorded operations (`push`, `insert`,
//! `remove`, `update_at`, `move_item`, `clear`) on an index of its own (two order-statistic
//! trees) and turns each one into at most two keyed-patch ops of the view. Attached to a store
//! ([`StoreCell::attach_derived`](crate::StoreCell::attach_derived)), it reaches the host as those
//! ops: one row changed in a 100,000-row source is one op of a few bytes, never the view.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Weak};

use crate::computed::Computed;
use crate::deps::{Compute, Dep, OwnedDep};
use crate::graph::Reactive;
use crate::signal::Signal;
use crate::value::SignalValue;

pub(crate) mod index;
mod node;
mod pipeline;
pub(crate) mod slot;
pub(crate) mod tap;

use node::{DerivedInner, DerivedNode};
use pipeline::{EvalFn, Param, ParamSlot, eval_fn, identity};

/// Pending source operations a derived list keeps before it stops recording and rebuilds at its
/// next read (ADR-039 section 4). Fixed: it does not grow with the list, so a derived list nobody
/// reads holds at most this many items.
pub(crate) const TAP_LIMIT: usize = 4096;

/// Derived operations an observed list keeps for its next commit before it sends the full value
/// instead (also where ADR-031's platform mirror drops a merged patch, SPEC 11.1).
pub(crate) const OUT_LIMIT: usize = 4096;

/// Most operations a parameter change is sent as; beyond, the full value. Between the measured
/// break-even points of the platforms (Kotlin about 150 ops, Swift about 300, TypeScript about
/// 2,500; ADR-039 section 4).
pub(crate) const PARAM_WALK_LIMIT: usize = 256;

/// What a debug build says when a pipeline closure is not a pure function of its arguments:
/// `what` is what it did ("read a signal", "wrote a signal", ..).
pub(crate) fn purity_message(what: &str) -> String {
    format!(
        "undra-signals: a derived list's closure {what}. Filter, sort-key and map closures must be \
         pure functions of their arguments: the list re-evaluates a row only when the row changes, \
         so state read any other way leaves rows where an older answer put them. Pass the value as \
         a parameter (`filter_with`, `sort_by_key_with`)."
    )
}

thread_local! {
    /// How many pipeline evaluations are running on this thread (debug builds).
    static DERIVING: Cell<u32> = const { Cell::new(0) };
}

/// Marks a pipeline evaluation as running on this thread until dropped (debug builds; free in
/// release builds).
pub(crate) struct DerivingScope(());

impl DerivingScope {
    pub(crate) fn enter() -> DerivingScope {
        #[cfg(debug_assertions)]
        let _ = DERIVING.try_with(|depth| depth.set(depth.get() + 1));
        DerivingScope(())
    }
}

impl Drop for DerivingScope {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        let _ = DERIVING.try_with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Debug builds: panics with the purity message when a pipeline closure is running on this
/// thread. `what` is "read" or "wrote". Called where reads and writes already pass:
/// `Signal::snapshot` / `read_locked`, every write, `Computed` reads and `DerivedList` reads.
#[inline]
pub(crate) fn assert_not_deriving(what: &str) {
    #[cfg(debug_assertions)]
    {
        let deriving = DERIVING.try_with(Cell::get).unwrap_or(0) > 0;
        assert!(!deriving, "{}", purity_message(&format!("{what} a signal")));
    }
    #[cfg(not(debug_assertions))]
    let _ = what;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Unsorted {}
    impl<K> Sealed for super::Sorted<K> {}
}

/// Whether a pipeline has a sort stage, at the type level: [`Unsorted`] or [`Sorted<K>`]. A
/// pipeline has at most one sort, which the types enforce (`sort_by_key` exists only on an
/// unsorted pipeline).
pub trait Order: sealed::Sealed + Send + Sync + 'static {
    /// The sort key (`()` without a sort stage).
    #[doc(hidden)]
    type Key: Ord + Clone + Send + Sync + 'static;
    /// Whether the view is sorted.
    #[doc(hidden)]
    const SORTED: bool;
}

/// A pipeline without a sort stage: the view keeps source order.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unsorted;

/// A pipeline sorted by a key of type `K`: the view is in key order, equal keys in source order.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sorted<K>(PhantomData<fn() -> K>);

impl Order for Unsorted {
    type Key = ();
    const SORTED: bool = false;
}

impl<K: Ord + Clone + Send + Sync + 'static> Order for Sorted<K> {
    type Key = K;
    const SORTED: bool = true;
}

/// A pipeline over a list signal, waiting for its [`build`](Derive::build) or
/// [`count`](Derive::count): `T` is the source item, `U` the view's item, `O` its order.
///
/// Every closure is `Fn(&U) -> X + Send + Sync + 'static`: it receives the row and nothing else,
/// and **must be a pure function of its arguments**. The list re-evaluates a row only when the row
/// changes, so a closure that reads other state (a signal it captured, a clock, a `Mutex`) leaves
/// rows where an older answer put them. Values the view depends on besides the row are
/// parameters: [`filter_with`](Derive::filter_with), [`sort_by_key_with`](Derive::sort_by_key_with).
/// Debug builds panic when a closure reads or writes a signal, a computed or a derived list.
///
/// The stages apply in the order they are written; a pipeline is fused into one function per row
/// and at most one stable sort.
#[must_use = "a pipeline does nothing until `build()` or `count()`"]
pub struct Derive<T: SignalValue, U: SignalValue = T, O: Order = Unsorted> {
    source: Signal<Vec<T>>,
    eval: Box<EvalFn<T, U, O::Key>>,
    params: Vec<Box<dyn ParamSlot>>,
    _order: PhantomData<O>,
}

impl<T: SignalValue> Signal<Vec<T>> {
    /// Starts a pipeline over this list: a [`DerivedList`] (or a count) kept up to date from the
    /// list's recorded operations at O(log n) per changed row (ADR-039).
    ///
    /// Write the source with the recorded operations (`push`, `insert`, `remove`, `update_at`,
    /// `move_item`, `clear`): a raw write (`set`, `update`, `replace`) cannot be replayed, so it
    /// rebuilds every view of the list (O(n log n)) and sends each one whole.
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Signal;
    ///
    /// let scores = Signal::new(vec![40_u32, 7, 99, 12]);
    /// let high = scores.derive().filter(|s| *s >= 12).sort_by_key(|s| *s).build();
    /// assert_eq!(high.get(), [12, 40, 99]);
    ///
    /// scores.push(50);
    /// scores.update_at(1, |s| *s = 70); // 7 -> 70: it enters the view
    /// assert_eq!(high.get(), [12, 40, 50, 70, 99]);
    /// ```
    pub fn derive(&self) -> Derive<T> {
        Derive {
            source: self.clone(),
            eval: identity(),
            params: Vec::new(),
            _order: PhantomData,
        }
    }
}

impl<T: SignalValue, U: SignalValue, O: Order> Derive<T, U, O> {
    /// Keeps only the rows for which `f` returns `true`.
    ///
    /// `f` must be a pure function of the row (see [`Derive`]).
    pub fn filter(self, f: impl Fn(&U) -> bool + Send + Sync + 'static) -> Self {
        let Derive {
            source,
            eval: prev,
            params,
            _order,
        } = self;
        let eval = eval_fn(move |p, row| {
            let (key, value) = prev(p, row)?;
            f(&value).then_some((key, value))
        });
        Derive {
            source,
            eval,
            params,
            _order,
        }
    }

    /// Keeps only the rows for which `f(&param, row)` returns `true`, where `param` is the current
    /// value of a [`Signal`] or [`Computed`] (several: put them in a `Computed` of a tuple).
    ///
    /// When the parameter changes, the next read or commit re-evaluates every row with the new
    /// value and sends the rows that entered and left the view as one patch, or the full value when
    /// more than 256 rows changed. `f` must read the parameter through its first argument, never by
    /// capturing the signal (see [`Derive`]).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Signal;
    ///
    /// let words = Signal::new(vec![String::from("apple"), String::from("kiwi"), String::from("fig")]);
    /// let min_len = Signal::new(4_u32);
    /// let long = words.derive().filter_with(&min_len, |min, w: &String| w.len() >= *min as usize).build();
    /// assert_eq!(long.get(), ["apple", "kiwi"]);
    /// min_len.set(5);
    /// assert_eq!(long.get(), ["apple"]);
    /// ```
    pub fn filter_with<D: Dep>(
        self,
        param: D,
        f: impl Fn(&D::Value, &U) -> bool + Send + Sync + 'static,
    ) -> Self {
        let Derive {
            source,
            eval: prev,
            mut params,
            _order,
        } = self;
        let slot = params.len();
        params.push(Box::new(Param(param.into_owned())));
        let eval = eval_fn(move |p, row| {
            let (key, value) = prev(p, row)?;
            f(p.get::<D::Value>(slot), &value).then_some((key, value))
        });
        Derive {
            source,
            eval,
            params,
            _order,
        }
    }

    /// Transforms every row that reaches this stage with `f`.
    ///
    /// A map after a sort transforms the row the key was taken from; it does not move it. `f` must
    /// be a pure function of the row (see [`Derive`]).
    ///
    /// ```
    /// use undra_signals::Signal;
    ///
    /// let prices = Signal::new(vec![(1_u32, 250_u64), (2, 900)]);
    /// let labels = prices.derive().map(|(id, cents)| format!("#{id}: {}.{:02}", cents / 100, cents % 100)).build();
    /// assert_eq!(labels.get(), ["#1: 2.50", "#2: 9.00"]);
    /// ```
    pub fn map<V: SignalValue>(
        self,
        f: impl Fn(&U) -> V + Send + Sync + 'static,
    ) -> Derive<T, V, O> {
        let Derive {
            source,
            eval: prev,
            params,
            _order,
        } = self;
        let eval = eval_fn(move |p, row| {
            let (key, value) = prev(p, row)?;
            Some((key, std::borrow::Cow::Owned(f(&value))))
        });
        Derive {
            source,
            eval,
            params,
            _order,
        }
    }

    /// Ends the pipeline: the view, ready to be read from Rust or attached to a store
    /// ([`StoreCell::attach_derived`](crate::StoreCell::attach_derived); a store field declares it
    /// as `#[undra(key = "..")] visible: DerivedList<Item>`).
    ///
    /// Building costs nothing: the index is built by the first read or observe.
    pub fn build(self) -> DerivedList<U> {
        let node = DerivedInner::build(self.source, self.eval, self.params, O::SORTED);
        DerivedList { inner: node }
    }

    /// Ends the pipeline with only the length of its view: a [`Computed<u32>`] kept current as rows
    /// enter and leave it, O(log n) per changed row and O(1) per read, with no scan of the list.
    /// (Sort and map stages do not change a count; they are evaluated all the same.)
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Signal;
    ///
    /// let done = Signal::new(vec![true, false, false]);
    /// let remaining = done.derive().filter(|d| !*d).count();
    /// assert_eq!(remaining.get(), 2);
    /// done.update_at(1, |d| *d = true);
    /// assert_eq!(remaining.get(), 1);
    /// ```
    pub fn count(self) -> Computed<u32> {
        let Derive {
            source,
            eval: prev,
            params,
            ..
        } = self;
        let eval = eval_fn(move |p, row| prev(p, row).map(|_| ((), std::borrow::Cow::Owned(()))));
        let node: Arc<DerivedInner<T, (), ()>> = DerivedInner::build(source, eval, params, false);
        Computed::from_compute(Box::new(Count { node }))
    }
}

impl<T: SignalValue, U: SignalValue> Derive<T, U, Unsorted> {
    /// Sorts the view by `f(row)`, stably: rows with equal keys keep their source order (what
    /// `Vec::sort_by_key` does).
    ///
    /// A row whose key changes moves: the host receives a `Move` and an `Update`, so SwiftUI and
    /// Compose animate it and React keeps its component. `K`'s `Ord` must be a total order; an
    /// inconsistent one can misplace rows but never lose or duplicate one. `f` must be a pure
    /// function of the row (see [`Derive`]).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Signal;
    ///
    /// let names = Signal::new(vec!["cy", "ab", "bo"]);
    /// let sorted = names.derive().sort_by_key(|n| *n).build();
    /// assert_eq!(sorted.get(), ["ab", "bo", "cy"]);
    /// names.update_at(0, |n| *n = "aa"); // a Move to the front, then an Update
    /// assert_eq!(sorted.get(), ["aa", "ab", "bo"]);
    /// ```
    pub fn sort_by_key<K: Ord + Clone + Send + Sync + 'static>(
        self,
        f: impl Fn(&U) -> K + Send + Sync + 'static,
    ) -> Derive<T, U, Sorted<K>> {
        let Derive {
            source,
            eval: prev,
            params,
            ..
        } = self;
        let eval = eval_fn(move |p, row| {
            let ((), value) = prev(p, row)?;
            let key = f(&value);
            Some((key, value))
        });
        Derive {
            source,
            eval,
            params,
            _order: PhantomData,
        }
    }

    /// Sorts the view by `f(&param, row)`, stably, where `param` is the current value of a
    /// [`Signal`] or [`Computed`] (a sort direction, a column). A parameter change re-walks the
    /// rows and sends the moves, or the full value past 256 of them; see
    /// [`filter_with`](Derive::filter_with).
    pub fn sort_by_key_with<D: Dep, K: Ord + Clone + Send + Sync + 'static>(
        self,
        param: D,
        f: impl Fn(&D::Value, &U) -> K + Send + Sync + 'static,
    ) -> Derive<T, U, Sorted<K>> {
        let Derive {
            source,
            eval: prev,
            mut params,
            ..
        } = self;
        let slot = params.len();
        params.push(Box::new(Param(param.into_owned())));
        let eval = eval_fn(move |p, row| {
            let ((), value) = prev(p, row)?;
            let key = f(p.get::<D::Value>(slot), &value);
            Some((key, value))
        });
        Derive {
            source,
            eval,
            params,
            _order: PhantomData,
        }
    }
}

/// The node behind [`Derive::count`].
struct Count<T: SignalValue> {
    node: Arc<DerivedInner<T, (), ()>>,
}

impl<T: SignalValue> Compute<u32> for Count<T> {
    fn run(&self) -> u32 {
        u32::try_from(self.node.view_len()).unwrap_or(u32::MAX)
    }

    fn subscribe(&self, dependent: Weak<dyn Reactive>) {
        self.node.add_dependent(dependent);
    }
}

/// Counters of a [`DerivedList`]: what its maintenance did, for tests and devtools.
///
/// A list whose source is written only with the recorded operations rebuilds once (its first read
/// or observe) and is then sent as patches; `rebuilds` and `full_values` that keep growing point
/// at raw writes (`set`, `update`, `replace`) of the source or at parameter changes that move more
/// than 256 rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DerivedStats {
    /// Times the index was rebuilt from the whole source (first use, a raw write of the source,
    /// more than 4,096 operations nobody read, a closure that panicked).
    pub rebuilds: u64,
    /// Full values sent to the host (observe, and commits that could not send a patch).
    pub full_values: u64,
    /// Patches sent to the host.
    pub patches: u64,
    /// Operations in those patches.
    pub ops_emitted: u64,
}

/// A filtered, sorted and mapped view of a `Signal<Vec<T>>`, kept up to date from the source's
/// recorded operations (ADR-039). Built with [`Signal::derive`].
///
/// [`Clone`] gives another handle to the same list. `Send + Sync`.
///
/// * **Reads** ([`len`](DerivedList::len), [`get`](DerivedList::get),
///   [`with`](DerivedList::with)) first replay the source's operations recorded since the last
///   read, O(log n) each. `len` is then O(1); `get` and `with` materialise the view, O(n) once and
///   cached until it changes. A read sees the source as it is now, as a [`Signal`] read does:
///   inside a transaction that includes the writes this transaction already made (they are
///   recorded as they happen, not at the commit), and the host receives them all at the commit.
///   Writers never wait for a read's closures: the pipeline runs under the list's own lock, which
///   no writer takes, and the read holds the source's lock only to take its operations and a
///   snapshot.
/// * **In a store** (`#[undra(key = "id")] visible: DerivedList<Todo>`, or
///   [`StoreCell::attach_derived`](crate::StoreCell::attach_derived)) the host receives the view
///   once in full, then keyed patches: one source operation is at most two ops of the view (a
///   sort-key change is a `Move` and an `Update`), the operations of a transaction are one patch,
///   and an operation that does not change the view sends nothing. The platforms generate it as a
///   read-only keyed list, exactly the property a `Computed<Vec<T>>` had.
/// * **Raw writes** of the source (`set`, `update`, `replace`) cannot be replayed: they rebuild the
///   index (O(n log n)) and send the full value. Write the source with the recorded operations.
/// * A [`Computed`] or [`Effect`](crate::Effect) can depend on it (`Computed::new(&visible, ..)`),
///   which materialises it.
///
/// A derived list is not itself the source of another derived list in v1: chain the stages in one
/// pipeline instead.
///
/// # Example
///
/// ```
/// use undra_signals::Signal;
///
/// #[derive(Clone, Debug, PartialEq)]
/// struct Todo { id: u32, title: String, done: bool }
/// impl undra_wire::Encode for Todo {
///     fn encode(&self, w: &mut undra_wire::Writer) {
///         self.id.encode(w);
///         self.title.encode(w);
///         self.done.encode(w);
///     }
/// }
/// let todo = |id, title: &str| Todo { id, title: title.into(), done: false };
///
/// let todos = Signal::new(vec![todo(1, "milk"), todo(2, "bread")]);
/// let open = todos.derive().filter(|t: &Todo| !t.done).build();
/// assert_eq!(open.len(), 2);
///
/// todos.update_at(0, |t| t.done = true); // the view's ops: one Remove
/// todos.push(todo(3, "eggs"));           // one Insert
/// let titles: Vec<String> = open.with(|rows| rows.iter().map(|t| t.title.clone()).collect());
/// assert_eq!(titles, ["bread", "eggs"]);
/// ```
pub struct DerivedList<T> {
    pub(crate) inner: Arc<dyn DerivedNode<T>>,
}

impl<T> Clone for DerivedList<T> {
    fn clone(&self) -> Self {
        DerivedList {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: SignalValue> DerivedList<T> {
    /// Rows in the view. O(1) once the source's recorded operations are replayed (O(log n) each).
    pub fn len(&self) -> usize {
        self.check_read();
        self.inner.view_len()
    }

    /// Whether the view has no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a clone of the view. Materialising is O(n), cached until the view changes.
    pub fn get(&self) -> Vec<T> {
        self.check_read();
        Vec::clone(&self.inner.materialise())
    }

    /// Calls `f` with the view, without cloning it. No lock is held while `f` runs.
    pub fn with<R>(&self, f: impl FnOnce(&Vec<T>) -> R) -> R {
        self.check_read();
        f(&self.inner.materialise())
    }

    /// Returns `true` if `self` and `other` are handles to the same list.
    pub fn ptr_eq(&self, other: &DerivedList<T>) -> bool {
        self.inner.id() == other.inner.id()
    }

    /// Returns `true` once the list has been attached to a store.
    pub fn is_attached(&self) -> bool {
        self.inner.binding().get().is_some()
    }

    /// What maintaining the list has done so far.
    pub fn stats(&self) -> DerivedStats {
        self.inner.stats()
    }

    /// The materialised view, shared (for [`Dep`]).
    fn shared(&self) -> Arc<Vec<T>> {
        self.check_read();
        self.inner.materialise()
    }

    /// Debug builds: a pipeline closure must not read a derived list.
    fn check_read(&self) {
        // A closure that reads its own list gets the cycle message in every build, before the
        // purity check of a debug build, which says less precisely what to fix.
        node::check_cycle(self.inner.id());
        assert_not_deriving("read");
    }
}

impl<T: SignalValue> fmt::Debug for DerivedList<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never drains from `Debug`: that would run pipeline closures from a formatter.
        f.debug_struct("DerivedList")
            .field("stats", &self.stats())
            .field("attached", &self.is_attached())
            .finish()
    }
}

impl<T: SignalValue> Dep for &DerivedList<T> {
    type Value = Vec<T>;
    type Owned = DerivedList<T>;
    fn into_owned(self) -> DerivedList<T> {
        self.clone()
    }
}

impl<T: SignalValue> OwnedDep for DerivedList<T> {
    type Value = Vec<T>;
    fn subscribe(&self, dependent: Weak<dyn Reactive>) {
        self.inner.add_dependent(dependent);
    }
    fn with_value<R>(&self, f: impl FnOnce(&Vec<T>) -> R) -> R {
        f(&self.shared())
    }
    fn snapshot(&self) -> Arc<Vec<T>> {
        self.shared()
    }
}

#[cfg(test)]
mod tests;
