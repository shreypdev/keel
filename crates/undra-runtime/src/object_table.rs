//! The object table: a generation-tagged slab that issues and resolves handles (SPEC 1.2, 5.4).
//!
//! A [`Handle`] packs a slot index (low 24 bits, 16.7 million live objects) and a generation
//! (high 40 bits, starting at 1). Every handle the table issues gets a **fresh generation from
//! one process-wide, monotonically increasing counter**, so a `(slot, generation)` pair is never
//! issued twice in a process: a stale handle is rejected instead of aliasing whatever object
//! reuses the slot, and that holds across [`restore`](crate::Runtime::restore) too (a snapshot
//! carries the counter's high-water mark, ADR-022). The counter has 2^40 - 1 values (3.5 years
//! at 10,000 issues a second, ADR-040 decision 8); when it is spent the table refuses to issue
//! more handles.
//!
//! **Host references (ADR-040).** An entry counts the references the host owns: a constructor's
//! reply is one, and so is every handle in a reply that [`issue`](ObjectTable::issue_with)
//! produced. The table also maps an object's address to its handle, so **an object has at most
//! one live handle**: issuing the same `Arc` again while the host holds it gives the same handle
//! and one more reference, and [`release`](ObjectTable::release) removes the entry when the last
//! reference goes. An entry first issued by a return is *transient*: a snapshot leaves it out and
//! a restore makes its handle stale.
//!
//! The table has its own reader-writer lock and is safe to use from anywhere, including from
//! inside a dispatcher that runs under the core lock. Objects are `Arc`s, so an object
//! outlives its handle while a task still holds it.
//!
//! ```
//! use std::sync::Arc;
//! use undra_runtime::object_table::ObjectTable;
//! use undra_runtime::{UndraObject, plain};
//!
//! struct Counter;
//! impl UndraObject for Counter {
//!     const TYPE_ID: u32 = 1;
//!     const NAME: &'static str = "Counter";
//! }
//!
//! let table = ObjectTable::new();
//! let first = table.insert(plain(Arc::new(Counter)));
//! assert!(table.get::<Counter>(first).is_ok());
//! table.release(first).unwrap();
//! let second = table.insert(plain(Arc::new(Counter)));
//! assert_eq!(first.index(), second.index());          // the slot was reused ...
//! assert!(table.get::<Counter>(first).is_err());      // ... but the old handle is stale
//! assert!(table.get::<Counter>(second).is_ok());
//! ```

use crate::atomic_update::cas_update_u64;
use core::fmt;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::RwLock;

pub use undra_wire::Handle;

use crate::object::AnyObject;

/// How far past the current end of the table [`ObjectTable::insert_at`] will extend it. A
/// corrupt snapshot must not be able to make the table allocate gigabytes.
const MAX_INDEX_GAP: usize = 1 << 20;

/// The most slots a table can have: a handle holds 24 bits of slot index.
const MAX_SLOTS: usize = Handle::MAX_INDEX as usize + 1;

/// The largest slot index [`Runtime::restore`](crate::Runtime::restore) accepts in a snapshot.
pub(crate) const MAX_RESTORE_INDEX: usize = MAX_INDEX_GAP;

/// The highest generation (and snapshot `generation_floor`) a restore accepts. The counter is
/// shared by every runtime in the process, so obeying a floor near the end of the counter would
/// let one corrupt or hostile snapshot exhaust handle creation process-wide, permanently across
/// relaunches (crash recovery restores the same bytes). 2^36 of headroom keeps ~68 billion
/// issues (about 79 days at 10,000 a second) available after the most adversarial accepted
/// snapshot.
pub(crate) const GENERATION_CEILING: u64 = Handle::MAX_GENERATION - (1 << 36);

/// Why a handle did not resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadHandleReason {
    /// The null handle (`0`).
    Null,
    /// No slot with that index has ever existed.
    OutOfRange,
    /// The slot exists but the handle's generation is not the current one (released, or
    /// invalidated by a restore).
    Stale,
    /// The handle is live but the object is not a `T`.
    WrongType {
        /// The requested type.
        expected: &'static str,
        /// The type name the object reports.
        found: &'static str,
    },
}

/// A handle that did not resolve to an object of the requested type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadHandle {
    /// The offending handle.
    pub handle: Handle,
    /// Why it failed.
    pub reason: BadHandleReason,
}

impl fmt::Display for BadHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            BadHandleReason::Null => f.write_str("null handle"),
            BadHandleReason::OutOfRange => write!(f, "unknown handle {:?}", self.handle),
            BadHandleReason::Stale => write!(f, "stale handle {:?}", self.handle),
            BadHandleReason::WrongType { expected, found } => write!(
                f,
                "handle {:?} refers to a {found}, not a {expected}",
                self.handle
            ),
        }
    }
}

impl std::error::Error for BadHandle {}

/// Why [`ObjectTable::insert_at`] refused a handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertAtError {
    /// The null handle, a generation of `0`, or the largest generation (which would leave the
    /// generation counter nothing to issue).
    Invalid,
    /// The slot is occupied.
    Occupied,
    /// The index is implausibly far beyond the end of the table.
    IndexTooFar,
}

impl fmt::Display for InsertAtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InsertAtError::Invalid => "null handle, generation 0 or the last generation",
            InsertAtError::Occupied => "the slot is occupied",
            InsertAtError::IndexTooFar => "the slot index is too far beyond the table",
        })
    }
}

impl std::error::Error for InsertAtError {}

/// What [`ObjectTable::release`] did.
pub enum Released {
    /// The host owns more references: the entry stays.
    Kept {
        /// The references still owned.
        remaining: u32,
    },
    /// That was the last one: the entry is gone and this is its object.
    Removed(Arc<dyn AnyObject>),
}

