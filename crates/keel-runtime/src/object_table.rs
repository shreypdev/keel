//! The object table: a generation-tagged slab that issues and resolves handles (SPEC 1.2, 5.4).
//!
//! A [`Handle`] packs a slot index (low 32 bits) and a generation (high 32 bits, starting at
//! 1). Releasing an object bumps its slot's generation, so a stale handle is rejected instead
//! of aliasing whatever object reuses the slot.
//!
//! The table has its own reader-writer lock and is safe to use from anywhere, including from
//! inside a dispatcher that runs under the core lock. Objects are `Arc`s, so an object
//! outlives its handle while a task still holds it.
//!
//! ```
//! use std::sync::Arc;
//! use keel_runtime::object_table::ObjectTable;
//! use keel_runtime::{KeelObject, plain};
//!
//! struct Counter;
//! impl KeelObject for Counter {
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

use core::fmt;
use std::collections::BTreeSet;
use std::sync::Arc;

use parking_lot::RwLock;

pub use keel_wire::Handle;

use crate::object::AnyObject;

/// How far past the current end of the table [`ObjectTable::insert_at`] will extend it. A
/// corrupt snapshot must not be able to make the table allocate gigabytes.
const MAX_INDEX_GAP: usize = 1 << 20;

/// The largest slot index [`Runtime::restore`](crate::Runtime::restore) accepts in a snapshot.
pub(crate) const MAX_RESTORE_INDEX: usize = MAX_INDEX_GAP;

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
    /// The null handle, or a generation of `0`.
    Invalid,
    /// The slot is occupied.
    Occupied,
    /// The index is implausibly far beyond the end of the table.
    IndexTooFar,
}

