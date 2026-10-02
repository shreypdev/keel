//! [`StoreCell`]: the per-store table that connects signals to the host.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, RwLock};
use undra_wire::payload::{ChangeEntry, ChangeOp, ChangeSetBuilder, StoreSnapshot};
use undra_wire::{Encode, Handle, KeyedPatch, Writer};

use crate::computed::Computed;
use crate::derived::DerivedList;
use crate::derived::slot::{Attached, DerivedSlot, Emitted};
use crate::error::SignalsError;
use crate::graph::{Binding, SlotFlags};
use crate::lazy::{Lazy, LazyCell, LazyEmit, LazySlot, LazySource, isolated};
use crate::oplog::{KeyedLog, ListLog, Taken, apply_ops};
use crate::signal::Signal;
use crate::sink::ChangeSink;
use crate::txn::{TxnGuard, next_txn_id, recycle_buffer, take_buffer};
use crate::value::{KeyFn, ListLike, SignalValue};

/// The `signal_id` that means "every signal of the store" (SPEC 1.1).
pub const ALL_SIGNALS: u32 = u32::MAX;

type Encoder = Box<dyn Fn(&mut Writer) + Send + Sync>;

/// How many times `observe` re-encodes its targets while computed closures keep writing to
/// them (see [`StoreCell::observe`]). Every pass but the last one is only needed when a closure
/// writes, so a store without such closures pays for exactly one.
const OBSERVE_SETTLE_PASSES: usize = 8;

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
    ///
    /// With `retain == false` (the slot is delivered only because it is `no_coalesce`, the host
    /// does not observe it) the full value is always written and no baseline is kept: nobody
    /// could apply a patch, and the copy of the list would live as long as the store.
    fn diff(&self, w: &mut Writer, retain: bool) -> PatchOrFull;
    /// Writes the full value and remembers the current list as the host's baseline.
    fn resync(&self, w: &mut Writer);
    /// Drops the baseline (the slot is no longer observed).
    fn forget(&self);
}

enum SlotKind {
    Plain,
    Keyed(Box<dyn KeyedState>),
    Computed,
    /// A derived list (ADR-039): a computed that ships keyed patches. Isolated like a computed
    /// when its closures panic (ADR-019 amendment).
    Derived(Box<dyn DerivedSlot>),
    /// A `Lazy<T>` (ADR-043): the host pages it, and is told by length and version, never by value.
    /// Persisted like a plain signal (a snapshot carries the items).
    Lazy(Box<dyn LazySlot>),
}

impl SlotKind {
    /// A slot evaluated by the core rather than written: left out of snapshots, held back on its
    /// own when its evaluation panics.
    fn is_computed(&self) -> bool {
        matches!(self, SlotKind::Computed | SlotKind::Derived(_))
    }

    /// Drops whatever the slot keeps about the host's copy (a keyed baseline, a derived list's
    /// pending ops).
    fn forget(&self) {
        match self {
            SlotKind::Keyed(state) => state.forget(),
            SlotKind::Derived(state) => state.forget(),
            SlotKind::Lazy(state) => state.forget(),
            SlotKind::Plain | SlotKind::Computed => {}
        }
    }
}

struct Slot {
    flags: Arc<SlotFlags>,
    /// Writes the current full value.
    encode: Encoder,
    kind: SlotKind,
}

/// The signal table of one store instance.
///
/// A store (a struct with `#[undra::store]`) owns one `StoreCell`. Generated code creates it with
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
/// * A computed whose evaluation panics is left out and held back on its own (ADR-019
///   amendment): the rest of the change-set is delivered, the slot is listed in
///   [`failed_signals`](StoreCell::failed_signals), and it is evaluated again when its inputs
///   change.
/// * A change-set whose building or delivery otherwise panicked (an encoder, the sink) is
///   abandoned whole, and its slots are remembered: the next commit that touches the store sends
///   them again as full values (keyed baselines and the op logs recorded against them are
///   dropped), so the host cannot be left with values the core has moved on from.
///
/// # Keyed lists: O(change), not O(list)
///
/// For a keyed list ([`attach_keyed`](StoreCell::attach_keyed)) the cell keeps a copy of the list
/// as the host last saw it (the *baseline*), so that a commit can say what changed. There are
/// two ways it finds out, and the write decides which (ADR-027):
///
/// * **Recorded operations** ([`Signal::push`], [`insert`](Signal::insert),
///   [`remove`](Signal::remove), [`update_at`](Signal::update_at),
///   [`move_item`](Signal::move_item), [`clear`](Signal::clear)) append the SPEC 3.8 op they
///   perform to a log as they perform it. The commit sends the log as the patch and replays it
///   on the baseline: O(ops), whatever the list's length (the `memmove` a `Vec` needs for an
///   insertion in the middle aside, which the host pays too). Key hashing and item comparison
///   do not happen at all.
/// * **Raw writes** ([`Signal::set`], [`update`](Signal::update), [`replace`](Signal::replace))
///   invalidate the log, and the commit **diffs** the list against the baseline: O(list) (key
///   hashing plus an encoded comparison of surviving items), sending the full value when more
///   than half of the items were removed or no key overlaps. A transaction that mixes the two
///   is diffed.
///
/// The baseline exists only while the slot is observed and costs one clone of the list per
/// observed keyed signal; the op log holds at most as many ops as the list has items (at least
/// 4096), and a transaction that records more makes the commit diff instead. Both are dropped
/// when the host stops observing the slot or a delivery of it is abandoned, and nothing is
/// recorded for a slot nobody observes.
///
/// # Example
///
/// ```
/// use undra_signals::{Computed, Signal, StoreCell, ALL_SIGNALS};
/// use undra_wire::Writer;
///
/// // What `#[undra::store]` generates for `struct Counter { count: Signal<i32>, double: Computed<i32> }`:
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
    /// The id of the runtime that owns the store (`0` until published), shared with every
    /// signal's binding so a write can be checked against it cheaply (ADR-035).
    owner: Arc<AtomicU64>,
    slots: RwLock<Vec<Arc<Slot>>>,
    /// Slots whose change was claimed for delivery but never reached the sink, sorted (see
    /// `commit_slots`). The next commit that touches this store sends them again.
    unsent: Mutex<Vec<u32>>,
    /// `unsent` is not empty. Lets the common commit skip the lock.
    has_unsent: AtomicBool,
    /// Held by `commit_slots` from the claim until the sink has returned, so that the
    /// change-sets of one store reach the sink in the order they were claimed, whatever threads
    /// commit them.
    delivery: Mutex<()>,
    /// The `txn_id` of the change-set delivered last; guarded by `delivery`.
    last_txn: AtomicU64,
    /// The computed slots whose evaluation panicked, with the panic message: held back until
    /// they evaluate again (ADR-019 amendment). The slot's `failed` flag is the fast check.
    failed: Mutex<BTreeMap<u32, String>>,
}

/// What evaluating the computeds of one delivery did to their failed state: reported to the sink
/// after the store's delivery lock is released.
#[derive(Default)]
struct Health {
    /// Slots that panicked, with the message.
    failed: Vec<(u32, String)>,
    /// Slots that had failed and evaluated successfully again.
    recovered: Vec<u32>,
}

