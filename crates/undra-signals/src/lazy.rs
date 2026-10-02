//! [`Lazy`]: a list the host pages through instead of mirroring (ADR-043 decision 3).
//!
//! A `Signal<Vec<T>>` reaches the host whole (or as keyed patches); a 100,000-row table should not.
//! A `Lazy<T>` is a core-owned list that **never crosses the boundary as a value**: the host gets
//! its length and a *version* (change-set op 0, `LazyValue`), asks for the window it shows with a
//! page call (`LazyPage`: a page of 50 rows is encoded on request, microseconds), and is told that
//! something changed by an `LazyInvalidated` entry of 12 bytes (op 2) whatever the change was,
//! after which it asks for its window again. Nothing the host does not show is ever encoded.
//!
//! * [`Lazy::new`] / [`Lazy::from_vec`] make an **owned** list with the recorded API of a
//!   `Signal<Vec<T>>` ([`push`](Lazy::push), [`insert`](Lazy::insert), [`remove`](Lazy::remove),
//!   [`update_at`](Lazy::update_at), [`move_item`](Lazy::move_item), [`clear`](Lazy::clear),
//!   [`replace`](Lazy::replace), [`len`](Lazy::len), [`with_range`](Lazy::with_range)). Items stay
//!   typed in the core. [`version`](Lazy::version) increases with every change.
//! * [`Lazy::over`] makes a **read-only view** of a [`DerivedList`](crate::DerivedList) that pages
//!   through the derived list's index (O(log n + limit) per page) and is announced to the host
//!   when the view changes.
//! * [`LazySource`] is the type-erased page server both implement, and what the runtime keeps in
//!   its object table for the host's page calls.
//!
//! A `Lazy<T>` is a legal `#[undra::store]` field (`#[undra(key = "id")]` optional) and is attached
//! with [`StoreCell::attach_lazy`](crate::StoreCell::attach_lazy).
//!
//! ```
//! use undra_signals::{Lazy, LazySource};
//! use undra_wire::Writer;
//!
//! let books = Lazy::from_vec((0..1_000_u32).collect());
//! books.push(1_000);
//!
//! // What a page call is answered with: the version and total the page was read at, then the rows.
//! let mut rows = Writer::new();
//! let page = books.encode_page(10, 3, &mut rows);
//! assert_eq!((page.total, page.count, page.version), (1_001, 3, 1));
//! assert_eq!(rows.as_slice(), [10, 0, 0, 0, 11, 0, 0, 0, 12, 0, 0, 0]);
//! ```

use std::any::Any;
use std::fmt;
use std::ops::{Bound, Range, RangeBounds};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::Mutex;
use undra_wire::payload::{LazyInvalidated, LazyPage, LazyValue};
use undra_wire::{Encode, Handle, Writer};

use crate::derived::DerivedList;
use crate::derived::node::{DerivedNode, check_cycle};
use crate::derived::slot::{DerivedSlot, Emitted};
use crate::graph::{Binding, Reactive, record};
use crate::oplog::move_within;
use crate::signal::Signal;
use crate::store::StoreCell;
use crate::value::SignalValue;

/// A list a host pages through: what the runtime's object table serves `LazyPage` calls from
/// (ADR-043).
///
/// [`Lazy`] implements it (so does the runtime's `LazyList` of pre-encoded items). A page server
/// must be cheap to ask: the runtime calls [`encode_page`](LazySource::encode_page) for every page
/// request, on the core thread, under the panic guard.
///
/// # Example
///
/// ```
/// use undra_signals::{Lazy, LazySource};
///
/// let list = Lazy::from_vec(vec![String::from("a"), String::from("b")]);
/// let source: &dyn LazySource = &list;
/// assert_eq!(source.stamp(), (2, 0));
/// list.push(String::from("c"));
/// assert_eq!((source.len(), source.version()), (3, 1));
/// ```
pub trait LazySource: Send + Sync + 'static {
    /// The number of items and the version they belong to, read as one pair: the version increases
    /// with every change of the list, so a host that holds `(len, version)` knows whether what it
    /// has is current.
    fn stamp(&self) -> (usize, u64);

    /// The number of items.
    fn len(&self) -> usize {
        self.stamp().0
    }

    /// Whether there are no items.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The version of the list: it increases with every change.
    fn version(&self) -> u64 {
        self.stamp().1
    }

    /// Appends to `out` the items `offset .. offset + limit` (clamped to the list: see
    /// [`page_window`]), each encoded as the item type, and returns the page header: the
    /// `version` and `total` the page was read at, atomically with the items, and the `count` of
    /// items written. Past the end `count` is `0` and nothing is written.
    ///
    /// The runtime puts the header (`version u64, total u32, count u32`) in front of the items to
    /// make the reply of a page call (SPEC 3.3) and clamps `limit` before it gets here.
    fn encode_page(&self, offset: u32, limit: u32, out: &mut Writer) -> LazyPage;
}

