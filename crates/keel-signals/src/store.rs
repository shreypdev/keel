//! [`StoreCell`]: the per-store table that connects signals to the host.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use keel_wire::payload::{ChangeEntry, ChangeOp, ChangeSetBuilder, StoreSnapshot};
use keel_wire::{Handle, KeyedPatch, Writer};
use parking_lot::{Mutex, RwLock};

use crate::computed::Computed;
use crate::error::SignalsError;
use crate::graph::{Binding, SlotFlags};
use crate::signal::Signal;
use crate::sink::ChangeSink;
use crate::txn::{next_txn_id, recycle_buffer, take_buffer};
use crate::value::{KeyFn, ListLike, SignalValue};

/// The `signal_id` that means "every signal of the store" (SPEC 1.1).
pub const ALL_SIGNALS: u32 = u32::MAX;

type Encoder = Box<dyn Fn(&mut Writer) + Send + Sync>;

/// What a keyed slot produced for one commit.
enum PatchOrFull {
    /// The bytes are a keyed patch (`ChangeOp::KeyedPatch`).
    Patch,
    /// The bytes are the full value (`ChangeOp::Full`).
    Full,
}

/// The state a keyed list slot needs beyond its encoder: the list as the host last saw it.
trait KeyedState: Send + Sync {
    /// Writes what changed since the last call as a patch, or the full value when a patch is
    /// not possible or not worthwhile, and remembers the current list.
    fn diff(&self, w: &mut Writer) -> PatchOrFull;
    /// Writes the full value and remembers the current list as the host's baseline.
    fn resync(&self, w: &mut Writer);
    /// Drops the baseline (the slot is no longer observed).
    fn forget(&self);
}

enum SlotKind {
    Plain,
    Keyed(Box<dyn KeyedState>),
    Computed,
}

struct Slot {
    flags: Arc<SlotFlags>,
    /// Writes the current full value.
    encode: Encoder,
    kind: SlotKind,
}

/// The signal table of one store instance.
///
/// A store (a struct with `#[keel::store]`) owns one `StoreCell`. Generated code creates it with
/// `StoreCell::new(type_id)` and attaches every signal field in declaration order; the runtime
/// then assigns the store's handle ([`set_handle`](StoreCell::set_handle)) and drives
/// observation. The cell knows how to encode each signal, which ones the host observes, and
/// which ones changed in the current transaction. `Send + Sync`.
///
/// # Delivery rules
///
/// * A slot is *observed* between `observe(id, true)` and `observe(id, false)`. Only observed
///   slots (and `no_coalesce` slots) are put in change-sets.
/// * A write marks its slot *dirty* and queues it in the writing thread's transaction. A slot
///   that is dirty but unobserved stays dirty and costs nothing further per write.
/// * `observe(id, true)` always sends the slot's **current** value and clears its dirty bit, so
///   a host that starts observing never needs the writes it missed. Calling it again on an
///   observed slot re-sends the value: that is how a host resynchronises after a bad patch
///   (SPEC 3.8).
/// * Entries of one change-set are ordered by `signal_id`.
///
/// # Memory cost of keyed lists
///
/// For a keyed list ([`attach_keyed`](StoreCell::attach_keyed)) the cell keeps a copy of the list
/// as the host last saw it, so the next commit can compute a patch. The copy exists only while
/// the slot is observed and costs one clone of the list (`Vec<Item>`) per observed keyed
/// signal. Each commit costs O(n) to compute the patch (key hashing plus an encoded comparison
/// of surviving items) but only clones the items that changed.
///
/// # Example
///
/// ```
/// use keel_signals::{Computed, Signal, StoreCell, ALL_SIGNALS};
/// use keel_wire::Writer;
///
/// // What `#[keel::store]` generates for `struct Counter { count: Signal<i32>, double: Computed<i32> }`:
/// let cell = StoreCell::new(0xC0DE);
/// let count = Signal::new(1);
/// let double = Computed::new(&count, |n| n * 2);
/// cell.attach(&count, 0).unwrap();
/// cell.attach_computed(&double, 1).unwrap();
///
/// // The runtime hands the store a handle when it enters the object table...
/// cell.set_handle(0x0000_0001_0000_0003);
/// // ...and the host starts observing: the current values come back immediately.
/// let mut entries = Writer::new();
/// let n = cell.observe(ALL_SIGNALS, true, &mut entries);
/// assert_eq!(n, 2);
/// ```
pub struct StoreCell {
    type_id: u32,
    handle: AtomicU64,
    slots: RwLock<Vec<Arc<Slot>>>,
}