impl Health {
    fn is_empty(&self) -> bool {
        self.failed.is_empty() && self.recovered.is_empty()
    }
}

/// The message of a caught panic.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with a non-string payload".to_owned()
    }
}

/// Encodes a computed slot's value into `out` (which is cleared first) with its panic caught: the
/// one place a computed's closure runs during a delivery (ADR-019 amendment).
fn encode_computed(slot: &Slot, out: &mut Writer) -> Result<(), String> {
    out.clear();
    catch_unwind(AssertUnwindSafe(|| (slot.encode)(out)))
        .map_err(|payload| panic_message(&*payload))
}

impl StoreCell {
    /// Creates an empty cell for a store type (`type_id` is `fnv1a32("<TypeName>")`).
    pub fn new(type_id: u32) -> Arc<StoreCell> {
        Arc::new(StoreCell {
            type_id,
            handle: AtomicU64::new(0),
            owner: Arc::new(AtomicU64::new(0)),
            slots: RwLock::new(Vec::new()),
            unsent: Mutex::new(Vec::new()),
            has_unsent: AtomicBool::new(false),
            delivery: Mutex::new(()),
            last_txn: AtomicU64::new(0),
            failed: Mutex::new(BTreeMap::new()),
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
    /// `#[undra(key = "..")]`: changes are delivered as keyed patches (SPEC 3.8) whenever that
    /// is possible, and as full values otherwise.
    ///
    /// `key` maps an item to the `u64` that identifies it (generated code hashes the encoded key
    /// field); it is only called when a commit has to diff the list. The signal's recorded list
    /// operations ([`Signal::push`] and friends) start logging into the slot from now on, which
    /// makes their commits O(ops); see the [type-level docs](StoreCell#keyed-lists-ochange-not-olist)
    /// for that and for the memory cost.
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
        let log = Arc::new(KeyedLog::new());
        let state = KeyedList {
            signal: signal.clone(),
            key,
            baseline: Mutex::new(None),
            log: Arc::clone(&log),
        };
        self.install(
            signal_id,
            &signal.inner.binding,
            encode,
            SlotKind::Keyed(Box::new(state)),
        )?;
        // The signal's recorded list operations (`push`, `insert`, ..) log into this from now
        // on; until the host observes the slot the log stays disarmed and records nothing.
        let log: Arc<dyn ListLog> = log;
        let _ = signal.inner.log.set(log);
        Ok(())
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

    /// Binds a [`DerivedList`] to the next slot (ADR-039). The host receives its full value when it
    /// observes it, then keyed patches (SPEC 3.8) made of the list's own derived ops: one source
    /// operation is at most two ops, a transaction's ops are one patch, and a commit whose
    /// operations did not change the view adds no entry. A raw write of the source, more pending ops
    /// than the list keeps, or a parameter change that moves more than 256 rows sends the full value
    /// instead.
    ///
    /// `key` maps a row to the `u64` that identifies it (generated code hashes the encoded key
    /// field); maintenance never uses it, and debug builds check with it at every full value that
    /// the view's keys are unique.
    ///
    /// Like a computed, a derived list whose closures panic while a commit or an observe evaluates
    /// it is held back on its own ([`failed_signals`](StoreCell::failed_signals)); it is rebuilt and
    /// sent as a full value when its inputs next change and the evaluation succeeds.
    ///
    /// # Errors
    ///
    /// Same as [`attach`](StoreCell::attach): a list attaches to one store slot for life.
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::{Signal, StoreCell, ALL_SIGNALS};
    /// use undra_wire::Writer;
    ///
    /// let cell = StoreCell::new(7);
    /// let numbers = Signal::new(vec![1_u32, 2, 3, 4]);
    /// let even = numbers.derive().filter(|n| n % 2 == 0).build();
    /// cell.attach_keyed(&numbers, 0, |n| u64::from(*n)).unwrap();
    /// cell.attach_derived(&even, 1, |n| u64::from(*n)).unwrap();
    /// cell.set_handle(0x1_0000_0001);
    /// assert_eq!(cell.observe(ALL_SIGNALS, true, &mut Writer::new()), 2);
    /// ```
    pub fn attach_derived<T: SignalValue>(
        self: &Arc<Self>,
        list: &DerivedList<T>,
        signal_id: u32,
        key: fn(&T) -> u64,
    ) -> Result<(), SignalsError> {
        let source = list.clone();
        let encode: Encoder = Box::new(move |w| source.with(|value| Encode::encode(value, w)));
        let slot = Attached {
            node: Arc::clone(&list.inner),
            key,
        };
        self.install(
            signal_id,
            list.inner.binding(),
            encode,
            SlotKind::Derived(Box::new(slot)),
        )
    }

    /// Binds a [`Lazy`] list to the next slot (ADR-043). The host never receives its items by
    /// value: observing the slot sends `LazyValue { handle, len, version }` (change-set op 0), where
    /// `handle` is the page server the runtime registered for the slot (see
    /// [`lazy_sources`](StoreCell::lazy_sources)), and every commit that follows a change sends
    /// `LazyInvalidated { len, version }` (op 2, 12 bytes, whatever and however many the changes
    /// were). The host then asks for the window it shows with page calls. A snapshot carries the
    /// items of an owned list.
    ///
    /// A view ([`Lazy::over`]) is announced when its derived list's view changes; a write of the
    /// source that the view ignores sends nothing. Like a derived list, a view whose pipeline
    /// panics while a commit or an observe evaluates it is held back on its own
    /// ([`failed_signals`](StoreCell::failed_signals)).
    ///
    /// # Errors
    ///
    /// Same as [`attach`](StoreCell::attach).
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::{Lazy, StoreCell, ALL_SIGNALS};
    /// use undra_wire::Writer;
    ///
    /// let cell = StoreCell::new(7);
    /// let books = Lazy::from_vec(vec![1_u32, 2, 3]);
    /// cell.attach_lazy(&books, 0).unwrap();
    /// cell.set_handle(0x1_0000_0001);
    /// // The runtime registers `cell.lazy_sources()` and tells the cell the handle of each.
    /// cell.set_lazy_handle(0, 0x1_0000_0002);
    /// assert_eq!(cell.observe(ALL_SIGNALS, true, &mut Writer::new()), 1);
    /// ```
    pub fn attach_lazy<T: SignalValue>(
        self: &Arc<Self>,
        lazy: &Lazy<T>,
        signal_id: u32,
    ) -> Result<(), SignalsError> {
        let snapshot = lazy.clone();
        let encode: Encoder = Box::new(move |w| snapshot.encode_snapshot(w));
        let source: Arc<dyn LazySource> = Arc::new(lazy.clone());
        self.install(
            signal_id,
            lazy.binding(),
            encode,
            SlotKind::Lazy(Box::new(LazyCell::new(source))),
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
            owner: Arc::clone(&self.owner),
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
    /// (`#[undra(no_coalesce)]`, SPEC 4.3): every commit that dirties the signal is delivered.
    /// While the signal is unobserved a keyed list is delivered as a full value each time (there
    /// is no baseline to patch against, and none is kept).
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

    /// Records the id of the runtime that owns the store. The runtime calls this wherever it
    /// sets the handle (inserting the store, restoring it), before the handle (ADR-035).
    ///
    /// Two things follow from it: a write to one of the store's signals is allowed only on a
    /// thread that holds **that** runtime's core lock (the checker of
    /// [`set_write_checker`](crate::set_write_checker) is asked about this id), and the store's
    /// change-sets go to that runtime ([`ChangeSink::deliver_from`]), whichever thread commits.
    pub fn set_owner(&self, runtime_id: u64) {
        self.owner.store(runtime_id, Ordering::SeqCst);
    }

    /// The owner recorded by [`set_owner`](StoreCell::set_owner), or `0`.
    pub fn owner(&self) -> u64 {
        self.owner.load(Ordering::SeqCst)
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

    /// The computed signals of this store whose last evaluation panicked, with the panic
    /// message, in id order: the typed *poisoned* state of a signal (ADR-019 amendment). Such a
    /// signal is held back (the host keeps the last value it received) while every other signal
    /// of the store keeps being delivered; it is evaluated again when its inputs change, and
    /// leaves this list when that succeeds.
    pub fn failed_signals(&self) -> Vec<(u32, String)> {
        self.failed
            .lock()
            .iter()
            .map(|(id, message)| (*id, message.clone()))
            .collect()
    }

    /// Whether the computed `signal_id` is currently held back because its evaluation panicked
    /// (see [`failed_signals`](StoreCell::failed_signals)).
    pub fn is_failed(&self, signal_id: u32) -> bool {
        self.slot(signal_id)
            .is_some_and(|slot| slot.flags.failed.load(Ordering::SeqCst))
    }

    /// The lazy lists of this store: each `Lazy` signal's id and its page server, in id order.
    ///
    /// The runtime calls this when the store enters its object table, registers each page server
    /// there (a transient entry that lives and dies with the store) and tells the cell the handle
    /// ([`set_lazy_handle`](StoreCell::set_lazy_handle)). Empty for a store without a `Lazy`.
    pub fn lazy_sources(&self) -> Vec<(u32, Arc<dyn LazySource>)> {
        let slots = self.slots.read();
        slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match &slot.kind {
                SlotKind::Lazy(lazy) => {
                    Some((u32::try_from(index).unwrap_or(ALL_SIGNALS), lazy.source()))
                }
                _ => None,
            })
            .collect()
    }

    /// Records the object-table handle of the page server of lazy signal `signal_id`: what its
    /// `LazyValue` entries carry. Ignored for a signal that is not a `Lazy`.
    pub fn set_lazy_handle(&self, signal_id: u32, handle: u64) {
        if let Some(slot) = self.slot(signal_id) {
            if let SlotKind::Lazy(lazy) = &slot.kind {
                lazy.set_handle(handle);
            }
        }
    }

    /// The handle recorded by [`set_lazy_handle`](StoreCell::set_lazy_handle) (`0` before, and for a
    /// signal that is not a `Lazy`).
    pub fn lazy_handle(&self, signal_id: u32) -> u64 {
        match self.slot(signal_id) {
            Some(slot) => match &slot.kind {
                SlotKind::Lazy(lazy) => lazy.handle(),
                _ => 0,
            },
            None => 0,
        }
    }

    /// Records what a delivery's computed evaluations did and tells the sink: once per transition
    /// into the failed state, and once when a failed slot recovers. Called with no lock held.
    fn note_health(&self, targets: &[(u32, Arc<Slot>)], health: Health) {
        if health.is_empty() {
            return;
        }
        let slot_of = |id: u32| {
            targets
                .iter()
                .find(|(slot_id, _)| *slot_id == id)
                .map(|(_, slot)| slot)
        };
        let mut newly_failed = Vec::new();
        let mut recovered = Vec::new();
        {
            let mut failed = self.failed.lock();
            for (id, message) in health.failed {
                let Some(slot) = slot_of(id) else { continue };
                if !slot.flags.failed.swap(true, Ordering::SeqCst) {
                    newly_failed.push((id, message.clone()));
                }
                failed.insert(id, message);
            }
            for id in health.recovered {
                let Some(slot) = slot_of(id) else { continue };
                if slot.flags.failed.swap(false, Ordering::SeqCst) {
                    recovered.push(id);
                }
                failed.remove(&id);
            }
        }
        let Some(sink) = crate::sink::current() else {
            return;
        };
        let (owner, handle) = (self.owner(), self.handle());
        // A sink that panics while reporting must not undo a delivery that already happened.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            for (id, message) in &newly_failed {
                sink.computed_failed(owner, handle, *id, message);
            }
            for id in &recovered {
                sink.computed_recovered(owner, handle, *id);
            }
        }));
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
    /// An unknown `signal_id` is ignored in every build (returns 0, writes nothing): it comes
    /// from the host, and host input must never be able to make the core assert.
    ///
    /// `observe(on)` runs inside a [transaction](crate::txn), so a computed's closure that writes
    /// signals while it is evaluated does not commit on the spot (which would put a change-set
    /// ahead of the entries the caller is about to deliver). The entries are re-encoded until no
    /// target was written while they were built (at most eight passes), so they hold the
    /// **post-write** values, and the writes are absorbed into them: their own commit, when the
    /// transaction ends, has nothing left to send for the targets. Writes to slots that were not
    /// targeted are committed normally at that point, after the entries have been built. A
    /// closure that writes one of its own inputs on every evaluation cannot be settled; its
    /// writes are then committed as they are.
    ///
    /// If building the entries panics (a computed's closure or an encoder), nothing is left half
    /// done: no target stays marked observed by this call, no keyed baseline is kept for a value
    /// the host never received, and `out` is untouched.
    ///
    /// Call [`set_handle`](StoreCell::set_handle) first: entries carry the current handle.
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::{next_txn_id, Signal, StoreCell};
    /// use undra_wire::payload::ChangeSet;
    /// use undra_wire::{Reader, Writer};
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
        let targets = self.targets_of(&[signal_id]);
        if targets.is_empty() {
            // An unknown id comes from the host: it is ignored in every build (never asserted),
            // and nothing is counted or logged here.
            return 0;
        }
        if !on {
            self.stop_observing(&targets);
            return 0;
        }

        // Computed closures may write signals while their values are encoded. Inside this
        // transaction such writes only queue up, and are committed after the entries are built
        // (the guard is declared first, so it is dropped last).
        let _txn = TxnGuard::enter();
        let mut rollback = self.start_observing(&targets);
        let (entries, count, health) = self.encode_settled(&targets);
        rollback.armed = false;
        // The host is about to receive the current value of every target, so an earlier
        // abandoned delivery of one of them no longer needs a retry.
        self.forget_unsent(targets.iter().map(|(id, _)| *id));
        out.write_raw(entries.as_slice());
        self.note_health(&targets, health);
        count
    }

    /// Starts observing `signal_ids` (any of them may be [`ALL_SIGNALS`]) and hands the host
    /// their current values as **one complete change-set** (`txn_id u64, count u32, entries`,
    /// SPEC 3.5) through `deliver`, **under the store's delivery lock**. Returns how many
    /// entries the change-set holds; when that is `0` (every id unknown) `deliver` is not
    /// called.
    ///
    /// This is [`observe`](StoreCell::observe) plus delivery, done the way a commit does it,
    /// and it exists for callers that must put the values in front of the host themselves (the
    /// runtime's `observe` and `restore`). Building the entries and handing them over are one
    /// step under the lock that orders this store's change-sets, with a transaction id allocated
    /// inside it (and recorded, so a later commit of the store never reuses or undercuts it).
    /// A commit of the same store on another thread therefore either delivered completely
    /// before this change-set, carrying older values, or waits for it and delivers after it, so
    /// the host's last word on the store is always its newest. Calling `observe` and delivering
    /// the entries separately cannot promise that: a commit can slip in between and the host
    /// ends on the older value.
    ///
    /// The entries are encoded and settled exactly as [`observe`](StoreCell::observe) does (inside a
    /// transaction, re-encoded while computed closures write their own targets, at most eight
    /// passes); writes that remain are committed **after** this call returns and the lock is
    /// released, as ordinary change-sets, so the host converges on the core's values. Ids are
    /// deduplicated and the entries are ordered by `signal_id`.
    ///
    /// If encoding or `deliver` panics, nothing is left half done: no target stays observed
    /// because of this call, baselines are dropped, and targets that were already observed are
    /// remembered as unsent, so the next commit of the store sends their current values.
    ///
    /// # Locking
    ///
    /// The delivery lock is held while computed closures and encoders run and while `deliver`
    /// runs, so `deliver` follows the [`ChangeSink`] contract: it must not wait for another
    /// thread that writes this store, and it must not call back into this cell's `observe`
    /// family or commit this store from the same thread while the lock is held (writes it makes
    /// are queued and committed afterwards, as for a sink). A computed closure must not block on
    /// such a thread either.
    ///
    /// # Example
    ///
    /// ```
    /// use undra_signals::{Signal, StoreCell, ALL_SIGNALS};
    /// use undra_wire::payload::ChangeSet;
    /// use undra_wire::Reader;
    ///
    /// let cell = StoreCell::new(1);
    /// let name = Signal::new(String::from("ada"));
    /// cell.attach(&name, 0).unwrap();
    /// cell.set_handle(0x0000_0001_0000_0001);
    ///
    /// let mut received = Vec::new();
    /// let count = cell.observe_and_deliver(&[ALL_SIGNALS], |payload| received.push(payload.to_vec()));
    /// assert_eq!(count, 1);
    /// let change_set = ChangeSet::decode(&mut Reader::new(&received[0])).unwrap();
    /// assert_eq!(change_set.entries[0].signal_id, 0);
    /// ```
    pub fn observe_and_deliver(&self, signal_ids: &[u32], deliver: impl FnOnce(&[u8])) -> u32 {
        let targets = self.targets_of(signal_ids);
        if targets.is_empty() {
            return 0;
        }
        // Declared first, so dropped last: writes that could not settle commit only after the
        // delivery lock below has been released (the commit takes it again).
        let _txn = TxnGuard::enter();
        let delivery = self.delivery.lock();
        let mut rollback = self.start_observing(&targets);
        let (entries, count, health) = self.encode_settled(&targets);

        // A target whose computed panicked has no entry (it stays observed and is sent when it
        // evaluates again); a change-set with no entry at all is not sent.
        if count > 0 {
            // Allocated under the lock, after everything delivered before it, and recorded, so
            // `commit_slots` replaces a shared transaction id that this one has overtaken.
            let txn = next_txn_id();
            self.last_txn.store(txn, Ordering::SeqCst);
            let mut payload = Writer::from_vec(take_buffer());
            payload.write_u64(txn);
            payload.write_u32(count);
            payload.write_raw(entries.as_slice());
            // A panic in `deliver` leaves the rollback armed: the host may not have these
            // values.
            deliver(payload.as_slice());
            recycle_buffer(payload.into_vec());
        }
        rollback.armed = false;
        self.forget_unsent(targets.iter().map(|(id, _)| *id));
        drop(delivery);
        self.note_health(&targets, health);
        count
    }

    /// Stops observing `targets`: their baselines are dropped and nothing is delivered for them.
    fn stop_observing(&self, targets: &[(u32, Arc<Slot>)]) {
        for (_, slot) in targets {
            slot.flags.observed.store(false, Ordering::SeqCst);
            slot.kind.forget();
        }
    }

    /// Marks `targets` observed and returns the guard that undoes it if building (or delivering)
    /// the entries unwinds: every target returns to its previous state, its keyed baseline is
    /// dropped, and a target that was already observed (whose dirty bit is cleared while the
    /// entries are built) is remembered as unsent.
    fn start_observing(&self, targets: &[(u32, Arc<Slot>)]) -> ObserveRollback<'_> {
        let mut rollback = ObserveRollback {
            cell: self,
            touched: Vec::with_capacity(targets.len()),
            armed: true,
        };
        for (id, slot) in targets {
            rollback.touched.push((
                *id,
                Arc::clone(slot),
                slot.flags.observed.swap(true, Ordering::SeqCst),
            ));
        }
        rollback
    }

