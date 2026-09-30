#![forbid(unsafe_code)]
//! PROVISIONAL; superseded by the keel-signals branch.
//!
//! This file is the smallest surface of the SPEC section 16.1 contract that `keel-runtime`
//! needs in order to compile and be tested while the real `keel-signals` crate is written in
//! parallel: `Signal`, `StoreCell`, `txn`, the `ChangeSink` and `next_txn_id`. It has no
//! `Computed`, `Effect` or keyed-list patches. Every signature matches SPEC 16.1; the merge
//! replaces this whole crate with the real one.

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use keel_wire::payload::{ChangeOp, ChangeSetBuilder};
use keel_wire::{Encode, Handle, Writer};
use parking_lot::{Mutex, RwLock};

/// The `signal_id` that addresses every signal of a store at once.
pub const ALL_SIGNALS: u32 = u32::MAX;

/// Values a signal can hold: encodable, cloneable and thread safe.
pub trait SignalValue: Encode + Clone + Send + Sync + 'static {}
impl<T: Encode + Clone + Send + Sync + 'static> SignalValue for T {}

/// Extracts the key of a list item (provisional: the real alias is over `ListLike::Item`).
pub type KeyFn<T> = fn(&T) -> u64;

/// Receives every committed change-set (SPEC 3.5), encoded.
pub trait ChangeSink: Send + Sync {
    /// Called once per change-set, in commit order.
    fn deliver(&self, change_set: &[u8]);
}

static SINK: RwLock<Option<Arc<dyn ChangeSink>>> = RwLock::new(None);
static TXN_ID: AtomicU64 = AtomicU64::new(1);
static COMMIT: Mutex<()> = Mutex::new(());

/// Installs the process-wide change sink, replacing any previous one.
pub fn set_sink(sink: Arc<dyn ChangeSink>) {
    *SINK.write() = Some(sink);
}

/// Returns the next monotonic transaction id.
pub fn next_txn_id() -> u64 {
    TXN_ID.fetch_add(1, Ordering::Relaxed)
}

struct SignalInner<T> {
    value: RwLock<T>,
    binding: OnceLock<(Weak<StoreCell>, usize)>,
}

/// A reactive value. Cloning a signal yields another handle to the same value.
pub struct Signal<T> {
    inner: Arc<SignalInner<T>>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Signal {
            inner: self.inner.clone(),
        }
    }
}

impl<T: SignalValue> Signal<T> {
    /// Creates a signal holding `value`.
    pub fn new(value: T) -> Signal<T> {
        Signal {
            inner: Arc::new(SignalInner {
                value: RwLock::new(value),
                binding: OnceLock::new(),
            }),
        }
    }

    /// Returns a clone of the current value.
    pub fn get(&self) -> T {
        self.inner.value.read().clone()
    }

    /// Runs `f` on the current value.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.inner.value.read())
    }

    /// Replaces the value; an implicit transaction if none is open.
    pub fn set(&self, value: T) {
        *self.inner.value.write() = value;
        self.mark_dirty();
    }

    /// Mutates the value in place; an implicit transaction if none is open.
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        f(&mut self.inner.value.write());
        self.mark_dirty();
    }

    fn mark_dirty(&self) {
        let Some((cell, slot)) = self.inner.binding.get() else {
            return;
        };
        let Some(cell) = cell.upgrade() else { return };
        let open = TXN.with(|t| {
            let mut t = t.borrow_mut();
            if t.depth > 0 {
                t.dirty.push((cell.clone(), *slot));
                true
            } else {
                false
            }
        });
        if !open {
            commit(vec![(cell, *slot)]);
        }
    }
}

struct Slot {
    signal_id: u32,
    observed: bool,
    pending_dirty: bool,
    encode: Box<dyn Fn(&mut Writer) + Send + Sync>,
}

/// One per store instance: binds signals, tracks observation and builds change-sets.
pub struct StoreCell {
    type_id: u32,
    handle: AtomicU64,
    slots: Mutex<Vec<Slot>>,
}