impl StoreCell {
    /// Creates an empty cell for a store type (`type_id` is `fnv1a32("<TypeName>")`).
    pub fn new(type_id: u32) -> Arc<StoreCell> {
        Arc::new(StoreCell {
            type_id,
            handle: AtomicU64::new(0),
            slots: RwLock::new(Vec::new()),
        })
    }

    /// Binds `signal` to the next slot, which must be numbered `signal_id`.
    ///
    /// Generated code calls this once per signal field, in declaration order. Changes to the
    /// signal are delivered as full values.
    ///
    /// # Errors
    ///
    /// * [`SignalsError::AlreadyAttached`] if the signal already belongs to a store;
    /// * [`SignalsError::OutOfOrder`] if `signal_id` is not the number of signals attached so
    ///   far.
    ///
    /// On error nothing is changed.
    pub fn attach<T: SignalValue>(
        self: &Arc<Self>,
        signal: &Signal<T>,
        signal_id: u32,
    ) -> Result<(), SignalsError> {
        let source = signal.clone();
        let encode: Encoder = Box::new(move |w| source.with(|value| value.encode(w)));
        self.install(signal_id, &signal.inner.binding, encode, SlotKind::Plain)
    }

    /// Like [`attach`](StoreCell::attach) for a `Signal<Vec<T>>` marked
    /// `#[keel(key = "..")]`: changes are delivered as keyed patches (SPEC 3.8) whenever that
    /// is possible, and as full values otherwise.
    ///
    /// `key` maps an item to the `u64` that identifies it (generated code hashes the encoded key
    /// field). See the [type-level docs](StoreCell#memory-cost-of-keyed-lists) for the memory cost.
    ///
    /// # Errors
    ///
    /// Same as [`attach`](StoreCell::attach).
    pub fn attach_keyed<T: SignalValue + ListLike>(
        self: &Arc<Self>,
        signal: &Signal<T>,
        signal_id: u32,
        key: KeyFn<T>,
    ) -> Result<(), SignalsError> {
        let source = signal.clone();
        let encode: Encoder = Box::new(move |w| source.with(|value| value.encode(w)));
        let state = KeyedList {
            signal: signal.clone(),
            key,
            baseline: Mutex::new(None),
        };
        self.install(
            signal_id,
            &signal.inner.binding,
            encode,
            SlotKind::Keyed(Box::new(state)),
        )
    }

    /// Binds a computed to the next slot. Its value is delivered as a full value whenever it is
    /// observed and its inputs changed; it is recomputed at commit only while observed.
    ///
    /// # Errors
    ///
    /// Same as [`attach`](StoreCell::attach).
    pub fn attach_computed<T: SignalValue>(
        self: &Arc<Self>,
        computed: &Computed<T>,
        signal_id: u32,
    ) -> Result<(), SignalsError> {
        let source = computed.clone();
        let encode: Encoder = Box::new(move |w| source.with(|value| value.encode(w)));
        self.install(
            signal_id,
            &computed.inner.binding,
            encode,
            SlotKind::Computed,
        )
    }