    /// Encodes the current value of every target as change-set entries, re-encoding while a
    /// computed closure writes one of the targets (at most [`OBSERVE_SETTLE_PASSES`] passes), so
    /// that the entries hold the post-write values. The caller holds a transaction open.
    fn encode_settled(&self, targets: &[(u32, Arc<Slot>)]) -> (Writer, u32, Health) {
        let handle = Handle(self.handle());
        let mut entries = Writer::new();
        let mut count = 0_u32;
        let mut health = Health::default();
        for pass in 1..=OBSERVE_SETTLE_PASSES {
            entries.clear();
            count = 0;
            health = Health::default();
            for (id, slot) in targets {
                // Clear the dirty bit *before* reading the value: a write that lands in between
                // is recorded again and delivered by its own commit, instead of being lost.
                slot.flags.dirty.store(false, Ordering::SeqCst);
                let mut value = Writer::new();
                match &slot.kind {
                    SlotKind::Keyed(state) => state.resync(&mut value),
                    SlotKind::Plain => (slot.encode)(&mut value),
                    // A computed that panics is left out and held back, not the whole observe
                    // (ADR-019 amendment).
                    SlotKind::Computed => match encode_computed(slot, &mut value) {
                        Ok(()) => {
                            if slot.flags.failed.load(Ordering::SeqCst) {
                                health.recovered.push(*id);
                            }
                        }
                        Err(message) => {
                            health.failed.push((*id, message));
                            continue;
                        }
                    },
                    // A lazy list announces its length and version, never its items; a view's
                    // pipeline is isolated as a derived list's is.
                    SlotKind::Lazy(state) => match isolated(|| state.write_full(&mut value)) {
                        Ok(()) => {
                            if slot.flags.failed.load(Ordering::SeqCst) {
                                health.recovered.push(*id);
                            }
                        }
                        Err(message) => {
                            health.failed.push((*id, message));
                            continue;
                        }
                    },
                    // A derived list is isolated as a computed is.
                    SlotKind::Derived(state) => {
                        let resynced = catch_unwind(AssertUnwindSafe(|| {
                            value.clear();
                            state.resync(&mut value);
                        }));
                        match resynced {
                            Ok(()) => {
                                if slot.flags.failed.load(Ordering::SeqCst) {
                                    health.recovered.push(*id);
                                }
                            }
                            Err(payload) => {
                                health.failed.push((*id, panic_message(&*payload)));
                                continue;
                            }
                        }
                    }
                }
                ChangeEntry {
                    handle,
                    signal_id: *id,
                    op: ChangeOp::Full,
                    value: value.into_vec(),
                }
                .encode(&mut entries);
                count += 1;
            }
            // A target that is dirty again was written while the entries were being built (by
            // a computed's closure), possibly after its own entry was encoded: encode once more,
            // so that the host receives the post-write values in one coherent view. The commit
            // at the end of the transaction then finds those slots clean.
            let settled = targets
                .iter()
                .all(|(_, slot)| !slot.flags.dirty.load(Ordering::SeqCst));
            if settled || pass == OBSERVE_SETTLE_PASSES {
                break;
            }
        }
        (entries, count, health)
    }