impl fmt::Display for InsertAtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InsertAtError::Invalid => "null handle or generation 0",
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
    pub(crate) fn record(&mut self, signal_id: u32, on: bool, signal_count: u32) {
        if signal_id == keel_signals::ALL_SIGNALS {
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
            vec![keel_signals::ALL_SIGNALS]
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
    /// Generation given to slots that are created without an object (gaps, fresh slots after
    /// a restore); raised by restore so handles from before it cannot alias new objects.
    min_generation: u32,
}

fn next_generation(generation: u32) -> u32 {
    match generation.wrapping_add(1) {
        0 => 1,
        n => n,
    }
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
}

impl Default for ObjectTable {
    fn default() -> Self {
        ObjectTable::new()
    }
}

impl ObjectTable {
    /// Creates an empty table.
    pub fn new() -> ObjectTable {
        ObjectTable {
            inner: RwLock::new(Inner {
                slots: Vec::new(),
                free: Vec::new(),
                live: 0,
                stores: BTreeSet::new(),
                min_generation: 1,
            }),
        }
    }

    /// Stores `object` and returns its handle. If the object is a store, its cell learns its
    /// handle.
    ///
    /// # Panics
    ///
    /// Panics if the table would need more than `u32::MAX` slots.
    pub fn insert(&self, object: Arc<dyn AnyObject>) -> Handle {
        let is_store = object.as_store().is_some();
        let cell = object.as_store().cloned();
        let handle = {
            let mut inner = self.inner.write();
            let index = match inner.free.pop() {
                Some(index) => index,
                None => {
                    let index = u32::try_from(inner.slots.len())
                        .unwrap_or_else(|_| panic!("keel-runtime: object table is full"));
                    let generation = inner.min_generation;
                    inner.slots.push(Slot {
                        generation,
                        entry: None,
                    });
                    index
                }
            };
            let slot = &mut inner.slots[index as usize];
            slot.entry = Some(Entry {
                object,
                poisoned: false,
                observed: Observed::default(),
            });
            let handle = Handle::new(index, slot.generation);
            inner.live += 1;
            if is_store {
                inner.stores.insert(index);
            }
            handle
        };
        if let Some(cell) = cell {
            cell.set_handle(handle.0);
        }
        handle
    }

    /// Stores `object` at exactly `handle`: the restore path (SPEC 5.9). Fails if the slot is
    /// occupied. Slots below `handle.index()` that do not exist yet are created vacant.
    pub fn insert_at(
        &self,
        handle: Handle,
        object: Arc<dyn AnyObject>,
    ) -> Result<(), InsertAtError> {
        if handle.is_null() || handle.generation() == 0 {
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
                let generation = inner.min_generation;
                inner.slots.push(Slot {
                    generation,
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

    /// Resolves `handle` to a `T`.
    pub fn get<T: Send + Sync + 'static>(&self, handle: Handle) -> Result<Arc<T>, BadHandle> {
        let object = self.get_dyn(handle)?;
        object.downcast::<T>().ok_or(BadHandle {
            handle,
            reason: BadHandleReason::WrongType {
                expected: core::any::type_name::<T>(),
                found: object.keel_type_name(),
            },
        })
    }

    /// Removes the object behind `handle` and returns it, so the caller can drop it wherever
    /// it wants (the runtime drops it under the core lock). The slot's generation is bumped
    /// and the slot becomes reusable.
    pub fn release(&self, handle: Handle) -> Result<Arc<dyn AnyObject>, BadHandle> {
        let mut inner = self.inner.write();
        Self::check(&inner, handle)?;
        let slot = &mut inner.slots[handle.index() as usize];
        let entry = slot.entry.take();
        slot.generation = next_generation(slot.generation);
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

    /// Empties the table: every slot becomes vacant with a bumped generation, so every handle
    /// issued so far is stale. Returns what was in it.
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
                slot.generation = next_generation(slot.generation);
            }
            free.push(index as u32);
        }
        free.reverse(); // pop() hands out the lowest index first
        inner.free = free;
        inner.live = 0;
        inner.stores.clear();
        out
    }

    /// Raises the generation used for slots created from now on (never lowers it).
    pub(crate) fn raise_min_generation(&self, generation: u32) {
        let mut inner = self.inner.write();
        if generation > inner.min_generation {
            inner.min_generation = generation;
        }
        let floor = inner.min_generation;
        // Vacant slots keep the higher of their own generation and the floor.
        for slot in inner.slots.iter_mut() {
            if slot.entry.is_none() && slot.generation < floor {
                slot.generation = floor;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{KeelObject, plain};

    #[derive(Debug)]
    struct A(u32);
    impl KeelObject for A {
        const TYPE_ID: u32 = 1;
        const NAME: &'static str = "A";
    }
    #[derive(Debug)]
    struct B;
    impl KeelObject for B {
        const TYPE_ID: u32 = 2;
        const NAME: &'static str = "B";
    }

    fn a(n: u32) -> Arc<dyn AnyObject> {
        plain(Arc::new(A(n)))
    }

    #[test]
    fn first_handle_is_index_zero_generation_one() {
        let t = ObjectTable::new();
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
        let t = ObjectTable::new();
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
    fn generation_wraps_past_zero() {
        assert_eq!(next_generation(1), 2);
        assert_eq!(next_generation(u32::MAX), 1);
    }

    #[test]
    fn insert_at_restores_index_and_generation() {
        let t = ObjectTable::new();
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
        let t = ObjectTable::new();
        assert_eq!(t.insert_at(Handle::NULL, a(0)), Err(InsertAtError::Invalid));
        assert_eq!(
            t.insert_at(Handle::new(4, 0), a(0)),
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
        let t = ObjectTable::new();
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
        assert_eq!(h3.generation(), 2);
    }

    #[test]
    fn raised_generation_floor_protects_gaps_and_fresh_slots() {
        let t = ObjectTable::new();
        t.raise_min_generation(10);
        t.insert_at(Handle::new(2, 4), a(0)).unwrap();
        let fresh = t.insert(a(1));
        // Gap slots 0 and 1 were created at the floor.
        assert_eq!(fresh.generation(), 10);
        let fresh2 = t.insert(a(2));
        assert_eq!(fresh2.generation(), 10);
        let after = t.insert(a(3));
        assert_eq!((after.index(), after.generation()), (3, 10));
    }

    #[test]
    fn observed_tracks_all_and_individual_signals() {
        let mut o = Observed::default();
        assert!(o.is_empty());
        o.record(2, true, 4);
        o.record(0, true, 4);
        assert_eq!(o.to_reobserve(), [0, 2]);
        o.record(keel_signals::ALL_SIGNALS, true, 4);
        assert_eq!(o.to_reobserve(), [keel_signals::ALL_SIGNALS]);
        o.record(1, false, 4); // all but signal 1
        assert_eq!(o.to_reobserve(), [0, 2, 3]);
        o.record(keel_signals::ALL_SIGNALS, false, 4);
        assert!(o.is_empty());
    }

    #[test]
    fn with_observed_and_poison_flags_need_a_live_handle() {
        let t = ObjectTable::new();
        let h = t.insert(a(1));
        assert_eq!(t.with_observed(h, |o| o.record(0, true, 1)), Some(()));
        assert!(t.mark_poisoned(h));
        t.release(h).unwrap();
        assert!(t.with_observed(h, |_| ()).is_none());
        assert!(!t.mark_poisoned(h));
    }
}