    fn install(
        self: &Arc<Self>,
        signal_id: u32,
        binding: &OnceLock<Binding>,
        encode: Encoder,
        kind: SlotKind,
    ) -> Result<(), SignalsError> {
        let mut slots = self.slots.write();
        if binding.get().is_some() {
            return Err(SignalsError::AlreadyAttached);
        }
        let expected = u32::try_from(slots.len()).unwrap_or(ALL_SIGNALS);
        if signal_id != expected || signal_id == ALL_SIGNALS {
            return Err(SignalsError::OutOfOrder {
                expected,
                got: signal_id,
            });
        }
        let flags = Arc::new(SlotFlags::default());
        let bound = Binding {
            cell: Arc::downgrade(self),
            signal_id,
            flags: Arc::clone(&flags),
        };
        if binding.set(bound).is_err() {
            return Err(SignalsError::AlreadyAttached);
        }
        slots.push(Arc::new(Slot {
            flags,
            encode,
            kind,
        }));
        Ok(())
    }

    /// Makes changes to signal `signal_id` reach the host even while it is unobserved
    /// (`#[keel(no_coalesce)]`, SPEC 4.3): every commit that dirties the signal is delivered.
    ///
    /// # Errors
    ///
    /// [`SignalsError::UnknownSignal`] if no such signal has been attached.
    pub fn set_no_coalesce(&self, signal_id: u32) -> Result<(), SignalsError> {
        match self.slot(signal_id) {
            Some(slot) => {
                slot.flags.no_coalesce.store(true, Ordering::SeqCst);
                Ok(())
            }
            None => Err(SignalsError::UnknownSignal { signal_id }),
        }
    }

    /// Records the store's handle. The runtime calls this when the store enters the object
    /// table, before anything observes it; change-sets carry this handle.
    ///
    /// Until a handle is set (it is `0`), commits consume the store's pending changes without
    /// delivering them: writes before the store is published are plain writes, and the host
    /// gets the values from its first `observe`.
    pub fn set_handle(&self, handle: u64) {
        self.handle.store(handle, Ordering::SeqCst);
    }

    /// The handle set by [`set_handle`](StoreCell::set_handle), or `0`.
    pub fn handle(&self) -> u64 {
        self.handle.load(Ordering::SeqCst)
    }

    /// The store type's id.
    pub fn type_id(&self) -> u32 {
        self.type_id
    }

    /// Number of signals (including computeds) attached so far.
    pub fn signal_count(&self) -> u32 {
        u32::try_from(self.slots.read().len()).unwrap_or(ALL_SIGNALS)
    }

    /// Returns `true` if the host currently observes signal `signal_id`.
    pub fn is_observed(&self, signal_id: u32) -> bool {
        self.slot(signal_id)
            .is_some_and(|slot| slot.flags.observed.load(Ordering::SeqCst))
    }