    /// Appends the full encoded value of one signal to `out` (no entry header, no length).
    /// Returns `false`, writing nothing, if there is no such signal.
    ///
    /// Reads only: it does not change observation, dirtiness or keyed baselines.
    pub fn encode_signal(&self, signal_id: u32, out: &mut Writer) -> bool {
        match self.slot(signal_id) {
            Some(slot) => {
                // A computed's closure may write signals: hold the writes back until the value
                // has been produced instead of committing them ahead of it.
                let _txn = TxnGuard::enter();
                (slot.encode)(out);
                true
            }
            None => false,
        }
    }

    /// Appends this store's body of a snapshot (SPEC 5.9): `handle u64, type_id u32,
    /// signal_count u32, signals x { signal_id u32, len u32, value }`. Computed signals and
    /// derived lists are left out; they are recomputed on restore.
    pub fn encode_snapshot(&self, out: &mut Writer) {
        let slots: Vec<Arc<Slot>> = self.slots.read().clone();
        let mut signals = Vec::with_capacity(slots.len());
        for (index, slot) in slots.iter().enumerate() {
            if slot.kind.is_computed() {
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

    /// The slots `signal_ids` name (unknown ids are skipped), deduplicated and ordered by id;
    /// [`ALL_SIGNALS`] stands for every slot.
    fn targets_of(&self, signal_ids: &[u32]) -> Vec<(u32, Arc<Slot>)> {
        let slots = self.slots.read();
        if signal_ids.contains(&ALL_SIGNALS) {
            return slots
                .iter()
                .enumerate()
                .map(|(i, slot)| (u32::try_from(i).unwrap_or(ALL_SIGNALS), Arc::clone(slot)))
                .collect();
        }
        let mut ids = signal_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .filter_map(|id| slots.get(id as usize).map(|slot| (id, Arc::clone(slot))))
            .collect()
    }

    /// Commits the slots `ids` that a transaction dirtied: builds one change-set from those
    /// that are observed (or `no_coalesce`) and delivers it through `sink`.
    ///
    /// Slots that are dirty but unobserved are left dirty. Slots that would be delivered but
    /// cannot be (no sink installed, or the store has no handle yet) are consumed, since the
    /// host cannot have missed something it never had; its next `observe` sends the current
    /// value.
    ///
    /// The claim is transactional. If building the change-set or delivering it is abandoned
    /// (a computed or an encoder panicked, or the sink did), the claimed slots are remembered
    /// as *unsent* and the keyed baselines among them are dropped, so that the next commit that
    /// touches this store sends every one of them again as a full value. Without that, the host
    /// would silently keep values the core has moved on from.
    ///
    /// The store's delivery lock is held from the claim until `sink` has returned, so that
    /// commits of one store from different threads reach the sink one at a time, in claim
    /// order, with strictly increasing `txn_id`s.
    ///
    /// `txn_id` is the transaction id of the current round, allocated on first use so that
    /// every store committed by the same transaction shares one id.
    pub(crate) fn commit_slots(
        &self,
        mut ids: Vec<u32>,
        sink: Option<&Arc<dyn ChangeSink>>,
        txn_id: &mut Option<u64>,
    ) {
        // One commit of this store at a time, from the claim to the sink's return: a second
        // thread that dirtied the store waits here, so its change-set cannot overtake this one
        // (a patch computed against a baseline that a still-undelivered patch has already moved
        // would otherwise reach the host first). See the crate docs, "Threading".
        let delivery = self.delivery.lock();

        // Slots an earlier commit of this store abandoned are sent again with this one.
        let unsent = self.take_unsent();
        if !unsent.is_empty() {
            ids.extend_from_slice(&unsent);
        }
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
                if !wanted {
                    continue;
                }
                let dirty = flags.dirty.swap(false, Ordering::SeqCst);
                if dirty || unsent.binary_search(&id).is_ok() {
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
        // A transaction id is shared by every store the transaction commits, but the ids one
        // store sees must only ever grow, so a shared id that a concurrent commit of this store
        // has already overtaken is replaced (for this store and the ones after it).
        let mut txn = *txn_id.get_or_insert_with(next_txn_id);
        if txn <= self.last_txn.load(Ordering::SeqCst) {
            txn = next_txn_id();
            *txn_id = Some(txn);
        }
        self.last_txn.store(txn, Ordering::SeqCst);

        // From here until the sink has returned, the change-set is in flight: unwinding
        // abandons it (see `Abandon`).
        let mut abandon = Abandon {
            cell: self,
            claimed: &claimed,
            armed: true,
        };
        let mut payload = Writer::from_vec(take_buffer());
        let mut scratch = Writer::new();
        let mut builder = ChangeSetBuilder::new(&mut payload, txn);
        let mut health = Health::default();
        let mut entries = 0_usize;
        for (id, slot) in &claimed {
            match &slot.kind {
                SlotKind::Keyed(state) => {
                    scratch.clear();
                    // Decided once, here: an unobserved (`no_coalesce`) slot keeps no baseline.
                    let retain = slot.flags.observed.load(Ordering::SeqCst);
                    let op = match state.diff(&mut scratch, retain) {
                        PatchOrFull::Patch => ChangeOp::KeyedPatch,
                        PatchOrFull::Full => ChangeOp::Full,
                    };
                    builder.push(handle, *id, op, scratch.as_slice());
                }
                SlotKind::Plain => {
                    builder.push_with(handle, *id, ChangeOp::Full, |w| (slot.encode)(w));
                }
                // A computed is evaluated with its panic caught (ADR-019 amendment): one that
                // panics on its current inputs is left out and held back; the store's other
                // slots still go, and the write that triggered the commit succeeds.
                SlotKind::Computed => match encode_computed(slot, &mut scratch) {
                    Ok(()) => {
                        builder.push(handle, *id, ChangeOp::Full, scratch.as_slice());
                        if slot.flags.failed.load(Ordering::SeqCst) {
                            health.recovered.push(*id);
                        }
                    }
                    Err(message) => {
                        health.failed.push((*id, message));
                        continue;
                    }
                },
                // A lazy list sends 12 bytes of length and version, or nothing at all when the
                // host already knows them (ADR-043).
                SlotKind::Lazy(state) => {
                    let retain = slot.flags.observed.load(Ordering::SeqCst);
                    scratch.clear();
                    let emitted = isolated(|| state.commit(retain, &mut scratch));
                    let op = match emitted {
                        Ok(LazyEmit::Invalidated) => ChangeOp::LazyInvalidated,
                        Ok(LazyEmit::Full) => ChangeOp::Full,
                        Ok(LazyEmit::Nothing) => continue,
                        Err(message) => {
                            health.failed.push((*id, message));
                            continue;
                        }
                    };
                    builder.push(handle, *id, op, scratch.as_slice());
                    if slot.flags.failed.load(Ordering::SeqCst) {
                        health.recovered.push(*id);
                    }
                }
                // A derived list sends its pending derived ops, the full value, or nothing at
                // all (ADR-039); its closures' panics are isolated as a computed's are.
                SlotKind::Derived(state) => {
                    let retain = slot.flags.observed.load(Ordering::SeqCst);
                    let emitted = catch_unwind(AssertUnwindSafe(|| {
                        scratch.clear();
                        state.commit(&mut scratch, retain)
                    }));
                    let op = match emitted {
                        Ok(Emitted::Patch) => ChangeOp::KeyedPatch,
                        Ok(Emitted::Full) => ChangeOp::Full,
                        Ok(Emitted::Nothing) => continue,
                        Err(payload) => {
                            health.failed.push((*id, panic_message(&*payload)));
                            continue;
                        }
                    };
                    builder.push(handle, *id, op, scratch.as_slice());
                    if slot.flags.failed.load(Ordering::SeqCst) {
                        health.recovered.push(*id);
                    }
                }
            }
            entries += 1;
        }
        builder.finish();
        if entries > 0 {
            sink.deliver_from(self.owner(), payload.as_slice());
        }
        abandon.armed = false;
        recycle_buffer(payload.into_vec());
        drop(delivery);
        self.note_health(&claimed, health);
    }

    /// Releases a slot that a cut-off commit still had queued: it is clean again (so any thread's
    /// next write to it is recorded and committed) and remembered as unsent, so that the next
    /// commit of this store delivers its current value.
    pub(crate) fn defer(&self, signal_id: u32) {
        if let Some(slot) = self.slot(signal_id) {
            slot.flags.dirty.store(false, Ordering::SeqCst);
        }
        self.mark_unsent(signal_id);
    }

    /// Remembers that the host may not have the current value of `signal_id`: the next commit
    /// that touches this store sends it again.
    fn mark_unsent(&self, signal_id: u32) {
        let mut unsent = self.unsent.lock();
        if let Err(at) = unsent.binary_search(&signal_id) {
            unsent.insert(at, signal_id);
        }
        self.has_unsent.store(true, Ordering::Release);
    }

    /// Takes the sorted list of slots to send again.
    fn take_unsent(&self) -> Vec<u32> {
        if !self.has_unsent.load(Ordering::Acquire) {
            return Vec::new();
        }
        let mut unsent = self.unsent.lock();
        self.has_unsent.store(false, Ordering::Release);
        std::mem::take(&mut *unsent)
    }

    /// Stops remembering `ids` as unsent (the host has just been given their current values).
    fn forget_unsent(&self, ids: impl Iterator<Item = u32>) {
        if !self.has_unsent.load(Ordering::Acquire) {
            return;
        }
        let mut unsent = self.unsent.lock();
        for id in ids {
            if let Ok(at) = unsent.binary_search(&id) {
                unsent.remove(at);
            }
        }
        self.has_unsent.store(!unsent.is_empty(), Ordering::Release);
    }
}

/// Armed while a claimed change-set is being built and delivered; if it is dropped still armed
/// (something panicked), the change-set is abandoned: every claimed slot is marked unsent and
/// its keyed baseline is dropped, because the host may never have seen the change the baseline
/// already counts.
struct Abandon<'a> {
    cell: &'a StoreCell,
    claimed: &'a [(u32, Arc<Slot>)],
    armed: bool,
}

impl Drop for Abandon<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for (id, slot) in self.claimed {
            slot.kind.forget();
            self.cell.mark_unsent(*id);
        }
    }
}

/// Undoes a partly built `observe(on)` when it unwinds: every target it touched returns to its
/// previous observed state, its keyed baseline is dropped, and a target that was already
/// observed (and whose dirty bit `observe` cleared) is marked unsent, so the host still gets
/// its current value with the next commit.
struct ObserveRollback<'a> {
    cell: &'a StoreCell,
    touched: Vec<(u32, Arc<Slot>, bool)>,
    armed: bool,
}

