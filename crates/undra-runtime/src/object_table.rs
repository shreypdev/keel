//! The object table: a generation-tagged slab that issues and resolves handles (SPEC 1.2, 5.4).
//!
//! A [`Handle`] packs a slot index (low 32 bits) and a generation (high 32 bits, starting at
//! 1). Every handle the table issues gets a **fresh generation from one process-wide,
//! monotonically increasing counter**, so a `(slot, generation)` pair is never issued twice in a
//! process: a stale handle is rejected instead of aliasing whatever object reuses the slot, and
//! that holds across [`restore`](crate::Runtime::restore) too (a snapshot carries the counter's
//! high-water mark, ADR-022). The counter has 2^32 - 1 values; when it is spent the table
//! refuses to issue more handles (a v1 limit, see ADR-022).
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

use crate::atomic_update::cas_update;
use core::fmt;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use parking_lot::RwLock;

pub use undra_wire::Handle;

use crate::object::AnyObject;

/// How far past the current end of the table [`ObjectTable::insert_at`] will extend it. A
/// corrupt snapshot must not be able to make the table allocate gigabytes.
const MAX_INDEX_GAP: usize = 1 << 20;

/// The largest slot index [`Runtime::restore`](crate::Runtime::restore) accepts in a snapshot.
pub(crate) const MAX_RESTORE_INDEX: usize = MAX_INDEX_GAP;

/// The highest generation (and snapshot `generation_floor`) a restore accepts. The counter is
/// shared by every runtime in the process, so obeying a floor near `u32::MAX` would let one
/// corrupt or hostile snapshot exhaust handle creation process-wide, permanently across
/// relaunches (crash recovery restores the same bytes). 2^24 of headroom keeps ~16.7 million
/// issues available after the most adversarial accepted snapshot.
pub(crate) const GENERATION_CEILING: u32 = u32::MAX - (1 << 24);

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
    /// The null handle, a generation of `0`, or a generation of `u32::MAX` (which would leave
    /// the generation counter nothing to issue).
    Invalid,
    /// The slot is occupied.
    Occupied,
    /// The index is implausibly far beyond the end of the table.
    IndexTooFar,
}

impl fmt::Display for InsertAtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InsertAtError::Invalid => "null handle, generation 0 or generation u32::MAX",
            InsertAtError::Occupied => "the slot is occupied",
            InsertAtError::IndexTooFar => "the slot index is too far beyond the table",
        })
    }
}

impl std::error::Error for InsertAtError {}

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
}

struct Slot {
    generation: u32,
    entry: Option<Entry>,
}

struct Inner {
    slots: Vec<Slot>,
    free: Vec<u32>,
    live: usize,
    stores: BTreeSet<u32>,
}

/// The source of handle generations: a counter of the highest generation issued so far
/// (`0` = none yet). Issuing is one atomic increment; generation `0` is never issued (it is
/// the invalid value) and the counter never wraps: at `u32::MAX` it is exhausted for good.
pub(crate) struct Generations {
    last: AtomicU32,
    /// The exhaustion has been reported (the FATAL record is written once per counter).
    exhaustion_reported: AtomicBool,
}

impl Generations {
    const fn new() -> Generations {
        Generations {
            last: AtomicU32::new(0),
            exhaustion_reported: AtomicBool::new(false),
        }
    }

    /// The next generation, or `None` if all `u32::MAX` of them have been issued.
    fn issue(&self) -> Option<u32> {
        cas_update(&self.last, Ordering::AcqRel, Ordering::Acquire, |last| {
            last.checked_add(1)
        })
        .ok()
        .map(|previous| previous + 1)
    }

    /// The highest generation issued so far.
    fn last(&self) -> u32 {
        self.last.load(Ordering::Acquire)
    }