impl core::fmt::Debug for Released {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Released::Kept { remaining } => f
                .debug_struct("Kept")
                .field("remaining", remaining)
                .finish(),
            Released::Removed(_) => f.write_str("Removed(..)"),
        }
    }
}

impl Released {
    /// The object, when it was removed.
    pub fn into_removed(self) -> Option<Arc<dyn AnyObject>> {
        match self {
            Released::Removed(object) => Some(object),
            Released::Kept { .. } => None,
        }
    }
}

/// The signals of a store the host has asked to observe, tracked by the runtime so that a
/// restore can re-emit them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Observed {
    all: bool,
    signals: BTreeSet<u32>,
}

impl Observed {
    /// Records an `observe(signal_id, on)` call. `signal_count` resolves "all but one".
    ///
    /// An id the store does not have is ignored, like `StoreCell::observe` ignores it: it comes
    /// from the host, and must not be able to grow this set without bound.
    pub(crate) fn record(&mut self, signal_id: u32, on: bool, signal_count: u32) {
        if signal_id != undra_meta::ids::ALL_SIGNALS && signal_id >= signal_count {
            return;
        }
        if signal_id == undra_meta::ids::ALL_SIGNALS {
            self.all = on;
            self.signals.clear();
        } else if on {
            if !self.all {
                self.signals.insert(signal_id);
            }
        } else {
            if self.all {
                self.all = false;
                self.signals = (0..signal_count).collect();
            }
            self.signals.remove(&signal_id);
        }
    }

    /// The `signal_id`s to re-observe: `ALL_SIGNALS` alone, or the explicit set.
    pub(crate) fn to_reobserve(&self) -> Vec<u32> {
        if self.all {
            vec![undra_meta::ids::ALL_SIGNALS]
        } else {
            self.signals.iter().copied().collect()
        }
    }

    /// Whether nothing is observed.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        !self.all && self.signals.is_empty()
    }
}

struct Entry {
    object: Arc<dyn AnyObject>,
    poisoned: bool,
    observed: Observed,
    /// The references the host owns (ADR-040): a constructor's reply is one, every handle of a
    /// reply produced by [`ObjectTable::issue_with`] is one. The entry goes at zero.
    host_refs: u32,
    /// First issued by a return, not by a host-called constructor: a snapshot leaves it out and
    /// a restore makes its handle stale.
    transient: bool,
    /// A page server registered for a store (ADR-043): the host holds no reference to it, so it
    /// cannot release it; it goes with its store.
    table_owned: bool,
}

struct Slot {
    generation: u64,
    entry: Option<Entry>,
}

struct Inner {
    slots: Vec<Slot>,
    free: Vec<u32>,
    live: usize,
    stores: BTreeSet<u32>,
    /// An object's address (`Arc::as_ptr`, valid because the entry holds the `Arc`) to its slot
    /// index: what makes a handle per object (ADR-040 decision 5).
    by_address: HashMap<usize, u32>,
    /// The sum of every entry's `host_refs`.
    host_refs: u64,
}

/// The source of handle generations: a counter of the highest generation issued so far
/// (`0` = none yet). Issuing is one atomic increment; generation `0` is never issued (it is
/// the invalid value) and the counter never wraps: at [`Handle::MAX_GENERATION`] it is exhausted
/// for good.
pub(crate) struct Generations {
    last: AtomicU64,
    /// The exhaustion has been reported (the FATAL record is written once per counter).
    exhaustion_reported: AtomicBool,
}

impl Generations {
    const fn new() -> Generations {
        Generations {
            last: AtomicU64::new(0),
            exhaustion_reported: AtomicBool::new(false),
        }
    }

    /// The next generation, or `None` if all of them have been issued.
    fn issue(&self) -> Option<u64> {
        cas_update_u64(&self.last, Ordering::AcqRel, Ordering::Acquire, |last| {
            if last >= Handle::MAX_GENERATION {
                None
            } else {
                Some(last + 1)
            }
        })
        .ok()
        .map(|previous| previous + 1)
    }

    /// The highest generation issued so far.
    fn last(&self) -> u64 {
        self.last.load(Ordering::Acquire)
    }

    /// Makes sure nothing at or below `floor` is issued from now on. Never lowers the counter.
    fn raise_to(&self, floor: u64) {
        self.last
            .fetch_max(floor.min(Handle::MAX_GENERATION), Ordering::AcqRel);
    }
}

/// The counter every runtime of the process shares, so that a handle from one runtime (or from
/// an earlier `init`/`shutdown` cycle of this process) cannot be mistaken for a handle of
/// another.
static PROCESS_GENERATIONS: Generations = Generations::new();

/// The highest generation the process-wide counter has issued so far (`0` before any), which
/// survives a runtime's shutdown: what a snapshot taken with no runtime running must record as
/// its floor so a fresh `init` never re-issues a generation a host may still hold (ADR-022).
/// Tables with a counter of their own (the ones test runtimes use) are not counted.
pub fn process_generation_floor() -> u64 {
    PROCESS_GENERATIONS.last()
}

/// What [`ObjectTable::clear`] took out of the table.
pub(crate) struct Cleared {
    pub handle: Handle,
    pub object: Arc<dyn AnyObject>,
    pub observed: Observed,
}

/// The generation-tagged slab of live objects. See the [module documentation](self).
pub struct ObjectTable {
    inner: RwLock<Inner>,
    /// The id of the runtime that owns the table: every store placed in it learns it
    /// (`StoreCell::set_owner`, ADR-035). `0` for a table no runtime owns.
    owner: AtomicU64,
    /// `None`: the process-wide counter. Tables built with [`ObjectTable::isolated`] own one
    /// (unit tests that need exact generations).
    own_generations: Option<Generations>,
}

impl Default for ObjectTable {
    fn default() -> Self {
        ObjectTable::new()
    }
}