impl Drop for ObserveRollback<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for (id, slot, was_observed) in &self.touched {
            slot.flags.observed.store(*was_observed, Ordering::SeqCst);
            slot.kind.forget();
            if *was_observed {
                self.cell.mark_unsent(*id);
            }
        }
    }
}

/// Where a store keeps its [`StoreCell`]: an empty slot that is filled the first time the cell
/// is needed.
///
/// `#[undra::store]` adds one hidden field of this type to the struct. Rust cannot add state to
/// a struct any other way, and the cell cannot be created by the constructor the user wrote
/// (the macro does not own it), so the slot creates the cell lazily and exactly once, and then
/// hands out the same `Arc<StoreCell>` for the store's whole life. `Default` gives the empty
/// slot, which is what lets the macro fill the field in struct literals it rewrites.
///
/// The slot is `Send + Sync`. Concurrent first calls agree on a single cell: the initialiser
/// runs once and the other callers wait for it.
///
/// Deliberately not `Clone`: a store's signals are attached to exactly one cell, so a second
/// slot over the same signals could never be attached.
///
/// # Example
///
/// ```
/// use undra_signals::{CellSlot, Signal, StoreCell};
///
/// let count = Signal::new(0_u32);
/// let slot = CellSlot::default();
/// assert!(slot.get().is_none());
///
/// // Generated code: create the cell and attach the signals on first use.
/// let cell = slot.get_or_init(|| {
///     let cell = StoreCell::new(0xC0DE);
///     cell.attach(&count, 0).expect("fresh signal");
///     cell
/// });
/// assert_eq!(cell.signal_count(), 1);
///
/// // Later calls return the same cell; the initialiser is not run again.
/// let again = slot.get_or_init(|| unreachable!("already initialised"));
/// assert!(std::sync::Arc::ptr_eq(cell, again));
/// ```
#[derive(Default)]
pub struct CellSlot {
    cell: OnceLock<Arc<StoreCell>>,
    /// Serialises initialisation (`OnceLock::get_or_try_init` is not stable), so that two threads
    /// never both attach the same signals, whichever of the `get_or_*init` calls they use.
    init: Mutex<()>,
}