/// The items a page call asks for, as indices into a list of `len` items: `offset .. offset +
/// limit` clamped to `0..len`. An `offset` at or past the end gives the empty window at `len`; a
/// `limit` that would pass the end is cut; nothing overflows whatever the arguments are.
///
/// ```
/// use undra_signals::page_window;
///
/// assert_eq!(page_window(100, 10, 50), 10..60);
/// assert_eq!(page_window(100, 90, 50), 90..100);
/// assert_eq!(page_window(100, 100, 50), 100..100);
/// assert_eq!(page_window(100, u32::MAX, u32::MAX), 100..100);
/// assert_eq!(page_window(100, 5, 0), 5..5);
/// ```
#[must_use]
pub fn page_window(len: usize, offset: u32, limit: u32) -> Range<usize> {
    let start = (offset as usize).min(len);
    let end = start.saturating_add(limit as usize).min(len);
    start..end
}

/// A count as the wire carries it: saturating at `u32::MAX` (a list that long cannot be paged
/// anyway).
fn wire_count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A range of a list as indices, clamped to `0..len` (and empty when it is inverted).
fn clamp_range(range: &impl RangeBounds<usize>, len: usize) -> Range<usize> {
    let start = match range.start_bound() {
        Bound::Included(&n) => n,
        Bound::Excluded(&n) => n.saturating_add(1),
        Bound::Unbounded => 0,
    }
    .min(len);
    let end = match range.end_bound() {
        Bound::Included(&n) => n.saturating_add(1),
        Bound::Excluded(&n) => n,
        Bound::Unbounded => len,
    }
    .min(len);
    start..end.max(start)
}

/// The items of an owned [`Lazy`] and its version, one value so that a reader sees both at once.
#[derive(Clone)]
pub(crate) struct LazyState<T> {
    items: Vec<T>,
    version: u64,
}

impl<T: Encode> Encode for LazyState<T> {
    /// A snapshot carries the items as a `Vec<T>` (ADR-043 decision 3.4).
    fn encode(&self, w: &mut Writer) {
        self.items.encode(w);
    }
}

/// A read-only lazy view of a derived list.
struct LazyView<T> {
    node: Arc<dyn DerivedNode<T>>,
    /// Where the view stands in a store: the derived list marks it dirty when the source changes.
    binding: OnceLock<Binding>,
}

impl<T: Send + Sync + 'static> Reactive for LazyView<T> {
    fn invalidate(self: Arc<Self>, _pass: u64) {
        if let Some(binding) = self.binding.get() {
            record(binding);
        }
    }
}

enum Repr<T> {
    Owned(Signal<LazyState<T>>),
    View(Arc<LazyView<T>>),
}

/// A core-owned list the host pages through (ADR-043): see "Lazy lists" in the crate documentation.
///
/// [`Clone`] gives another handle to the **same** list, as for a [`Signal`]. Writes follow the
/// rules of a signal's (ADR-035): outside a [`txn`](crate::txn) each is a transaction of its own,
/// and a list that belongs to a store may only be written on its runtime's core (E0065 otherwise;
/// [`can_write`](Lazy::can_write) asks). The recorded operations never walk the list: a
/// `push` is O(1), an `insert` or `remove` costs the `memmove` a `Vec` needs, and what the host is
/// sent is the same 12 bytes whatever the operation was and however many a transaction made.
///
/// A list made with [`Lazy::over`] is **read-only**: its write methods panic; change the list it
/// is derived from.
///
/// # Snapshots
///
/// An owned list is store state: a snapshot carries its items (encoded as the `Vec<T>` of them),
/// and a restore rebuilds it. A [`view`](Lazy::over) is derived data and is not persisted (its
/// snapshot value is an empty list): the store's restore hook rebuilds it from the list it is
/// derived from.
///
/// # Example
///
/// ```
/// use undra_signals::Lazy;
///
/// let books = Lazy::from_vec(vec![String::from("Dune"), String::from("Emma")]);
/// books.push(String::from("Ulysses"));
/// books.update_at(0, |title| title.push_str(" (1965)"));
/// assert_eq!(books.len(), 3);
/// assert_eq!(books.with_range(..2, |rows| rows.to_vec()), ["Dune (1965)", "Emma"]);
/// assert_eq!(books.version(), 2); // one per change
/// ```
pub struct Lazy<T> {
    repr: Repr<T>,
}