    /// Starts or stops observation of one signal, or of all of them (`signal_id ==`
    /// [`ALL_SIGNALS`]).
    ///
    /// With `on == true`, appends one change-set **entry** per targeted signal to `out`, each
    /// carrying the signal's current full value (SPEC 3.5 entry layout:
    /// `handle u64, signal_id u32, op u8, len u32, value`), and returns how many were written.
    /// Entries are written even for signals that were already observed: re-observing is how a
    /// host resynchronises. The caller wraps the entries into a payload
    /// (`txn_id u64, count u32, entries`), typically with [`next_txn_id`].
    ///
    /// With `on == false`, stops delivery for the targeted signals, writes nothing and returns 0.
    ///
    /// An unknown `signal_id` is ignored (returns 0); debug builds assert, because it means the
    /// caller's schema and the store disagree.
    ///
    /// Call [`set_handle`](StoreCell::set_handle) first: entries carry the current handle.
    ///
    /// # Example
    ///
    /// ```
    /// use keel_signals::{next_txn_id, Signal, StoreCell};
    /// use keel_wire::payload::ChangeSet;
    /// use keel_wire::{Reader, Writer};
    ///
    /// let cell = StoreCell::new(1);
    /// let name = Signal::new(String::from("ada"));
    /// cell.attach(&name, 0).unwrap();
    /// cell.set_handle(0x0000_0001_0000_0001);
    ///
    /// let mut entries = Writer::new();
    /// let count = cell.observe(0, true, &mut entries);
    ///
    /// let mut payload = Writer::new();
    /// payload.write_u64(next_txn_id());
    /// payload.write_u32(count);
    /// payload.write_raw(entries.as_slice());
    /// let change_set = ChangeSet::decode(&mut Reader::new(payload.as_slice())).unwrap();
    /// assert_eq!(change_set.entries[0].signal_id, 0);
    /// ```
    pub fn observe(&self, signal_id: u32, on: bool, out: &mut Writer) -> u32 {
        let targets = self.targets(signal_id);
        if targets.is_empty() {
            debug_assert!(
                signal_id == ALL_SIGNALS,
                "keel-signals: observe of unknown signal id {signal_id}"
            );
            return 0;
        }
        let handle = Handle(self.handle());
        let mut written = 0_u32;
        for (id, slot) in targets {
            if !on {
                slot.flags.observed.store(false, Ordering::SeqCst);
                if let SlotKind::Keyed(state) = &slot.kind {
                    state.forget();
                }
                continue;
            }
            slot.flags.observed.store(true, Ordering::SeqCst);
            // Clear the dirty bit *before* reading the value: a write that lands in between is
            // recorded again and delivered by its own commit, instead of being lost.
            slot.flags.dirty.store(false, Ordering::SeqCst);
            let mut value = Writer::new();
            match &slot.kind {
                SlotKind::Keyed(state) => state.resync(&mut value),
                SlotKind::Plain | SlotKind::Computed => (slot.encode)(&mut value),
            }
            ChangeEntry {
                handle,
                signal_id: id,
                op: ChangeOp::Full,
                value: value.into_vec(),
            }
            .encode(out);
            written += 1;
        }
        written
    }

    /// Appends the full encoded value of one signal to `out` (no entry header, no length).
    /// Returns `false`, writing nothing, if there is no such signal.
    ///
    /// Reads only: it does not change observation, dirtiness or keyed baselines.
    pub fn encode_signal(&self, signal_id: u32, out: &mut Writer) -> bool {
        match self.slot(signal_id) {
            Some(slot) => {
                (slot.encode)(out);
                true
            }
            None => false,
        }
    }

    /// Appends this store's body of a snapshot (SPEC 5.9): `handle u64, type_id u32,
    /// signal_count u32, signals x { signal_id u32, len u32, value }`. Computed signals are
    /// left out; they are recomputed on restore.
    pub fn encode_snapshot(&self, out: &mut Writer) {
        let slots: Vec<Arc<Slot>> = self.slots.read().clone();
        let mut signals = Vec::with_capacity(slots.len());
        for (index, slot) in slots.iter().enumerate() {
            if matches!(slot.kind, SlotKind::Computed) {
                continue;
            }
            let mut value = Writer::new();
            (slot.encode)(&mut value);
            let id = u32::try_from(index).unwrap_or(ALL_SIGNALS);
            signals.push((id, value.into_vec()));
        }
        StoreSnapshot {
            handle: Handle(self.handle()),
            type_id: self.type_id,
            signals,
        }
        .encode(out);
    }

    fn slot(&self, signal_id: u32) -> Option<Arc<Slot>> {
        self.slots.read().get(signal_id as usize).cloned()
    }

    fn targets(&self, signal_id: u32) -> Vec<(u32, Arc<Slot>)> {
        let slots = self.slots.read();
        if signal_id == ALL_SIGNALS {
            slots
                .iter()
                .enumerate()
                .map(|(i, slot)| (u32::try_from(i).unwrap_or(ALL_SIGNALS), Arc::clone(slot)))
                .collect()
        } else {
            slots
                .get(signal_id as usize)
                .map(|slot| vec![(signal_id, Arc::clone(slot))])
                .unwrap_or_default()
        }
    }