impl CellSlot {
    /// An empty slot (same as `CellSlot::default()`).
    pub const fn new() -> CellSlot {
        CellSlot {
            cell: OnceLock::new(),
            init: Mutex::new(()),
        }
    }

    /// The cell, if it has been created.
    pub fn get(&self) -> Option<&Arc<StoreCell>> {
        self.cell.get()
    }

    /// The cell, creating it with `init` the first time.
    ///
    /// If several threads race, exactly one runs `init` and the rest wait for its result. This
    /// also holds against concurrent [`get_or_try_init`](CellSlot::get_or_try_init) calls, which
    /// share the same lock. `init` must not call back into the same slot (it would deadlock).
    pub fn get_or_init(&self, init: impl FnOnce() -> Arc<StoreCell>) -> &Arc<StoreCell> {
        if let Some(cell) = self.cell.get() {
            return cell;
        }
        let _guard = self.init.lock();
        self.cell.get_or_init(init)
    }

    /// Like [`get_or_init`](CellSlot::get_or_init) for an initialiser that can fail (attaching
    /// signals returns a [`SignalsError`]). On failure the slot stays empty and the error is
    /// returned to this caller. Signals the initialiser attached before it failed stay bound to
    /// the discarded cell, so a store whose attach failed is unusable: running the initialiser
    /// again reports [`SignalsError::AlreadyAttached`].
    ///
    /// # Errors
    ///
    /// Whatever `init` returns.
    ///
    /// ```
    /// use undra_signals::{CellSlot, Signal, SignalsError, StoreCell};
    ///
    /// let taken = Signal::new(1_u8);
    /// StoreCell::new(1).attach(&taken, 0).unwrap(); // already belongs to another store
    ///
    /// let slot = CellSlot::new();
    /// let result = slot.get_or_try_init(|| {
    ///     let cell = StoreCell::new(2);
    ///     cell.attach(&taken, 0)?;
    ///     Ok::<_, SignalsError>(cell)
    /// });
    /// assert_eq!(result.unwrap_err(), SignalsError::AlreadyAttached);
    /// assert!(slot.get().is_none());
    /// ```
    pub fn get_or_try_init<E>(
        &self,
        init: impl FnOnce() -> Result<Arc<StoreCell>, E>,
    ) -> Result<&Arc<StoreCell>, E> {
        if let Some(cell) = self.cell.get() {
            return Ok(cell);
        }
        let _guard = self.init.lock();
        if let Some(cell) = self.cell.get() {
            return Ok(cell);
        }
        let cell = init()?;
        Ok(self.cell.get_or_init(|| cell))
    }
}