impl ObjectTable {
    /// Creates an empty table whose handles take their generations from the process-wide
    /// counter (see the [module documentation](self)).
    pub fn new() -> ObjectTable {
        ObjectTable {
            inner: RwLock::new(Inner {
                slots: Vec::new(),
                free: Vec::new(),
                live: 0,
                stores: BTreeSet::new(),
                by_address: HashMap::new(),
                host_refs: 0,
            }),
            owner: AtomicU64::new(0),
            own_generations: None,
        }
    }

    /// Records the runtime that owns the table; stores inserted from now on learn it.
    pub(crate) fn set_owner(&self, runtime_id: u64) {
        self.owner.store(runtime_id, Ordering::Relaxed);
    }

    /// A table with a generation counter of its own, starting at 1: the same behaviour with
    /// values that do not depend on what else the process has done. Test runtimes use it, so
    /// the handles a test sees (and may put in a golden file) are the same on every run, however
    /// many tests run in parallel.
    pub(crate) fn isolated() -> ObjectTable {
        ObjectTable {
            own_generations: Some(Generations::new()),
            ..ObjectTable::new()
        }
    }

    /// An isolated table whose counter has already issued `last` generations.
    #[cfg(test)]
    pub(crate) fn isolated_after(last: u64) -> ObjectTable {
        let table = ObjectTable::isolated();
        table.raise_generation_floor(last);
        table
    }

    fn generations(&self) -> &Generations {
        self.own_generations
            .as_ref()
            .unwrap_or(&PROCESS_GENERATIONS)
    }

    /// The highest generation issued so far (what a snapshot records as its floor).
    pub(crate) fn generation_floor(&self) -> u64 {
        self.generations().last()
    }

    /// Raises the generation counter to at least `floor` (restore). Never lowers it.
    pub(crate) fn raise_generation_floor(&self, floor: u64) {
        self.generations().raise_to(floor);
    }

    /// Takes the next generation, or panics with a clear message once the counter is spent.
    fn issue_generation(&self) -> u64 {
        let generations = self.generations();
        match generations.issue() {
            Some(generation) => generation,
            None => {
                if !generations
                    .exhaustion_reported
                    .swap(true, Ordering::Relaxed)
                {
                    crate::runtime::log_fatal_current(
                        "undra::runtime",
                        "handle generations exhausted: 2^40 - 1 handles have been issued in this \
                         process; no further object can be created (restart the core)",
                    );
                }
                panic!(
                    "undra-runtime: handle generations are exhausted (2^40 - 1 handles have been \
                     issued in this process); restart the core"
                )
            }
        }
    }

    /// Stores `object` and returns its handle, which the host owns one reference to
    /// (a host-called constructor's reply). If the object is a store, its cell learns its handle.
    ///
    /// # Panics
    ///
    /// Panics if the table would need more than 2^24 slots, or once the process has issued every
    /// handle generation and the counter is spent (logged at FATAL first; ADR-022). Both are
    /// contained at the runtime's entry points like any other panic.
    pub fn insert(&self, object: Arc<dyn AnyObject>) -> Handle {
        self.place(object, false)
    }

    /// Stores `object` as a new entry holding one host reference.
    fn place(&self, object: Arc<dyn AnyObject>, transient: bool) -> Handle {
        self.place_with(object, transient, false)
    }

    /// Stores `object` as a new entry: holding one host reference, or (`table_owned`) none, for an
    /// entry the table registers for a store and removes with it.
    fn place_with(&self, object: Arc<dyn AnyObject>, transient: bool, table_owned: bool) -> Handle {
        let host_refs = u32::from(!table_owned);
        let cell = object.as_store().cloned();
        let is_store = cell.is_some();
        let address = object.address();
        // Before any lock is taken and before the table is touched, so a refusal (which logs)
        // leaves it exactly as it was and calls out with no lock held.
        let generation = self.issue_generation();
        let handle = {
            let mut inner = self.inner.write();
            let index = match inner.free.pop() {
                Some(index) => index,
                None => {
                    if inner.slots.len() >= MAX_SLOTS {
                        panic!("undra-runtime: object table is full (2^24 live objects)");
                    }
                    let index = inner.slots.len() as u32;
                    inner.slots.push(Slot {
                        generation: 0,
                        entry: None,
                    });
                    index
                }
            };
            let slot = &mut inner.slots[index as usize];
            slot.generation = generation;
            slot.entry = Some(Entry {
                object,
                poisoned: false,
                observed: Observed::default(),
                host_refs,
                transient,
                table_owned,
            });
            let handle = Handle::new(index, generation);
            inner.live += 1;
            inner.host_refs += u64::from(host_refs);
            inner.by_address.insert(address, index);
            if is_store {
                inner.stores.insert(index);
            }
            handle
        };
        if let Some(cell) = cell {
            // The owner first: a commit that sees the handle must route to the right runtime.
            cell.set_owner(self.owner.load(Ordering::Relaxed));
            cell.set_handle(handle.0);
            if let Some(hooks) = cell.lazy_hooks() {
                (hooks.register)(self, &cell, handle.0);
            }
        }
        handle
    }

    /// Registers a page server for each `Lazy` signal of `cell` and tells the cell the handle
    /// (ADR-043): transient entries the table owns, so they are never in a snapshot and the host
    /// cannot release them; [`unregister_lazy`](ObjectTable::unregister_lazy) removes them with the
    /// store. Reached through the cell's [`LazyHooks`](undra_signals::LazyHooks), which only a core
    /// with a `Lazy` field sets.
    pub(crate) fn register_lazy(&self, cell: &undra_signals::StoreCell) {
        for (signal_id, source) in cell.lazy_sources() {
            let server = self.place_with(crate::lazy::page_server(source), true, true);
            cell.set_lazy_handle(signal_id, server.0);
        }
    }