impl<T> Clone for Lazy<T> {
    fn clone(&self) -> Self {
        Lazy {
            repr: match &self.repr {
                Repr::Owned(signal) => Repr::Owned(signal.clone()),
                Repr::View(view) => Repr::View(Arc::clone(view)),
            },
        }
    }
}

impl<T: SignalValue> Default for Lazy<T> {
    fn default() -> Self {
        Lazy::new()
    }
}

impl<T: SignalValue> Lazy<T> {
    /// An empty list.
    #[must_use]
    pub fn new() -> Lazy<T> {
        Lazy::from_vec(Vec::new())
    }

    /// A list holding `items`, at version `0`.
    #[must_use]
    pub fn from_vec(items: Vec<T>) -> Lazy<T> {
        Lazy {
            repr: Repr::Owned(Signal::new(LazyState { items, version: 0 })),
        }
    }

    /// A read-only lazy view of `list`: the host pages through the derived list's index (the
    /// `k`-th row is found in O(log n), a page of `limit` rows costs O(log n + limit) however long
    /// the view is) and is told when the view changes, so a filtered, sorted 100,000-row table
    /// reaches the platform one window at a time.
    ///
    /// The view reads the derived list as it is committed: a page asked for while the index is
    /// behind the source (writes since the last read) first replays the source's recorded
    /// operations, so it is always correct. Its [`version`](Lazy::version) moves whenever the view
    /// may have changed (a write to the source that the view's filter ignores does not announce).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::{Lazy, Signal};
    ///
    /// let numbers = Signal::new((0..10_u32).collect::<Vec<_>>());
    /// let even = numbers.derive().filter(|n| n % 2 == 0).sort_by_key(|n| std::cmp::Reverse(*n)).build();
    /// let view = Lazy::over(&even);
    /// assert_eq!(view.with_range(..3, |rows| rows.to_vec()), [8, 6, 4]);
    ///
    /// numbers.push(10);
    /// assert_eq!(view.len(), 6);
    /// assert_eq!(view.with_range(..1, |rows| rows.to_vec()), [10]);
    /// ```
    #[must_use]
    pub fn over(list: &DerivedList<T>) -> Lazy<T> {
        let view = Arc::new(LazyView {
            node: Arc::clone(&list.inner),
            binding: OnceLock::new(),
        });
        let weak: Weak<dyn Reactive> = Arc::downgrade(&view) as Weak<dyn Reactive>;
        view.node.add_dependent(weak);
        Lazy {
            repr: Repr::View(view),
        }
    }

    /// Whether this is a read-only view made with [`Lazy::over`].
    #[must_use]
    pub fn is_view(&self) -> bool {
        matches!(self.repr, Repr::View(_))
    }