impl fmt::Debug for CellSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.cell.get() {
            Some(cell) => f.debug_tuple("CellSlot").field(cell).finish(),
            None => f.write_str("CellSlot(empty)"),
        }
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

/// A keyed list slot: the signal, its key function, the list as the host last saw it and the
/// log of the recorded operations made since.
struct KeyedList<T: ListLike> {
    signal: Signal<T>,
    key: KeyFn<T>,
    /// The items the host has, in order; `None` until the first `resync`/`diff` and after
    /// `forget`. Replaying `log` on it gives the signal's list while the log is usable.
    baseline: Mutex<Option<Vec<T::Item>>>,
    /// Shared with the signal: what its recorded list operations append to (ADR-027).
    log: Arc<KeyedLog<T::Item>>,
}

impl<T: SignalValue + ListLike> KeyedList<T> {
    /// The keyed patch that turns the baseline into `items` by comparing them (O(list)), and
    /// the baseline brought up to date; `None` (the full value is to be sent, and the baseline
    /// was replaced) when no patch is possible or worthwhile.
    fn diff_against_baseline(
        &self,
        baseline: &mut Option<Vec<T::Item>>,
        items: &[T::Item],
        w: &mut Writer,
    ) -> PatchOrFull {
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
                    debug_assert_eq!(old_keys, new_keys, "keyed baseline drifted from the list");
                }
                PatchOrFull::Patch
            }
            None => {
                *baseline = Some(items.to_vec());
                PatchOrFull::Full
            }
        }
    }
}

impl<T: SignalValue + ListLike> KeyedState for KeyedList<T> {
    fn diff(&self, w: &mut Writer, retain: bool) -> PatchOrFull {
        if !retain {
            self.signal.with(|current| current.encode(w));
            return PatchOrFull::Full;
        }
        // Lock order: baseline, then the signal's value lock (read), then the log. `resync`
        // does the same, and no writer takes the baseline lock.
        let mut baseline = self.baseline.lock();
        let base_len = baseline.as_ref().map(Vec::len);
        // Taking the log and looking at the list happen under the value's read lock: no write
        // (which appends to the log under the write lock) can fall between them, so the ops
        // taken are exactly the ones the list seen includes.
        let taken = self
            .signal
            .read_locked(|current| self.log.take(current, current.items().len(), base_len));
        match taken {
            Taken::Full(current) => {
                current.encode(w);
                *baseline = Some(current.items().to_vec());
                PatchOrFull::Full
            }
            Taken::Diff(current) => {
                let outcome = self.diff_against_baseline(&mut baseline, current.items(), w);
                if matches!(outcome, PatchOrFull::Full) {
                    current.encode(w);
                }
                outcome
            }
            Taken::Recorded {
                ops,
                #[cfg(debug_assertions)]
                current,
            } => {
                // O(ops): the log is the patch, and the baseline follows by replaying it (the
                // ops move into it, so only the clone made when each op was recorded is paid).
                let patch = KeyedPatch { ops };
                patch.encode(w);
                let KeyedPatch { mut ops } = patch;
                if let Some(old) = baseline.as_mut() {
                    apply_ops(old, &mut ops);
                }
                self.log.recycle(ops);
                #[cfg(debug_assertions)]
                if let Some(old) = baseline.as_deref() {
                    debug_assert!(
                        same_encoding(old, current.items()),
                        "keyed baseline drifted from the list (recorded ops)"
                    );
                }
                PatchOrFull::Patch
            }
        }
    }

    fn resync(&self, w: &mut Writer) {
        let mut baseline = self.baseline.lock();
        // Arming the log and looking at the list are one step under the read lock, for the
        // reason `diff` gives: the baseline is the list as of this instant and the log starts
        // empty at the same instant.
        let current = self.signal.read_locked(|current| {
            self.log.arm(current.items().len());
            Arc::clone(current)
        });
        current.encode(w);
        *baseline = Some(current.items().to_vec());
    }