impl StoreCell {
    /// Creates the cell of a store of type `type_id`.
    pub fn new(type_id: u32) -> Arc<StoreCell> {
        Arc::new(StoreCell {
            type_id,
            handle: AtomicU64::new(0),
            slots: Mutex::new(Vec::new()),
        })
    }

    /// Binds a signal field; called once per field in declaration order.
    pub fn attach<T: SignalValue>(
        self: &Arc<Self>,
        signal: &Signal<T>,
        signal_id: u32,
        key: Option<KeyFn<T>>,
    ) {
        let _ = key;
        let value = signal.clone();
        let index = {
            let mut slots = self.slots.lock();
            slots.push(Slot {
                signal_id,
                observed: false,
                pending_dirty: false,
                encode: Box::new(move |w| value.with(|v| v.encode(w))),
            });
            slots.len() - 1
        };
        let _ = signal.inner.binding.set((Arc::downgrade(self), index));
    }

    /// Called by the runtime when the store enters the object table (`0` detaches it).
    pub fn set_handle(&self, handle: u64) {
        self.handle.store(handle, Ordering::Release);
    }

    /// The handle set by the runtime, or `0`.
    pub fn handle(&self) -> u64 {
        self.handle.load(Ordering::Acquire)
    }

    /// The store's type id.
    pub fn type_id(&self) -> u32 {
        self.type_id
    }

    /// Number of attached signals.
    pub fn signal_count(&self) -> u32 {
        u32::try_from(self.slots.lock().len()).unwrap_or(u32::MAX)
    }

    /// Starts or stops observing; on `true` appends one change-set entry per addressed signal
    /// with its current value and returns the number of entries.
    pub fn observe(&self, signal_id: u32, on: bool, out: &mut Writer) -> u32 {
        let handle = Handle(self.handle());
        let mut slots = self.slots.lock();
        let mut count = 0;
        for slot in slots.iter_mut() {
            if signal_id != ALL_SIGNALS && slot.signal_id != signal_id {
                continue;
            }
            slot.observed = on;
            if on {
                slot.pending_dirty = false;
                if !handle.is_null() {
                    push_entry(out, handle, slot);
                    count += 1;
                }
            }
        }
        count
    }

    /// Appends the full encoded value of one signal; `false` if there is no such signal.
    pub fn encode_signal(&self, signal_id: u32, out: &mut Writer) -> bool {
        let slots = self.slots.lock();
        match slots.iter().find(|s| s.signal_id == signal_id) {
            Some(slot) => {
                (slot.encode)(out);
                true
            }
            None => false,
        }
    }

    /// Appends the snapshot body: `signal_count u32, signals x { signal_id u32, len u32, value }`.
    pub fn encode_snapshot(&self, out: &mut Writer) {
        let slots = self.slots.lock();
        out.write_u32(u32::try_from(slots.len()).unwrap_or(u32::MAX));
        for slot in slots.iter() {
            let mut value = Writer::new();
            (slot.encode)(&mut value);
            out.write_u32(slot.signal_id);
            out.write_bytes(value.as_slice());
        }
    }

    fn change_set(&self, dirty_slots: &[usize]) -> Option<Vec<u8>> {
        let handle = Handle(self.handle());
        let mut slots = self.slots.lock();
        let mut entries: Vec<(u32, Vec<u8>)> = Vec::new();
        for &index in dirty_slots {
            let Some(slot) = slots.get_mut(index) else {
                continue;
            };
            if slot.observed && !handle.is_null() {
                let mut value = Writer::new();
                (slot.encode)(&mut value);
                entries.push((slot.signal_id, value.into_vec()));
            } else {
                slot.pending_dirty = true;
            }
        }
        drop(slots);
        if entries.is_empty() {
            return None;
        }
        let mut w = Writer::new();
        let mut builder = ChangeSetBuilder::new(&mut w, next_txn_id());
        for (signal_id, value) in &entries {
            builder.push(handle, *signal_id, ChangeOp::Full, value);
        }
        builder.finish();
        Some(w.into_vec())
    }
}