    /// Makes sure nothing at or below `floor` is issued from now on. Never lowers the counter.
    fn raise_to(&self, floor: u32) {
        self.last.fetch_max(floor, Ordering::AcqRel);
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
pub fn process_generation_floor() -> u32 {
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
    pub(crate) fn isolated_after(last: u32) -> ObjectTable {
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
    pub(crate) fn generation_floor(&self) -> u32 {
        self.generations().last()
    }

    /// Raises the generation counter to at least `floor` (restore). Never lowers it.
    pub(crate) fn raise_generation_floor(&self, floor: u32) {
        self.generations().raise_to(floor);
    }

    /// Takes the next generation, or panics with a clear message once the counter is spent.
    fn issue_generation(&self) -> u32 {
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
                        "handle generations exhausted: 2^32 - 1 handles have been issued in this \
                         process; no further object can be created (restart the core)",
                    );
                }
                panic!(
                    "undra-runtime: handle generations are exhausted (2^32 - 1 handles have been \
                     issued in this process); restart the core"
                )
            }
        }
    }

    /// Stores `object` and returns its handle. If the object is a store, its cell learns its
    /// handle.
    ///
    /// # Panics
    ///
    /// Panics if the table would need more than `u32::MAX` slots, or once the process has
    /// issued `u32::MAX` handles and the generation counter is spent (logged at FATAL first;
    /// ADR-022). Both are contained at the runtime's entry points like any other panic.
    pub fn insert(&self, object: Arc<dyn AnyObject>) -> Handle {
        let is_store = object.as_store().is_some();
        let cell = object.as_store().cloned();
        // Before any lock is taken and before the table is touched, so a refusal (which logs)
        // leaves it exactly as it was and calls out with no lock held.
        let generation = self.issue_generation();
        let handle = {
            let mut inner = self.inner.write();
            let index = match inner.free.pop() {
                Some(index) => index,
                None => {
                    let index = u32::try_from(inner.slots.len())
                        .unwrap_or_else(|_| panic!("undra-runtime: object table is full"));
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
            });
            let handle = Handle::new(index, generation);
            inner.live += 1;
            if is_store {
                inner.stores.insert(index);
            }
            handle
        };
        if let Some(cell) = cell {
            // The owner first: a commit that sees the handle must route to the right runtime.
            cell.set_owner(self.owner.load(Ordering::Relaxed));
            cell.set_handle(handle.0);
        }
        handle
    }

    /// Stores `object` at exactly `handle`: the restore path (SPEC 5.9). Fails if the slot is
    /// occupied. Slots below `handle.index()` that do not exist yet are created vacant. The
    /// generation counter is raised to the handle's generation, so the table never issues a
    /// generation that a live handle already carries.
    pub fn insert_at(
        &self,
        handle: Handle,
        object: Arc<dyn AnyObject>,
    ) -> Result<(), InsertAtError> {
        if handle.is_null() || handle.generation() == 0 || handle.generation() == u32::MAX {
            return Err(InsertAtError::Invalid);
        }
        let index = handle.index() as usize;
        let cell = object.as_store().cloned();
        let is_store = cell.is_some();
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
            });
            inner.live += 1;
            if is_store {
                inner.stores.insert(handle.index());
            }
        }
        if let Some(cell) = cell {
            cell.set_owner(self.owner.load(Ordering::Relaxed));
            cell.set_handle(handle.0);
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

    /// Removes the object behind `handle` and returns it, so the caller can drop it wherever
    /// it wants (the runtime drops it under the core lock). The slot becomes reusable; the next
    /// object placed in it gets a fresh generation, so `handle` stays stale for good.
    pub fn release(&self, handle: Handle) -> Result<Arc<dyn AnyObject>, BadHandle> {
        let mut inner = self.inner.write();
        Self::check(&inner, handle)?;
        let slot = &mut inner.slots[handle.index() as usize];
        let entry = slot.entry.take();
        inner.free.push(handle.index());
        inner.live -= 1;
        inner.stores.remove(&handle.index());
        match entry {
            Some(entry) => Ok(entry.object),
            // `check` proved the slot occupied while we held the write lock.
            None => Err(BadHandle {
                handle,
                reason: BadHandleReason::Stale,
            }),
        }
    }

    /// Number of live objects.
    pub fn live(&self) -> usize {
        self.inner.read().live
    }

    /// Number of live stores.
    pub fn store_count(&self) -> usize {
        self.inner.read().stores.len()
    }

    /// Every live store with its handle, in slot order.
    pub(crate) fn stores(&self) -> Vec<(Handle, Arc<dyn AnyObject>)> {
        let inner = self.inner.read();
        inner
            .stores
            .iter()
            .filter_map(|&index| {
                let slot = inner.slots.get(index as usize)?;
                let entry = slot.entry.as_ref()?;
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
        inner.stores.clear();
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
        assert_eq!(t.release(h1).unwrap().downcast::<A>().unwrap().0, 1);
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
        drop(t.release(h).unwrap());
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
    fn the_counter_is_spent_after_u32_max_handles_and_refuses_cleanly() {
        let t = ObjectTable::isolated_after(u32::MAX - 2);
        let last = t.insert(a(1));
        let last2 = t.insert(a(2));
        assert_eq!(
            (last.generation(), last2.generation()),
            (u32::MAX - 1, u32::MAX)
        );
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
        // A generation of u32::MAX would leave the counter nothing to issue.
        assert_eq!(
            t.insert_at(Handle::new(4, u32::MAX), a(0)),
            Err(InsertAtError::Invalid)
        );
        t.insert_at(Handle::new(1, 1), a(1)).unwrap();
        assert_eq!(
            t.insert_at(Handle::new(1, 9), a(2)),
            Err(InsertAtError::Occupied)
        );
        assert_eq!(
            t.insert_at(Handle::new(u32::MAX, 1), a(3)),
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
        isolated.raise_generation_floor(u32::MAX / 2);
        isolated.insert(a(2));
        assert!(process_generation_floor() < u32::MAX / 2);
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
}