    fn forget(&self) {
        *self.baseline.lock() = None;
        self.log.disarm();
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

/// Whether `a` and `b` are the same list as the host would see it: equal length, each item
/// with the same encoding. Debug builds use it to check a baseline against the list.
#[cfg(debug_assertions)]
fn same_encoding<I: SignalValue>(a: &[I], b: &[I]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let (mut left, mut right) = (Writer::new(), Writer::new());
            x.encode(&mut left);
            y.encode(&mut right);
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
    fn cell_slot_creates_the_cell_once_and_shares_it() {
        let slot = CellSlot::default();
        assert!(slot.get().is_none());
        assert_eq!(format!("{slot:?}"), "CellSlot(empty)");
        let count = Signal::new(1_u8);
        let first = Arc::clone(slot.get_or_init(|| {
            let cell = StoreCell::new(5);
            cell.attach(&count, 0).unwrap();
            cell
        }));
        let second = slot.get_or_init(|| panic!("the initialiser must not run twice"));
        assert!(Arc::ptr_eq(&first, second));
        assert!(Arc::ptr_eq(&first, slot.get().unwrap()));
        assert_eq!(first.signal_count(), 1);
        let text = format!("{slot:?}");
        assert!(text.starts_with("CellSlot(StoreCell"), "{text}");
    }

    #[test]
    fn cell_slot_new_is_const_and_empty() {
        static SLOT: CellSlot = CellSlot::new();
        assert!(SLOT.get().is_none());
    }

    #[test]
    fn cell_slot_try_init_reports_errors_and_stays_empty() {
        let shared = Signal::new(1_u8);
        StoreCell::new(1).attach(&shared, 0).unwrap();
        let slot = CellSlot::new();
        let err = slot
            .get_or_try_init(|| {
                let cell = StoreCell::new(2);
                cell.attach(&shared, 0)?;
                Ok::<_, SignalsError>(cell)
            })
            .unwrap_err();
        assert_eq!(err, SignalsError::AlreadyAttached);
        assert!(slot.get().is_none());
        // A successful initialiser fills the slot, and later calls do not run theirs.
        let ok = slot
            .get_or_try_init(|| Ok::<_, SignalsError>(StoreCell::new(3)))
            .unwrap();
        assert_eq!(ok.type_id(), 3);
        let again = slot
            .get_or_try_init(|| Err::<Arc<StoreCell>, _>(SignalsError::AlreadyAttached))
            .unwrap();
        assert!(Arc::ptr_eq(ok, again));
    }

    #[test]
    fn cell_slot_racing_threads_agree_on_one_cell() {
        use std::sync::atomic::AtomicUsize;
        let slot = Arc::new(CellSlot::new());
        let runs = Arc::new(AtomicUsize::new(0));
        let cells: Vec<Arc<StoreCell>> = (0..8)
            .map(|_| {
                let (slot, runs) = (Arc::clone(&slot), Arc::clone(&runs));
                std::thread::spawn(move || {
                    Arc::clone(
                        slot.get_or_try_init(|| {
                            runs.fetch_add(1, Ordering::SeqCst);
                            Ok::<_, SignalsError>(StoreCell::new(9))
                        })
                        .unwrap(),
                    )
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(cells.iter().all(|c| Arc::ptr_eq(c, &cells[0])));
    }

    #[test]
    fn cell_slot_infallible_and_fallible_initialisers_do_not_race_each_other() {
        use std::sync::atomic::AtomicUsize;
        // Both kinds of caller attach the same signal: without a shared lock one of them would
        // fail with `AlreadyAttached` even though a valid cell gets installed.
        for _ in 0..50 {
            let slot = Arc::new(CellSlot::new());
            let signal = Signal::new(0_u8);
            let runs = Arc::new(AtomicUsize::new(0));
            let threads: Vec<_> = (0..8)
                .map(|i| {
                    let (slot, signal, runs) =
                        (Arc::clone(&slot), signal.clone(), Arc::clone(&runs));
                    std::thread::spawn(move || {
                        let build = || {
                            runs.fetch_add(1, Ordering::SeqCst);
                            let cell = StoreCell::new(4);
                            cell.attach(&signal, 0).map(|()| cell)
                        };
                        if i % 2 == 0 {
                            slot.get_or_try_init(build).map(Arc::clone)
                        } else {
                            Ok(Arc::clone(slot.get_or_init(|| {
                                build().expect("the first initialiser attaches")
                            })))
                        }
                    })
                })
                .collect();
            for thread in threads {
                assert!(thread.join().unwrap().is_ok());
            }
            assert_eq!(runs.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn cell_slot_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CellSlot>();
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

    /// The op log of a keyed list signal.
    fn log_of(signal: &Signal<Vec<u32>>) -> &KeyedLog<u32> {
        signal
            .inner
            .log
            .get()
            .and_then(|log| log.as_any().downcast_ref::<KeyedLog<u32>>())
            .expect("attach_keyed installs the op log")
    }

    fn u32_key(n: &u32) -> u64 {
        u64::from(*n)
    }

    #[test]
    fn the_log_records_only_while_the_slot_is_observed() {
        let cell = StoreCell::new(1);
        let list = Signal::new(vec![1_u32, 2, 3]);
        cell.attach_keyed(&list, 0, u32_key).unwrap();
        cell.set_handle(7);
        let log = log_of(&list);
        assert!(!log.is_recording(), "attached but not observed");
        cell.observe(0, true, &mut Writer::new());
        assert!(
            log.is_recording(),
            "observing baselines the list and arms the log"
        );
        cell.observe(0, false, &mut Writer::new());
        assert!(
            !log.is_recording(),
            "unobserving forgets the baseline and the log"
        );
        list.push(4);
        assert!(!log.is_recording());
    }

    #[test]
    fn an_abandoned_commit_disarms_the_log_with_the_baseline() {
        use std::sync::atomic::AtomicBool;
        /// A value whose encoding panics while armed: a plain slot holding one abandons the
        /// store's change-set (a panicking computed no longer does, ADR-019 amendment).
        #[derive(Clone)]
        struct Bomb(Arc<AtomicBool>);
        impl undra_wire::Encode for Bomb {
            fn encode(&self, w: &mut Writer) {
                assert!(!self.0.load(Ordering::SeqCst), "encoder failure");
                0_u32.encode(w);
            }
        }
        let cell = StoreCell::new(1);
        let list = Signal::new(vec![1_u32, 2, 3]);
        let armed = Arc::new(AtomicBool::new(false));
        let bomb = Signal::new(Bomb(Arc::clone(&armed)));
        cell.attach_keyed(&list, 0, u32_key).unwrap();
        cell.attach(&bomb, 1).unwrap();
        cell.set_handle(7);
        cell.observe(ALL_SIGNALS, true, &mut Writer::new());
        let log = log_of(&list);
        assert!(log.is_recording());

        armed.store(true, Ordering::SeqCst);
        let sink = CaptureSink::new();
        let aborted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_sink(sink.clone(), || {
                txn(|| {
                    list.push(4);
                    bomb.update(|_| {});
                });
            });
        }));
        assert!(aborted.is_err());
        assert!(
            !log.is_recording(),
            "the ops of the abandoned change-set are dropped"
        );
        armed.store(false, Ordering::SeqCst);

        with_sink(sink.clone(), || list.push(5));
        assert!(
            log.is_recording(),
            "the full value that follows re-baselines and re-arms"
        );
    }

    #[test]
    fn a_failed_attach_installs_no_log() {
        let first = StoreCell::new(1);
        let second = StoreCell::new(2);
        let list = Signal::new(vec![1_u32]);
        first.attach_keyed(&list, 0, u32_key).unwrap();
        let other = Signal::new(vec![1_u32]);
        assert!(second.attach_keyed(&other, 5, u32_key).is_err());
        assert!(other.inner.log.get().is_none());
        assert!(second.attach_keyed(&list, 0, u32_key).is_err());
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