fn push_entry(out: &mut Writer, handle: Handle, slot: &Slot) {
    let mut value = Writer::new();
    (slot.encode)(&mut value);
    out.write_u64(handle.0);
    out.write_u32(slot.signal_id);
    out.write_u8(ChangeOp::Full.as_u8());
    out.write_bytes(value.as_slice());
}

#[derive(Default)]
struct TxnState {
    depth: usize,
    dirty: Vec<(Arc<StoreCell>, usize)>,
}

thread_local! {
    static TXN: RefCell<TxnState> = RefCell::new(TxnState::default());
}

fn commit(dirty: Vec<(Arc<StoreCell>, usize)>) {
    let _order = COMMIT.lock();
    let mut groups: Vec<(Arc<StoreCell>, Vec<usize>)> = Vec::new();
    for (cell, slot) in dirty {
        match groups.iter_mut().find(|(c, _)| Arc::ptr_eq(c, &cell)) {
            Some((_, slots)) => {
                if !slots.contains(&slot) {
                    slots.push(slot);
                }
            }
            None => groups.push((cell, vec![slot])),
        }
    }
    let sink = SINK.read().clone();
    for (cell, slots) in groups {
        if let (Some(bytes), Some(sink)) = (cell.change_set(&slots), sink.as_ref()) {
            sink.deliver(&bytes);
        }
    }
}

struct TxnExit;

impl Drop for TxnExit {
    fn drop(&mut self) {
        let dirty = TXN.with(|t| {
            let mut t = t.borrow_mut();
            t.depth -= 1;
            if t.depth == 0 {
                std::mem::take(&mut t.dirty)
            } else {
                Vec::new()
            }
        });
        if dirty.is_empty() {
            return;
        }
        // Writes already happened, so observers must still hear about them, even while a
        // panic unwinds through this scope.
        let result = panic::catch_unwind(AssertUnwindSafe(|| commit(dirty)));
        if let Err(payload) = result {
            if !std::thread::panicking() {
                panic::resume_unwind(payload);
            }
        }
    }
}

/// Batches every write made inside `f` into one transaction; nested calls join the outer one.
pub fn txn<R>(f: impl FnOnce() -> R) -> R {
    TXN.with(|t| t.borrow_mut().depth += 1);
    let _exit = TxnExit;
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Capture(Mutex<Vec<Vec<u8>>>);
    impl ChangeSink for Capture {
        fn deliver(&self, change_set: &[u8]) {
            self.0.lock().push(change_set.to_vec());
        }
    }

    #[test]
    fn observed_write_commits_one_change_set_per_txn() {
        let capture = Arc::new(Capture(Mutex::new(Vec::new())));
        set_sink(capture.clone());
        let cell = StoreCell::new(7);
        let a = Signal::new(1_i32);
        let b = Signal::new(2_i32);
        cell.attach(&a, 0, None);
        cell.attach(&b, 1, None);
        cell.set_handle(Handle::new(0, 1).0);

        let mut w = Writer::new();
        assert_eq!(cell.observe(ALL_SIGNALS, true, &mut w), 2);

        txn(|| {
            a.set(10);
            b.set(20);
        });
        let sets = capture.0.lock();
        let ours: Vec<_> = sets
            .iter()
            .filter(|s| {
                keel_wire::payload::ChangeSetRef::decode(&mut keel_wire::Reader::new(s))
                    .map(|c| c.iter().any(|e| e.handle == Handle::new(0, 1)))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(ours.len(), 1);
    }

    #[test]
    fn unbound_and_unobserved_writes_do_not_deliver() {
        let cell = StoreCell::new(8);
        let a = Signal::new(0_u8);
        a.set(1); // not attached
        cell.attach(&a, 0, None);
        cell.set_handle(Handle::new(9, 1).0);
        a.set(2); // attached, unobserved
        assert_eq!(a.get(), 2);
        let mut body = Writer::new();
        cell.encode_snapshot(&mut body);
        assert_eq!(body.as_slice(), &[1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2]);
    }
}