    /// The number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stamp().0
    }

    /// Whether the list has no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The version: `0` for a new list, increased by every change (every write method, and for a
    /// view every change of the derived list's view). What a page was read at, what `observe`
    /// and every `LazyInvalidated` entry carry.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.stamp().1
    }

    /// Calls `f` with the items `range` (clamped to the list; an inverted range is empty), without
    /// cloning an owned list. For a view the rows are read through the derived list's index (O(log
    /// n + rows)) and handed over as a slice of clones.
    ///
    /// No lock is held while `f` runs: it may write the list (it keeps the snapshot it started
    /// with).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Lazy;
    ///
    /// let list = Lazy::from_vec(vec![1, 2, 3, 4]);
    /// assert_eq!(list.with_range(1..3, |rows| rows.iter().sum::<i32>()), 5);
    /// assert_eq!(list.with_range(3.., |rows| rows.len()), 1);
    /// assert_eq!(list.with_range(9.., |rows| rows.len()), 0);
    /// ```
    pub fn with_range<R>(&self, range: impl RangeBounds<usize>, f: impl FnOnce(&[T]) -> R) -> R {
        match &self.repr {
            Repr::Owned(signal) => {
                let state = signal.snapshot();
                let window = clamp_range(&range, state.items.len());
                f(&state.items[window])
            }
            Repr::View(view) => {
                view.check_read();
                let (total, _) = view.node.view_stamp();
                let window = clamp_range(&range, total);
                let mut rows = Vec::with_capacity(window.len());
                view.node.page(window.start, window.len(), &mut |row| {
                    rows.push(row.clone())
                });
                f(&rows)
            }
        }
    }

    /// A clone of the item at `index`, if there is one.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<T> {
        self.with_range(index..index.saturating_add(1), |rows| rows.first().cloned())
    }

    /// A clone of every item: O(n), for tests and small lists (a host pages instead).
    #[must_use]
    pub fn to_vec(&self) -> Vec<T> {
        self.with_range(.., <[T]>::to_vec)
    }

    /// Returns `true` if `self` and `other` are handles to the same list.
    #[must_use]
    pub fn ptr_eq(&self, other: &Lazy<T>) -> bool {
        match (&self.repr, &other.repr) {
            (Repr::Owned(a), Repr::Owned(b)) => a.ptr_eq(b),
            (Repr::View(a), Repr::View(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Returns `true` once the list has been attached to a store.
    #[must_use]
    pub fn is_attached(&self) -> bool {
        self.binding().get().is_some()
    }

    /// Whether the calling thread may write this list right now (ADR-035; always `false` for a
    /// view).
    #[must_use]
    pub fn can_write(&self) -> bool {
        match &self.repr {
            Repr::Owned(signal) => signal.can_write(),
            Repr::View(_) => false,
        }
    }

    /// Appends `item`: O(1), version +1.
    ///
    /// # Panics
    ///
    /// On a [view](Lazy::over); with E0065 when the list belongs to a store and the calling
    /// thread does not hold its runtime's core lock (as [`Signal::set`] does).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Lazy;
    ///
    /// let list = Lazy::new();
    /// list.push("a");
    /// assert_eq!((list.len(), list.version()), (1, 1));
    /// ```
    pub fn push(&self, item: T) {
        self.owned("push").update(|state| {
            state.items.push(item);
            state.version += 1;
        });
    }

    /// Inserts `item` so that it ends up at `index`, shifting the items after it.
    ///
    /// # Panics
    ///
    /// If `index > len`, as `Vec::insert` does (the list is left unchanged); as [`push`](Lazy::push).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Lazy;
    ///
    /// let list = Lazy::from_vec(vec![1, 3]);
    /// list.insert(1, 2);
    /// assert_eq!(list.to_vec(), [1, 2, 3]);
    /// ```
    pub fn insert(&self, index: usize, item: T) {
        self.owned("insert").update(|state| {
            let len = state.items.len();
            assert!(
                index <= len,
                "undra-signals: Lazy::insert at index {index}, but the list has {len} item(s)"
            );
            state.version += 1;
            state.items.insert(index, item);
        });
    }

    /// Removes and returns the item at `index`, shifting the items after it.
    ///
    /// # Panics
    ///
    /// If `index >= len` (the list is left unchanged); as [`push`](Lazy::push).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Lazy;
    ///
    /// let list = Lazy::from_vec(vec![10_u32, 20, 30]);
    /// assert_eq!(list.remove(1), 20);
    /// assert_eq!(list.to_vec(), [10, 30]);
    /// ```
    pub fn remove(&self, index: usize) -> T {
        self.owned("remove").update_with(|state| {
            let len = state.items.len();
            assert!(
                index < len,
                "undra-signals: Lazy::remove at index {index}, but the list has {len} item(s)"
            );
            state.version += 1;
            state.items.remove(index)
        })
    }

    /// Changes the item at `index` in place with `f`. Like [`Signal::update`], `f` runs with the
    /// list write-locked: it must not read or write this list (that is detected and panics instead
    /// of deadlocking). If `f` panics the item keeps what `f` did and the version has moved, so the
    /// host is told.
    ///
    /// # Panics
    ///
    /// If `index >= len` (the list is left unchanged); as [`push`](Lazy::push).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Lazy;
    ///
    /// let list = Lazy::from_vec(vec![String::from("a"), String::from("b")]);
    /// list.update_at(1, |item| item.push('!'));
    /// assert_eq!(list.to_vec(), ["a", "b!"]);
    /// ```
    pub fn update_at(&self, index: usize, f: impl FnOnce(&mut T)) {
        self.owned("update_at").update(|state| {
            let len = state.items.len();
            assert!(
                index < len,
                "undra-signals: Lazy::update_at at index {index}, but the list has {len} item(s)"
            );
            // Before `f` runs: a panic inside it must still announce what it changed.
            state.version += 1;
            f(&mut state.items[index]);
        });
    }

    /// Takes the item at `from` out of the list and puts it back so that it ends up at `to`.
    /// `from == to` changes nothing (and does not move the version).
    ///
    /// # Panics
    ///
    /// If `from >= len` or `to >= len` (the list is left unchanged); as [`push`](Lazy::push).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Lazy;
    ///
    /// let list = Lazy::from_vec(vec![1_u32, 2, 3, 4]);
    /// list.move_item(0, 2);
    /// assert_eq!(list.to_vec(), [2, 3, 1, 4]);
    /// ```
    pub fn move_item(&self, from: usize, to: usize) {
        self.owned("move_item").update(|state| {
            let len = state.items.len();
            assert!(
                from < len && to < len,
                "undra-signals: Lazy::move_item from {from} to {to}, but the list has {len} item(s)"
            );
            if from != to {
                state.version += 1;
                move_within(state.items.as_mut_slice(), from, to);
            }
        });
    }

    /// Removes every item. Clearing an empty list changes nothing (and does not move the version).
    ///
    /// The removed items are dropped after the list's lock is released.
    ///
    /// # Panics
    ///
    /// As [`push`](Lazy::push).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Lazy;
    ///
    /// let list = Lazy::from_vec(vec![1, 2, 3]);
    /// list.clear();
    /// assert!(list.is_empty());
    /// ```
    pub fn clear(&self) {
        let owned = self.owned("clear");
        // Checked first so that clearing nothing is not a write (no announcement, no version).
        if owned.with(|state| state.items.is_empty()) {
            return;
        }
        let _removed = owned.update_with(|state| {
            state.version += 1;
            std::mem::take(&mut state.items)
        });
    }

    /// Replaces the whole list with `items`: O(1) here and nothing but the 12 bytes of an
    /// invalidation to the host (the old items are dropped after the lock is released).
    ///
    /// # Panics
    ///
    /// As [`push`](Lazy::push).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::Lazy;
    ///
    /// let list = Lazy::from_vec(vec![1, 2]);
    /// list.replace(vec![7, 8, 9]);
    /// assert_eq!(list.to_vec(), [7, 8, 9]);
    /// ```
    pub fn replace(&self, items: Vec<T>) {
        let _old = self.owned("replace").update_with(|state| {
            state.version += 1;
            std::mem::replace(&mut state.items, items)
        });
    }

    /// The signal behind an owned list; a view has none to write.
    fn owned(&self, op: &str) -> &Signal<LazyState<T>> {
        match &self.repr {
            Repr::Owned(signal) => signal,
            Repr::View(_) => panic!(
                "undra-signals: Lazy::{op} on a read-only view made with Lazy::over. A view shows a \
                 derived list: write the list it is derived from"
            ),
        }
    }

    /// Where the list stands in a store.
    pub(crate) fn binding(&self) -> &OnceLock<Binding> {
        match &self.repr {
            Repr::Owned(signal) => &signal.inner.binding,
            Repr::View(view) => &view.binding,
        }
    }

    /// The value a snapshot carries (ADR-043 decision 3.4): an owned list's items as a `Vec<T>`,
    /// an empty list for a view (derived data, rebuilt by the store's restore hook).
    pub(crate) fn encode_snapshot(&self, w: &mut Writer) {
        match &self.repr {
            Repr::Owned(signal) => signal.snapshot().encode(w),
            Repr::View(_) => w.write_len(0),
        }
    }
}

impl<T> LazyView<T> {
    /// A pipeline closure must not read a lazy view of its own list (a cycle) or any signal.
    fn check_read(&self) {
        check_cycle(self.node.id());
        crate::derived::assert_not_deriving("read");
    }
}

impl<T: SignalValue> LazySource for Lazy<T> {
    fn stamp(&self) -> (usize, u64) {
        match &self.repr {
            Repr::Owned(signal) => {
                let state = signal.snapshot();
                (state.items.len(), state.version)
            }
            Repr::View(view) => {
                view.check_read();
                view.node.view_stamp()
            }
        }
    }

    fn encode_page(&self, offset: u32, limit: u32, out: &mut Writer) -> LazyPage {
        match &self.repr {
            Repr::Owned(signal) => {
                // Items and version are one value: the page is read at one version.
                let state = signal.snapshot();
                let window = page_window(state.items.len(), offset, limit);
                for item in &state.items[window.clone()] {
                    item.encode(out);
                }
                LazyPage {
                    version: state.version,
                    total: wire_count(state.items.len()),
                    count: wire_count(window.len()),
                }
            }
            Repr::View(view) => {
                view.check_read();
                let (total, version, count) =
                    view.node
                        .page(offset as usize, limit as usize, &mut |row| row.encode(out));
                LazyPage {
                    version,
                    total: wire_count(total),
                    count: wire_count(count),
                }
            }
        }
    }
}

impl<T: SignalValue> fmt::Debug for Lazy<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Lazy");
        match &self.repr {
            // Never drains from `Debug`: that would run pipeline closures from a formatter.
            Repr::View(_) => s.field("view", &true),
            Repr::Owned(signal) => {
                let state = signal.snapshot();
                s.field("len", &state.items.len())
                    .field("version", &state.version)
            }
        };
        s.field("attached", &self.is_attached()).finish()
    }
}

// ---------------------------------------------------------------------------------------------
// The store slot
// ---------------------------------------------------------------------------------------------

/// How the runtime registers the page servers of a store's lazy lists (ADR-043), set on the store's
/// cell by [`StoreCell::set_lazy_hooks`](crate::StoreCell::set_lazy_hooks).
///
/// The runtime's object table calls them with itself as `table` (an `&dyn Any` so that this crate
/// knows nothing of the table): `register` when the store enters the table (a new store, or one a
/// restore places), `unregister` when the store leaves it. A core whose stores have no `Lazy` field
/// never sets them, which is what keeps the page-server code out of its binary.
#[derive(Clone, Copy)]
pub struct LazyHooks {
    /// The store entered `table` with `handle`: register a page server for each of
    /// [`cell.lazy_sources()`](crate::StoreCell::lazy_sources) and tell the cell the handles.
    pub register: fn(table: &dyn Any, cell: &Arc<StoreCell>, handle: u64),
    /// The store left `table`: remove its page servers.
    pub unregister: fn(table: &dyn Any, cell: &StoreCell),
}

impl fmt::Debug for LazyHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LazyHooks")
    }
}