    /// Removes the page servers [`register_lazy`](ObjectTable::register_lazy) registered for `cell`.
    pub(crate) fn unregister_lazy(&self, cell: &undra_signals::StoreCell) {
        for (signal_id, _) in cell.lazy_sources() {
            let server = Handle(cell.lazy_handle(signal_id));
            cell.set_lazy_handle(signal_id, 0);
            Self::remove_table_owned(&mut self.inner.write(), server);
        }
    }

    /// Gives the host one more reference to the object at `address`, if the table holds it:
    /// its handle, and whether the count saturated.
    fn add_ref(&self, address: usize) -> Option<(Handle, bool)> {
        let mut inner = self.inner.write();
        let index = *inner.by_address.get(&address)?;
        let slot = inner.slots.get_mut(index as usize)?;
        let generation = slot.generation;
        let entry = slot.entry.as_mut()?;
        let saturated = entry.host_refs == u32::MAX;
        if !saturated {
            entry.host_refs += 1;
        }
        if !saturated {
            inner.host_refs += 1;
        }
        Some((Handle::new(index, generation), saturated))
    }

    /// Hands the host a handle to the object at `address` (ADR-040): the handle it already has
    /// with one more reference, or, when the table does not hold it, a new entry built by `make`
    /// (one reference; transient when `transient`, which every return sets and a constructor
    /// returning `Arc<Self>` does not). Returns the handle and whether the entry is new.
    ///
    /// A count that would pass `u32::MAX` stays there and is logged at ERROR (it never wraps).
    ///
    /// # Panics
    ///
    /// As [`insert`](ObjectTable::insert).
    pub fn issue_with(
        &self,
        address: usize,
        make: impl FnOnce() -> Arc<dyn AnyObject>,
        transient: bool,
    ) -> (Handle, bool) {
        if let Some((handle, saturated)) = self.add_ref(address) {
            if saturated {
                crate::runtime::log_error_current(
                    "undra::runtime",
                    &format!(
                        "host references to {handle:?} reached u32::MAX and stay there; the host leaks"
                    ),
                );
            }
            return (handle, false);
        }
        (self.place(make(), transient), true)
    }

    /// Stores `object` at exactly `handle`: the restore path (SPEC 5.9). Fails if the slot is
    /// occupied. Slots below `handle.index()` that do not exist yet are created vacant. The
    /// generation counter is raised to the handle's generation, so the table never issues a
    /// generation that a live handle already carries. The entry holds one host reference.
    pub fn insert_at(
        &self,
        handle: Handle,
        object: Arc<dyn AnyObject>,
    ) -> Result<(), InsertAtError> {
        if handle.is_null()
            || handle.generation() == 0
            || handle.generation() >= Handle::MAX_GENERATION
        {
            return Err(InsertAtError::Invalid);
        }
        let index = handle.index() as usize;
        let cell = object.as_store().cloned();
        let is_store = cell.is_some();
        let address = object.address();
        {
            let mut inner = self.inner.write();
            if index > inner.slots.len().saturating_add(MAX_INDEX_GAP) {
                return Err(InsertAtError::IndexTooFar);
            }
            while inner.slots.len() <= index {
                let new_index = inner.slots.len();
                inner.slots.push(Slot {
                    generation: 0,
                    entry: None,
                });
                if new_index != index {
                    inner.free.push(new_index as u32);
                }
            }
            if inner.slots[index].entry.is_some() {
                return Err(InsertAtError::Occupied);
            }
            // The slot may have been on the free list (it existed and was vacant).
            inner.free.retain(|&i| i as usize != index);
            self.generations().raise_to(handle.generation());
            let slot = &mut inner.slots[index];
            slot.generation = handle.generation();
            slot.entry = Some(Entry {
                object,
                poisoned: false,
                observed: Observed::default(),
                host_refs: 1,
                transient: false,
                table_owned: false,
            });
            inner.live += 1;
            inner.host_refs += 1;
            inner.by_address.insert(address, handle.index());
            if is_store {
                inner.stores.insert(handle.index());
            }
        }
        if let Some(cell) = cell {
            cell.set_owner(self.owner.load(Ordering::Relaxed));
            cell.set_handle(handle.0);
            if let Some(hooks) = cell.lazy_hooks() {
                (hooks.register)(self, &cell, handle.0);
            }
        }
        Ok(())
    }

    fn check(inner: &Inner, handle: Handle) -> Result<&Entry, BadHandle> {
        let bad = |reason| BadHandle { handle, reason };
        if handle.is_null() {
            return Err(bad(BadHandleReason::Null));
        }
        let slot = inner
            .slots
            .get(handle.index() as usize)
            .ok_or_else(|| bad(BadHandleReason::OutOfRange))?;
        match &slot.entry {
            Some(entry) if slot.generation == handle.generation() => Ok(entry),
            _ => Err(bad(BadHandleReason::Stale)),
        }
    }

    /// Resolves `handle` to its object, whatever its type.
    pub fn get_dyn(&self, handle: Handle) -> Result<Arc<dyn AnyObject>, BadHandle> {
        let inner = self.inner.read();
        Self::check(&inner, handle).map(|e| e.object.clone())
    }

    /// The type id and type name of the object behind `handle`, without taking a reference to
    /// the object (the routing step of every call needs only these).
    pub fn type_of(&self, handle: Handle) -> Result<(u32, &'static str), BadHandle> {
        let inner = self.inner.read();
        Self::check(&inner, handle).map(|e| (e.object.undra_type_id(), e.object.undra_type_name()))
    }