    /// Commits the slots `ids` that a transaction dirtied: builds one change-set from those
    /// that are observed (or `no_coalesce`) and delivers it through `sink`.
    ///
    /// Slots that are dirty but unobserved are left dirty. Slots that would be delivered but
    /// cannot be (no sink installed, or the store has no handle yet) are consumed, since the
    /// host cannot have missed something it never had; its next `observe` sends the current
    /// value.
    ///
    /// `txn_id` is the transaction id of the current round, allocated on first use so that
    /// every store committed by the same transaction shares one id.
    pub(crate) fn commit_slots(
        &self,
        mut ids: Vec<u32>,
        sink: Option<&Arc<dyn ChangeSink>>,
        txn_id: &mut Option<u64>,
    ) {
        ids.sort_unstable();
        ids.dedup();

        // Claim the slots that will be delivered *before* encoding anything (see
        // `SlotFlags::dirty`). Only atomics are touched under the table lock, and the claimed
        // slots are cloned out of it, so no lock is held while user code (computed closures,
        // encoders) runs below.
        let claimed: Vec<(u32, Arc<Slot>)> = {
            let table = self.slots.read();
            let mut claimed = Vec::with_capacity(ids.len());
            for &id in &ids {
                let Some(slot) = table.get(id as usize) else {
                    continue;
                };
                let flags = &slot.flags;
                let wanted = flags.observed.load(Ordering::SeqCst)
                    || flags.no_coalesce.load(Ordering::SeqCst);
                if wanted && flags.dirty.swap(false, Ordering::SeqCst) {
                    claimed.push((id, Arc::clone(slot)));
                }
            }
            claimed
        };
        if claimed.is_empty() {
            return;
        }

        let handle = self.handle();
        let Some(sink) = sink else { return };
        if handle == 0 {
            return;
        }
        let handle = Handle(handle);
        let txn = *txn_id.get_or_insert_with(next_txn_id);

        let mut payload = Writer::from_vec(take_buffer());
        let mut scratch = Writer::new();
        let mut builder = ChangeSetBuilder::new(&mut payload, txn);
        for (id, slot) in &claimed {
            match &slot.kind {
                SlotKind::Keyed(state) => {
                    scratch.clear();
                    let op = match state.diff(&mut scratch) {
                        PatchOrFull::Patch => ChangeOp::KeyedPatch,
                        PatchOrFull::Full => ChangeOp::Full,
                    };
                    builder.push(handle, *id, op, scratch.as_slice());
                }
                SlotKind::Plain | SlotKind::Computed => {
                    builder.push_with(handle, *id, ChangeOp::Full, |w| (slot.encode)(w));
                }
            }
        }
        builder.finish();
        sink.deliver(payload.as_slice());
        recycle_buffer(payload.into_vec());
    }
}

impl fmt::Debug for StoreCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreCell")
            .field("type_id", &format_args!("{:#010x}", self.type_id))
            .field("handle", &format_args!("{:#x}", self.handle()))
            .field("signals", &self.signal_count())
            .finish()
    }
}

/// A keyed list slot: the signal, its key function and the list as the host last saw it.
struct KeyedList<T: ListLike> {
    signal: Signal<T>,
    key: KeyFn<T>,
    /// The items the host has, in order; `None` until the first `resync`/`diff` and after
    /// `forget`.
    baseline: Mutex<Option<Vec<T::Item>>>,
}

impl<T: SignalValue + ListLike> KeyedState for KeyedList<T> {
    fn diff(&self, w: &mut Writer) -> PatchOrFull {
        // Lock order: baseline, then the signal's value lock. `resync` does the same.
        let mut baseline = self.baseline.lock();
        self.signal.with(|current| {
            let items = current.items();
            let patch = baseline
                .as_deref()
                .and_then(|old| diff_items(old, items, self.key));
            match patch {
                Some(patch) => {
                    patch.encode(w);
                    // Keep the baseline in step by replaying the patch: only the changed items
                    // are cloned. `diff` never yields a patch that does not apply; if it ever
                    // did, re-cloning the list is the safe fallback.
                    let replayed = baseline
                        .as_mut()
                        .is_some_and(|old| patch.apply(old).is_ok());
                    if !replayed {
                        *baseline = Some(items.to_vec());
                    }
                    #[cfg(debug_assertions)]
                    if let Some(old) = baseline.as_deref() {
                        let old_keys: Vec<u64> = old.iter().map(self.key).collect();
                        let new_keys: Vec<u64> = items.iter().map(self.key).collect();
                        debug_assert_eq!(
                            old_keys, new_keys,
                            "keyed baseline drifted from the list"
                        );
                    }
                    PatchOrFull::Patch
                }
                None => {
                    current.encode(w);
                    *baseline = Some(items.to_vec());
                    PatchOrFull::Full
                }
            }
        })
    }