/// The state a store keeps for one `Lazy` slot: a [derived slot](DerivedSlot) that announces the
/// list's length and version instead of sending patches.
pub(crate) struct LazyCell {
    source: Arc<dyn LazySource>,
    /// The page server's handle in the runtime's object table (`0` until it is registered).
    handle: AtomicU64,
    /// The `(len, version)` the host was last told, by op 0 or op 2: a commit whose list is at the
    /// same stamp (a view whose source changed without changing the view) sends nothing.
    announced: Mutex<Option<(usize, u64)>>,
}

impl LazyCell {
    pub(crate) fn new(source: Arc<dyn LazySource>) -> LazyCell {
        LazyCell {
            source,
            handle: AtomicU64::new(0),
            announced: Mutex::new(None),
        }
    }

    /// The page server the runtime registers for the host.
    pub(crate) fn source(&self) -> Arc<dyn LazySource> {
        Arc::clone(&self.source)
    }

    pub(crate) fn handle(&self) -> u64 {
        self.handle.load(Ordering::SeqCst)
    }

    pub(crate) fn set_handle(&self, handle: u64) {
        self.handle.store(handle, Ordering::SeqCst);
    }

    /// Writes the signal's value (`LazyValue`): the page server, the length and the version.
    fn write_value(&self, len: usize, version: u64, w: &mut Writer) {
        LazyValue {
            handle: Handle(self.handle()),
            len: wire_count(len),
            version,
        }
        .encode(w);
    }
}