    /// Resolves `handle` to a `T`.
    pub fn get<T: Send + Sync + 'static>(&self, handle: Handle) -> Result<Arc<T>, BadHandle> {
        let inner = self.inner.read();
        let entry = Self::check(&inner, handle)?;
        // One reference is taken (the `Arc<T>` returned), not two: `shared()` hands out the
        // concrete object's own `Arc`.
        entry
            .object
            .shared()
            .downcast::<T>()
            .map_err(|_| BadHandle {
                handle,
                reason: BadHandleReason::WrongType {
                    expected: core::any::type_name::<T>(),
                    found: entry.object.undra_type_name(),
                },
            })
    }

    /// Gives one host reference back (ADR-040). While others remain the entry stays
    /// ([`Released::Kept`]); the last one removes it and returns the object, so the caller can
    /// drop it wherever it wants (the runtime drops it under the core lock). The slot becomes
    /// reusable; the next object placed in it gets a fresh generation, so `handle` stays stale
    /// for good.
    pub fn release(&self, handle: Handle) -> Result<Released, BadHandle> {
        let released = self.release_entry(handle)?;
        // The page servers of a store's `Lazy` signals go with it (ADR-043), outside the table's lock.
        if let Released::Removed(object) = &released {
            if let Some(cell) = object.as_store() {
                if let Some(hooks) = cell.lazy_hooks() {
                    (hooks.unregister)(self, cell);
                }
            }
        }
        Ok(released)
    }

    fn release_entry(&self, handle: Handle) -> Result<Released, BadHandle> {
        let mut inner = self.inner.write();
        Self::check(&inner, handle)?;
        let index = handle.index();
        if inner.slots[index as usize]
            .entry
            .as_ref()
            .is_some_and(|e| e.table_owned)
        {
            // A page server the table registered for a store: the host holds no reference to
            // give back, and it must not be able to take the list away from its own store.
            return Ok(Released::Kept { remaining: 0 });
        }
        let kept = {
            let Some(entry) = inner.slots[index as usize].entry.as_mut() else {
                return Err(BadHandle {
                    handle,
                    reason: BadHandleReason::Stale,
                });
            };
            entry.host_refs = entry.host_refs.saturating_sub(1);
            entry.host_refs
        };
        inner.host_refs = inner.host_refs.saturating_sub(1);
        if kept > 0 {
            return Ok(Released::Kept { remaining: kept });
        }
        let slot = &mut inner.slots[index as usize];
        let entry = slot.entry.take();
        inner.free.push(index);
        inner.live -= 1;
        inner.stores.remove(&index);
        match entry {
            Some(entry) => {
                let address = entry.object.address();
                if inner.by_address.get(&address) == Some(&index) {
                    inner.by_address.remove(&address);
                }
                Ok(Released::Removed(entry.object))
            }
            // `check` proved the slot occupied while we held the write lock.
            None => Err(BadHandle {
                handle,
                reason: BadHandleReason::Stale,
            }),
        }
    }

    /// Removes an entry the table registered (a page server), whatever its host references.
    fn remove_table_owned(inner: &mut Inner, handle: Handle) {
        let index = handle.index();
        let Some(slot) = inner.slots.get_mut(index as usize) else {
            return;
        };
        if slot.generation != handle.generation() {
            return;
        }
        let Some(entry) = slot.entry.take() else {
            return;
        };
        inner.free.push(index);
        inner.live -= 1;
        let address = entry.object.address();
        if inner.by_address.get(&address) == Some(&index) {
            inner.by_address.remove(&address);
        }
    }

    /// How many references the host owns to `handle`'s object (`None` for a stale handle).
    pub fn host_refs_of(&self, handle: Handle) -> Option<u32> {
        let inner = self.inner.read();
        Self::check(&inner, handle).ok().map(|e| e.host_refs)
    }

    /// The sum of the host references of every live object (`stats_json`'s `host_refs`).
    pub fn host_refs(&self) -> u64 {
        self.inner.read().host_refs
    }

    /// Number of live objects.
    pub fn live(&self) -> usize {
        self.inner.read().live
    }

    /// Number of live stores.
    pub fn store_count(&self) -> usize {
        self.inner.read().stores.len()
    }

    /// Every live store with its handle, in slot order, leaving out the ones first issued by a
    /// return (ADR-040: derived handles are transient, so a snapshot does not hold them).
    pub(crate) fn stores(&self) -> Vec<(Handle, Arc<dyn AnyObject>)> {
        let inner = self.inner.read();
        inner
            .stores
            .iter()
            .filter_map(|&index| {
                let slot = inner.slots.get(index as usize)?;
                let entry = slot.entry.as_ref()?;
                if entry.transient {
                    return None;
                }
                Some((Handle::new(index, slot.generation), entry.object.clone()))
            })
            .collect()
    }

    /// Flags the object behind `handle` as having panicked. Returns whether it was live.
    pub(crate) fn mark_poisoned(&self, handle: Handle) -> bool {
        let mut inner = self.inner.write();
        if Self::check(&inner, handle).is_err() {
            return false;
        }
        if let Some(entry) = inner.slots[handle.index() as usize].entry.as_mut() {
            entry.poisoned = true;
        }
        true
    }

    /// Number of live stores flagged as poisoned.
    pub(crate) fn poisoned_stores(&self) -> usize {
        let inner = self.inner.read();
        inner
            .stores
            .iter()
            .filter(|&&i| {
                inner
                    .slots
                    .get(i as usize)
                    .and_then(|s| s.entry.as_ref())
                    .is_some_and(|e| e.poisoned)
            })
            .count()
    }

    /// Runs `f` on the observation record of `handle`.
    pub(crate) fn with_observed<R>(
        &self,
        handle: Handle,
        f: impl FnOnce(&mut Observed) -> R,
    ) -> Option<R> {
        let mut inner = self.inner.write();
        Self::check(&inner, handle).ok()?;
        inner.slots[handle.index() as usize]
            .entry
            .as_mut()
            .map(|e| f(&mut e.observed))
    }

    /// Empties the table: every slot becomes vacant, so every handle issued so far is stale
    /// (a slot's next occupant gets a generation no earlier handle carries, or, in a restore,
    /// exactly the generation the snapshot recorded). Returns what was in it.
    pub(crate) fn clear(&self) -> Vec<Cleared> {
        let mut inner = self.inner.write();
        let mut out = Vec::with_capacity(inner.live);
        let mut free = Vec::with_capacity(inner.slots.len());
        for (index, slot) in inner.slots.iter_mut().enumerate() {
            if let Some(entry) = slot.entry.take() {
                out.push(Cleared {
                    handle: Handle::new(index as u32, slot.generation),
                    object: entry.object,
                    observed: entry.observed,
                });
            }
            free.push(index as u32);
        }
        free.reverse(); // pop() hands out the lowest index first
        inner.free = free;
        inner.live = 0;
        inner.host_refs = 0;
        inner.stores.clear();
        inner.by_address.clear();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{UndraObject, plain};

    #[derive(Debug)]
    struct A(u32);
    impl UndraObject for A {
        const TYPE_ID: u32 = 1;
        const NAME: &'static str = "A";
    }
    #[derive(Debug)]
    struct B;
    impl UndraObject for B {
        const TYPE_ID: u32 = 2;
        const NAME: &'static str = "B";
    }

    fn a(n: u32) -> Arc<dyn AnyObject> {
        plain(Arc::new(A(n)))
    }

    #[test]
    fn first_handle_is_index_zero_generation_one() {
        let t = ObjectTable::isolated();
        let h = t.insert(a(1));
        assert_eq!((h.index(), h.generation()), (0, 1));
        assert!(!h.is_null());
        assert_eq!(t.get::<A>(h).unwrap().0, 1);
        assert_eq!(t.live(), 1);
    }

    #[test]
    fn null_and_unknown_handles_are_rejected_with_a_reason() {
        let t = ObjectTable::new();
        assert_eq!(
            t.get::<A>(Handle::NULL).unwrap_err().reason,
            BadHandleReason::Null
        );
        assert_eq!(
            t.get::<A>(Handle::new(5, 1)).unwrap_err().reason,
            BadHandleReason::OutOfRange
        );
        assert!(t.release(Handle::new(0, 1)).is_err());
    }

    #[test]
    fn released_handle_is_stale_and_slot_is_reused_with_a_new_generation() {
        let t = ObjectTable::isolated();
        let h1 = t.insert(a(1));
        assert_eq!(
            t.release(h1)
                .unwrap()
                .into_removed()
                .unwrap()
                .downcast::<A>()
                .unwrap()
                .0,
            1
        );
        assert_eq!(t.live(), 0);
        assert_eq!(t.get::<A>(h1).unwrap_err().reason, BadHandleReason::Stale);

        let h2 = t.insert(a(2));
        assert_eq!(h2.index(), h1.index());
        assert_eq!(h2.generation(), h1.generation() + 1);
        assert_eq!(t.get::<A>(h2).unwrap().0, 2);
        assert_eq!(t.get::<A>(h1).unwrap_err().reason, BadHandleReason::Stale);
        // Releasing twice is an error, not a double free.
        assert!(t.release(h1).is_err());
    }

    #[test]
    fn wrong_type_names_both_types() {
        let t = ObjectTable::new();
        let h = t.insert(plain(Arc::new(B)));
        match t.get::<A>(h).unwrap_err().reason {
            BadHandleReason::WrongType { expected, found } => {
                assert!(expected.ends_with("::A"), "{expected}");
                assert_eq!(found, "B");
            }
            other => panic!("unexpected reason {other:?}"),
        }
        assert!(t.get::<B>(h).is_ok());
    }

    #[test]
    fn object_outlives_its_handle_while_an_arc_is_held() {
        let t = ObjectTable::new();
        let h = t.insert(a(9));
        let held = t.get::<A>(h).unwrap();
        drop(t.release(h).unwrap().into_removed());
        assert_eq!(held.0, 9);
    }

    #[test]
    fn the_counter_is_shared_by_every_table_of_the_process() {
        let (t1, t2) = (ObjectTable::new(), ObjectTable::new());
        let first = t1.insert(a(1));
        let second = t2.insert(a(2));
        let third = t1.insert(a(3));
        assert!(
            first.generation() < second.generation() && second.generation() < third.generation(),
            "{first:?} {second:?} {third:?}"
        );
        // A handle of one table means nothing in the other, even at the same index.
        assert_eq!(first.index(), second.index());
        assert!(t2.get::<A>(first).is_err());
    }

    #[test]
    fn a_slot_never_gets_a_generation_twice_however_often_it_is_recycled() {
        let t = ObjectTable::isolated();
        let mut seen = std::collections::BTreeSet::new();
        for n in 0..200 {
            let h = t.insert(a(n));
            assert_eq!(h.index(), 0, "the lone slot is recycled");
            assert!(
                seen.insert(h.generation()),
                "generation {} reissued",
                h.generation()
            );
            t.release(h).unwrap();
        }
        assert_eq!(seen.len(), 200);
    }

    #[test]
    fn the_counter_is_spent_after_2_pow_40_handles_and_refuses_cleanly() {
        let max = Handle::MAX_GENERATION;
        let t = ObjectTable::isolated_after(max - 2);
        let last = t.insert(a(1));
        let last2 = t.insert(a(2));
        assert_eq!((last.generation(), last2.generation()), (max - 1, max));
        let refused = crate::guard::guarded(|| t.insert(a(3))).unwrap_err();
        assert!(
            refused.message.contains("generations are exhausted"),
            "{}",
            refused.message
        );
        // Nothing was half done: the two live objects are intact and handles still resolve.
        assert_eq!(t.live(), 2);
        assert_eq!(t.get::<A>(last).unwrap().0, 1);
        assert_eq!(t.get::<A>(last2).unwrap().0, 2);
        // And it stays refused: releasing does not free up a generation.
        t.release(last).unwrap();
        assert!(crate::guard::guarded(|| t.insert(a(4))).is_err());
    }

    #[test]
    fn insert_at_restores_index_and_generation() {
        let t = ObjectTable::isolated();
        t.insert_at(Handle::new(3, 7), a(3)).unwrap();
        assert_eq!(t.live(), 1);
        assert_eq!(t.get::<A>(Handle::new(3, 7)).unwrap().0, 3);
        // Slots 0..3 exist and are free; the next inserts fill them lowest-first.
        let h = t.insert(a(0));
        assert_eq!(
            h.index(),
            2,
            "free list pops the most recently created gap first"
        );
        let mut seen = vec![h.index()];
        seen.push(t.insert(a(1)).index());
        seen.push(t.insert(a(2)).index());
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2]);
        assert_eq!(
            t.insert(a(4)).index(),
            4,
            "then fresh slots after the restored one"
        );
    }

    #[test]
    fn insert_at_rejects_bad_handles_and_occupied_slots() {
        let t = ObjectTable::isolated();
        assert_eq!(t.insert_at(Handle::NULL, a(0)), Err(InsertAtError::Invalid));
        assert_eq!(
            t.insert_at(Handle::new(4, 0), a(0)),
            Err(InsertAtError::Invalid)
        );
        // The last generation would leave the counter nothing to issue.
        assert_eq!(
            t.insert_at(Handle::new(4, Handle::MAX_GENERATION), a(0)),
            Err(InsertAtError::Invalid)
        );
        t.insert_at(Handle::new(1, 1), a(1)).unwrap();
        assert_eq!(
            t.insert_at(Handle::new(1, 9), a(2)),
            Err(InsertAtError::Occupied)
        );
        assert_eq!(
            t.insert_at(Handle::new(Handle::MAX_INDEX, 1), a(3)),
            Err(InsertAtError::IndexTooFar)
        );
        assert_eq!(t.live(), 1);
    }

    #[test]
    fn clear_invalidates_every_handle_and_returns_the_objects() {
        let t = ObjectTable::isolated();
        let h1 = t.insert(a(1));
        let h2 = t.insert(a(2));
        let cleared = t.clear();
        assert_eq!(cleared.len(), 2);
        assert_eq!(cleared[0].handle, h1);
        assert_eq!(cleared[1].handle, h2);
        assert_eq!(t.live(), 0);
        assert!(t.get::<A>(h1).is_err() && t.get::<A>(h2).is_err());
        let h3 = t.insert(a(3));
        assert_eq!(h3.index(), 0);
        assert_eq!(
            h3.generation(),
            3,
            "a cleared slot never gets an old generation back"
        );
        assert!(t.get::<A>(h1).is_err() && t.get::<A>(h3).is_ok());
    }

    #[test]
    fn a_raised_floor_moves_every_later_generation_above_it_and_never_lowers() {
        let t = ObjectTable::isolated();
        t.raise_generation_floor(10);
        assert_eq!(t.generation_floor(), 10);
        t.insert_at(Handle::new(2, 4), a(0)).unwrap();
        assert_eq!(
            t.generation_floor(),
            10,
            "a restored generation below the floor changes nothing"
        );
        assert_eq!(t.insert(a(1)).generation(), 11);
        t.raise_generation_floor(3);
        assert_eq!(t.generation_floor(), 11, "never lowered");
        assert_eq!(t.insert(a(2)).generation(), 12);
    }

    #[test]
    fn the_process_floor_follows_the_shared_counter_and_ignores_isolated_tables() {
        let shared = ObjectTable::new();
        let first = shared.insert(a(0));
        assert!(process_generation_floor() >= first.generation());
        let second = shared.insert(a(1));
        assert!(process_generation_floor() >= second.generation());
        assert!(second.generation() > first.generation());
        // A table with a counter of its own does not move the process one.
        let isolated = ObjectTable::isolated();
        isolated.raise_generation_floor(Handle::MAX_GENERATION / 2);
        isolated.insert(a(2));
        assert!(process_generation_floor() < Handle::MAX_GENERATION / 2);
    }

    #[test]
    fn insert_at_keeps_the_counter_above_every_live_generation() {
        let t = ObjectTable::isolated();
        t.insert_at(Handle::new(1, 50), a(0)).unwrap();
        assert_eq!(t.generation_floor(), 50);
        let fresh = t.insert(a(1));
        assert_eq!(fresh.generation(), 51);
    }

    #[test]
    fn observed_tracks_all_and_individual_signals() {
        let mut o = Observed::default();
        assert!(o.is_empty());
        o.record(2, true, 4);
        o.record(0, true, 4);
        assert_eq!(o.to_reobserve(), [0, 2]);
        o.record(undra_meta::ids::ALL_SIGNALS, true, 4);
        assert_eq!(o.to_reobserve(), [undra_meta::ids::ALL_SIGNALS]);
        o.record(1, false, 4); // all but signal 1
        assert_eq!(o.to_reobserve(), [0, 2, 3]);
        o.record(undra_meta::ids::ALL_SIGNALS, false, 4);
        assert!(o.is_empty());
    }

    #[test]
    fn observed_ignores_signal_ids_the_store_does_not_have() {
        let mut o = Observed::default();
        o.record(9, true, 4);
        o.record(u32::MAX - 1, true, 4);
        assert!(o.is_empty(), "host-supplied unknown ids are not remembered");
        o.record(undra_meta::ids::ALL_SIGNALS, true, 4);
        o.record(9, false, 4);
        assert_eq!(o.to_reobserve(), [undra_meta::ids::ALL_SIGNALS]);
    }

    #[test]
    fn with_observed_and_poison_flags_need_a_live_handle() {
        let t = ObjectTable::isolated();
        let h = t.insert(a(1));
        assert_eq!(t.with_observed(h, |o| o.record(0, true, 1)), Some(()));
        assert!(t.mark_poisoned(h));
        t.release(h).unwrap();
        assert!(t.with_observed(h, |_| ()).is_none());
        assert!(!t.mark_poisoned(h));
    }

    // ----- the page servers of a store's `Lazy` signals (ADR-043) -----------------------------

    use crate::ctx::Ctx;
    use crate::lazy::PageServer;
    use crate::object::{StoreObject, store};
    use undra_signals::{Lazy, StoreCell};
    use undra_wire::{Reader, WireError};

    /// A store with two lazy lists and a plain signal; `serving` decides whether its cell asks the
    /// runtime to serve them (what `#[undra::store]` does for a store with a `Lazy` field).
    struct Shelf {
        cell: Arc<StoreCell>,
        books: Lazy<u32>,
    }

    impl UndraObject for Shelf {
        const TYPE_ID: u32 = 31;
        const NAME: &'static str = "Shelf";
    }

    impl StoreObject for Shelf {
        fn cell(&self) -> &Arc<StoreCell> {
            &self.cell
        }

        fn restore(_: Ctx, _: &mut Reader<'_>) -> Result<Self, WireError> {
            Err(WireError::UnexpectedEof { at: 0, needed: 1 })
        }
    }

    fn shelf(serving: bool) -> Arc<Shelf> {
        let cell = StoreCell::new(31);
        let books = Lazy::from_vec(vec![10_u32, 20, 30]);
        let tags = Lazy::from_vec(vec![String::from("x")]);
        cell.attach_lazy(&books, 0).unwrap();
        cell.attach(&undra_signals::Signal::new(0_u8), 1).unwrap();
        cell.attach_lazy(&tags, 2).unwrap();
        if serving {
            crate::lazy::serve_lazy_lists(&cell);
        }
        Arc::new(Shelf { cell, books })
    }

    fn page_len(t: &ObjectTable, handle: Handle) -> Option<usize> {
        t.get::<PageServer>(handle)
            .ok()
            .map(|server| server.source.len())
    }

    #[test]
    fn a_store_enters_with_a_transient_page_server_per_lazy_signal() {
        let t = ObjectTable::isolated();
        let shelf = shelf(true);
        let h = t.insert(store(shelf.clone()));
        // The store and its two page servers; the host owns the store's reference only.
        assert_eq!((t.live(), t.host_refs(), t.store_count()), (3, 1, 1));
        let (books, tags) = (shelf.cell.lazy_handle(0), shelf.cell.lazy_handle(2));
        assert!(books != 0 && tags != 0 && books != tags);
        assert_eq!(shelf.cell.lazy_handle(1), 0, "a plain signal has none");
        assert_eq!(page_len(&t, Handle(books)), Some(3));
        assert_eq!(page_len(&t, Handle(tags)), Some(1));
        // The servers are objects of the table's own: no host reference, a name, no store.
        assert_eq!(t.host_refs_of(Handle(books)), Some(0));
        assert_eq!(t.type_of(Handle(books)).unwrap().1, "LazyList");
        assert!(t.get_dyn(Handle(books)).unwrap().as_store().is_none());
        // The server shares the store's list.
        // (A table of its own has no core to hold: the write check is lifted for this one.)
        crate::testing::unchecked_writes(|| shelf.books.push(40));
        assert_eq!(page_len(&t, Handle(books)), Some(4));
        // Only the store is listed for a snapshot.
        let listed = t.stores();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, h);
    }

    #[test]
    fn the_host_cannot_release_a_page_server_and_the_store_takes_them_with_it() {
        let t = ObjectTable::isolated();
        let shelf = shelf(true);
        let h = t.insert(store(shelf.clone()));
        let books = Handle(shelf.cell.lazy_handle(0));
        // A host release of a page server is nothing: the list stays served.
        assert!(matches!(
            t.release(books),
            Ok(Released::Kept { remaining: 0 })
        ));
        assert_eq!(page_len(&t, books), Some(3));
        assert_eq!(t.live(), 3);

        // Releasing the store removes its servers and the cell forgets their handles.
        let tags = Handle(shelf.cell.lazy_handle(2));
        assert!(t.release(h).unwrap().into_removed().is_some());
        assert_eq!(t.live(), 0);
        assert_eq!(page_len(&t, books), None);
        assert_eq!(
            t.get::<PageServer>(tags).err().map(|e| e.reason),
            Some(BadHandleReason::Stale)
        );
        assert_eq!(
            (shelf.cell.lazy_handle(0), shelf.cell.lazy_handle(2)),
            (0, 0)
        );

        // The same store entering again (issued once more) is served by new servers.
        let again = t.insert(store(shelf.clone()));
        assert_ne!(again, h);
        assert_eq!(t.live(), 3);
        let fresh = Handle(shelf.cell.lazy_handle(0));
        assert!(fresh != books && page_len(&t, fresh) == Some(3));
        assert_eq!(
            page_len(&t, books),
            None,
            "the old handle stays stale for good"
        );
    }

    #[test]
    fn a_restore_places_the_servers_too_and_a_clear_takes_everything() {
        let t = ObjectTable::isolated();
        let shelf = shelf(true);
        t.insert_at(Handle::new(4, 9), store(shelf.clone()))
            .unwrap();
        assert_eq!(t.live(), 3);
        assert_eq!(page_len(&t, Handle(shelf.cell.lazy_handle(0))), Some(3));
        let old = Handle(shelf.cell.lazy_handle(0));
        let cleared = t.clear();
        assert_eq!(cleared.len(), 3, "the store and its page servers");
        assert_eq!(t.live(), 0);
        assert_eq!(page_len(&t, old), None);
    }

    #[test]
    fn a_store_whose_cell_does_not_serve_registers_nothing() {
        let t = ObjectTable::isolated();
        let shelf = shelf(false);
        t.insert(store(shelf.clone()));
        assert_eq!(t.live(), 1);
        assert_eq!(shelf.cell.lazy_handle(0), 0);
    }
}