    fn resync(&self, w: &mut Writer) {
        let mut baseline = self.baseline.lock();
        self.signal.with(|current| {
            current.encode(w);
            *baseline = Some(current.items().to_vec());
        });
    }

    fn forget(&self) {
        *self.baseline.lock() = None;
    }
}

/// The patch that turns `old` into `new`, or `None` when the full value should be sent.
///
/// Items with equal keys are compared by their **encoded bytes**, which is exactly what the
/// host would see, and needs no `PartialEq` on the item type. Two scratch buffers are reused
/// for all comparisons.
fn diff_items<I: SignalValue>(old: &[I], new: &[I], key: fn(&I) -> u64) -> Option<KeyedPatch<I>> {
    let scratch = RefCell::new((Writer::new(), Writer::new()));
    KeyedPatch::diff(old, new, key, |a, b| {
        let mut buffers = scratch.borrow_mut();
        let (left, right) = &mut *buffers;
        left.clear();
        a.encode(left);
        right.clear();
        b.encode(right);
        left.as_slice() == right.as_slice()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CaptureSink;
    use crate::{txn, with_sink};

    #[test]
    fn new_cell_is_empty() {
        let cell = StoreCell::new(0xABCD);
        assert_eq!(cell.type_id(), 0xABCD);
        assert_eq!(cell.handle(), 0);
        assert_eq!(cell.signal_count(), 0);
        assert!(!cell.is_observed(0));
    }

    #[test]
    fn debug_output_is_readable() {
        let cell = StoreCell::new(0xAB);
        cell.set_handle(0x1_0000_0002);
        let text = format!("{cell:?}");
        assert!(text.contains("type_id: 0x000000ab"), "{text}");
        assert!(text.contains("handle: 0x100000002"), "{text}");
        assert!(text.contains("signals: 0"), "{text}");
    }

    #[test]
    fn diff_items_uses_encoded_equality() {
        // The u8 payload is ignored by the key, so only encoding can tell items apart.
        let old = vec![(1_u32, 1_u8), (2, 2)];
        let same = old.clone();
        let changed = vec![(1_u32, 1_u8), (2, 9)];
        fn key(item: &(u32, u8)) -> u64 {
            u64::from(item.0)
        }
        let patch = diff_items(&old, &same, key).expect("patch");
        assert!(patch.is_empty());
        let patch = diff_items(&old, &changed, key).expect("patch");
        assert_eq!(patch.len(), 1);
    }

    #[test]
    fn claimed_slots_are_delivered_sorted_and_deduplicated() {
        let cell = StoreCell::new(1);
        let a = Signal::new(0_u8);
        let b = Signal::new(0_u8);
        let c = Signal::new(0_u8);
        cell.attach(&a, 0).unwrap();
        cell.attach(&b, 1).unwrap();
        cell.attach(&c, 2).unwrap();
        cell.set_handle(7);
        cell.observe(ALL_SIGNALS, true, &mut Writer::new());

        let sink = CaptureSink::new();
        with_sink(sink.clone(), || {
            txn(|| {
                c.set(3);
                a.set(1);
                b.set(2);
                a.set(4);
            });
        });
        let sets = sink.take_decoded();
        assert_eq!(sets.len(), 1);
        let ids: Vec<u32> = sets[0].entries.iter().map(|e| e.signal_id).collect();
        assert_eq!(ids, vec![0, 1, 2]);
        assert_eq!(sets[0].entries[0].value, vec![4]);
    }
}