impl DerivedSlot for LazyCell {
    /// What a commit sends: 12 bytes of length and version, or nothing when the host already knows
    /// them. `retain == false` (delivered only because it is `no_coalesce`, unobserved): the value.
    fn commit(&self, w: &mut Writer, retain: bool) -> Emitted {
        let (len, version) = self.source.stamp();
        if !retain {
            self.write_value(len, version, w);
            return Emitted::Full;
        }
        let mut announced = self.announced.lock();
        if *announced == Some((len, version)) {
            return Emitted::Nothing;
        }
        *announced = Some((len, version));
        // The whole entry in one allocation of the commit's scratch buffer, not two.
        w.reserve(12);
        LazyInvalidated {
            len: wire_count(len),
            version,
        }
        .encode(w);
        Emitted::Invalidated
    }

    /// `observe(on)`: the value, remembering that the host was told it.
    fn resync(&self, w: &mut Writer) {
        let (len, version) = self.source.stamp();
        *self.announced.lock() = Some((len, version));
        self.write_value(len, version, w);
    }

    /// The host may not have what was last announced (unobserved, or a delivery was abandoned): the
    /// next commit announces again.
    fn forget(&self) {
        *self.announced.lock() = None;
    }

    fn persisted(&self) -> bool {
        true
    }

    fn lazy(&self) -> Option<&LazyCell> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    fn list(n: u32) -> Lazy<u32> {
        Lazy::from_vec((0..n).collect())
    }

    fn page(source: &dyn LazySource, offset: u32, limit: u32) -> (LazyPage, Vec<u8>) {
        let mut rows = Writer::new();
        let header = source.encode_page(offset, limit, &mut rows);
        (header, rows.into_vec())
    }

    #[test]
    fn window_clamps_to_the_list_and_never_overflows() {
        assert_eq!(page_window(10, 0, 4), 0..4);
        assert_eq!(page_window(10, 8, 4), 8..10);
        assert_eq!(page_window(10, 10, 4), 10..10);
        assert_eq!(page_window(10, 11, 4), 10..10);
        assert_eq!(page_window(0, 0, 5), 0..0);
        assert_eq!(page_window(10, 3, 0), 3..3);
        assert_eq!(page_window(10, u32::MAX, u32::MAX), 10..10);
        assert_eq!(page_window(10, 1, u32::MAX), 1..10);
        assert_eq!(page_window(usize::MAX, 5, 7), 5..12);
    }

    #[test]
    fn clamp_range_follows_every_kind_of_bound() {
        assert_eq!(clamp_range(&(..), 5), 0..5);
        assert_eq!(clamp_range(&(1..3), 5), 1..3);
        assert_eq!(clamp_range(&(1..=3), 5), 1..4);
        assert_eq!(clamp_range(&(3..), 5), 3..5);
        assert_eq!(clamp_range(&(..=9), 5), 0..5);
        assert_eq!(clamp_range(&(7..9), 5), 5..5);
        assert_eq!(
            clamp_range(&(Bound::Included(4), Bound::Excluded(2)), 5),
            4..4
        );
        assert_eq!(
            clamp_range(&(Bound::Excluded(1), Bound::Excluded(4)), 5),
            2..4
        );
        assert_eq!(clamp_range(&(usize::MAX..), 5), 5..5);
        assert_eq!(clamp_range(&(..=usize::MAX), 5), 0..5);
    }

    #[test]
    fn a_page_is_the_window_and_carries_the_version_it_was_read_at() {
        let l = list(5);
        l.push(5);
        let (header, rows) = page(&l, 4, 10);
        assert_eq!((header.version, header.total, header.count), (1, 6, 2));
        assert_eq!(rows, [4, 0, 0, 0, 5, 0, 0, 0]);
        let (header, rows) = page(&l, 6, 10);
        assert_eq!((header.total, header.count), (6, 0));
        assert!(rows.is_empty());
    }

    #[test]
    fn every_change_moves_the_version_and_a_non_change_does_not() {
        let l = list(3);
        assert_eq!(l.version(), 0);
        l.push(3);
        l.insert(0, 9);
        assert_eq!(l.remove(1), 0);
        l.update_at(0, |n| *n += 1);
        l.move_item(0, 2);
        l.replace(vec![1, 2]);
        assert_eq!(l.version(), 6);
        l.move_item(1, 1);
        assert_eq!(l.version(), 6, "a move onto itself is not a change");
        l.clear();
        assert_eq!(l.version(), 7);
        l.clear();
        assert_eq!(l.version(), 7, "clearing an empty list is not a change");
        assert!(l.is_empty());
    }

    #[test]
    fn out_of_range_writes_panic_and_change_nothing() {
        let l = list(2);
        let panics = |f: &dyn Fn()| catch_unwind(AssertUnwindSafe(f)).is_err();
        assert!(panics(&|| l.insert(3, 9)));
        assert!(panics(&|| {
            l.remove(2);
        }));
        assert!(panics(&|| l.update_at(2, |n| *n = 9)));
        assert!(panics(&|| l.move_item(0, 2)));
        assert!(panics(&|| l.move_item(2, 0)));
        assert_eq!((l.to_vec(), l.version()), (vec![0, 1], 0));
        l.push(2);
        assert_eq!(l.to_vec(), [0, 1, 2], "no lock was left held");
    }

    #[test]
    fn a_panicking_update_at_still_moves_the_version() {
        let l = list(2);
        let result = catch_unwind(AssertUnwindSafe(|| {
            l.update_at(1, |n| {
                *n = 20;
                panic!("boom");
            });
        }));
        assert!(result.is_err());
        assert_eq!((l.to_vec(), l.version()), (vec![0, 20], 1));
    }

    #[test]
    fn clear_and_replace_drop_the_old_items_after_the_lock_is_released() {
        #[derive(Clone)]
        struct Probe(Arc<AtomicU64>, Arc<Lazy<Probe>>);
        impl Encode for Probe {
            fn encode(&self, _w: &mut Writer) {}
        }
        impl Drop for Probe {
            fn drop(&mut self) {
                // Reading the list from the destructor would deadlock if its lock were held.
                self.0.fetch_add(self.1.len() as u64 + 1, Ordering::SeqCst);
            }
        }
        let seen = Arc::new(AtomicU64::new(0));
        let l = Arc::new(Lazy::new());
        l.push(Probe(Arc::clone(&seen), Arc::clone(&l)));
        l.clear();
        assert!(
            seen.load(Ordering::SeqCst) > 0,
            "dropped, and the list was readable"
        );
        l.push(Probe(Arc::clone(&seen), Arc::clone(&l)));
        l.replace(Vec::new());
        assert!(l.is_empty());
    }

    #[test]
    fn a_view_refuses_writes_with_a_message_that_says_what_to_do() {
        use crate::Signal;
        let source = Signal::new(vec![1_u32, 2, 3]);
        let view = Lazy::over(&source.derive().filter(|n| n % 2 == 1).build());
        assert!(view.is_view());
        assert!(!view.can_write());
        let message = catch_unwind(AssertUnwindSafe(|| view.push(9)))
            .unwrap_err()
            .downcast::<String>()
            .unwrap();
        assert!(message.contains("read-only view"), "{message}");
        assert!(
            message.contains("write the list it is derived from"),
            "{message}"
        );
    }

    #[test]
    fn clones_share_the_list_and_ptr_eq_tells_them_apart() {
        let a = list(1);
        let b = a.clone();
        let c = list(1);
        assert!(a.ptr_eq(&b) && !a.ptr_eq(&c));
        b.push(1);
        assert_eq!(a.len(), 2);
        assert!(format!("{a:?}").contains("version: 1"));
        assert_eq!(a.get(1), Some(1));
        assert_eq!(a.get(2), None);
    }

    #[test]
    fn a_view_pages_through_the_derived_index_and_versions_its_changes() {
        use crate::Signal;
        let source = Signal::new((0..20_u32).collect::<Vec<_>>());
        let view = Lazy::over(&source.derive().filter(|n| n % 3 == 0).build());
        assert_eq!(view.len(), 7);
        let v0 = view.version();
        let (header, rows) = page(&view, 2, 3);
        assert_eq!((header.total, header.count, header.version), (7, 3, v0));
        assert_eq!(rows, [6, 0, 0, 0, 9, 0, 0, 0, 12, 0, 0, 0]);

        source.push(21);
        assert_eq!(view.len(), 8);
        assert!(view.version() > v0, "a row entered the view");
        let v1 = view.version();
        source.push(22);
        assert_eq!(
            view.version(),
            v1,
            "a row the filter ignores is not a change"
        );
    }

    #[test]
    fn debug_never_drains_a_view() {
        use crate::Signal;
        let source = Signal::new(vec![1_u32]);
        let view = Lazy::over(&source.derive().build());
        assert!(format!("{view:?}").contains("view: true"));
    }

    #[test]
    fn lists_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Lazy<String>>();
        assert_send_sync::<Arc<dyn LazySource>>();
    }
}
