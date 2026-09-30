//! A naive stand-in for keel-signals: just enough surface for the code `#[keel::store]`
//! generates (SPEC 16.1), plus recording so tests can see what the generated code did.

use core::any::Any;
use core::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use keel_wire::{Encode, Writer};

/// `Encode + Clone + Send + Sync + 'static`.
pub trait SignalValue: Encode + Clone + Send + Sync + 'static {}
impl<T: Encode + Clone + Send + Sync + 'static> SignalValue for T {}

/// The key function of a keyed list: hashes the key of one item to a `u64`.
///
/// Type-erased over the item (the generated function downcasts): a generic
/// `fn(&Item) -> u64` cannot be expressed for both list and non-list signals in one method.
pub type KeyFn = fn(&dyn Any) -> u64;

/// A mutable value; clones share the value.
pub struct Signal<T>(Arc<Mutex<T>>);

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Signal(Arc::clone(&self.0))
    }
}

impl<T: SignalValue> Signal<T> {
    /// A new signal.
    pub fn new(value: T) -> Signal<T> {
        Signal(Arc::new(Mutex::new(value)))
    }
    /// A clone of the current value.
    pub fn get(&self) -> T {
        self.0.lock().unwrap().clone()
    }
    /// Replaces the value.
    pub fn set(&self, value: T) {
        *self.0.lock().unwrap() = value;
    }
    /// Mutates the value.
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        f(&mut self.0.lock().unwrap());
    }
}

/// What a computed value reads from.
pub trait Deps {
    /// Owned handles to the dependencies.
    type Owned: Send + Sync + 'static;
    /// The values handed to the closure.
    type Values;
    /// Takes owned handles.
    fn capture(&self) -> Self::Owned;
    /// Reads the current values.
    fn read(owned: &Self::Owned) -> Self::Values;
}

impl<A: SignalValue> Deps for (&Signal<A>,) {
    type Owned = (Signal<A>,);
    type Values = (A,);
    fn capture(&self) -> Self::Owned {
        (self.0.clone(),)
    }
    fn read(owned: &Self::Owned) -> Self::Values {
        (owned.0.get(),)
    }
}

impl<A: SignalValue, B: SignalValue> Deps for (&Signal<A>, &Signal<B>) {
    type Owned = (Signal<A>, Signal<B>);
    type Values = (A, B);
    fn capture(&self) -> Self::Owned {
        (self.0.clone(), self.1.clone())
    }
    fn read(owned: &Self::Owned) -> Self::Values {
        (owned.0.get(), owned.1.get())
    }
}

/// A value derived from other signals, recomputed on every read.
pub struct Computed<T>(Arc<dyn Fn() -> T + Send + Sync>);

impl<T: SignalValue> Computed<T> {
    /// A computed value over `deps`.
    pub fn new<D: Deps>(
        deps: D,
        f: impl Fn(D::Values) -> T + Send + Sync + 'static,
    ) -> Computed<T> {
        let owned = deps.capture();
        Computed(Arc::new(move || f(D::read(&owned))))
    }
    /// The current value.
    pub fn get(&self) -> T {
        (self.0)()
    }
}

/// A lazily paged list (a handle in the real crate).
pub struct Lazy<T>(PhantomData<fn() -> T>);

impl<T> Lazy<T> {
    /// A new lazy list.
    pub fn new() -> Lazy<T> {
        Lazy(PhantomData)
    }
}

impl<T> Default for Lazy<T> {
    fn default() -> Self {
        Lazy::new()
    }
}

/// How a signal was attached to a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachKind {
    /// `Signal<T>`.
    Plain,
    /// `Computed<T>`.
    Computed,
    /// `Lazy<T>`.
    Lazy,
}

/// One attached signal, as recorded by [`StoreCell`].
#[derive(Clone, Debug)]
pub struct Attached {
    /// The signal id.
    pub signal_id: u32,
    /// Plain, computed or lazy.
    pub kind: AttachKind,
    /// The key function of a keyed list.
    pub key: Option<KeyFn>,
    /// Whether the signal is excluded from coalescing.
    pub no_coalesce: bool,
}

type Encoder = Box<dyn Fn(&mut Writer) + Send + Sync>;

struct Entry {
    info: Attached,
    encoder: Option<Encoder>,
}

/// The per-instance cell every signal of a store attaches to.
pub struct StoreCell {
    type_id: u32,
    handle: AtomicU64,
    entries: Mutex<Vec<Entry>>,
}

impl StoreCell {
    /// A new, empty cell.
    pub fn new(type_id: u32) -> Arc<StoreCell> {
        Arc::new(StoreCell {
            type_id,
            handle: AtomicU64::new(0),
            entries: Mutex::new(Vec::new()),
        })
    }

    /// Binds a signal field.
    pub fn attach<T: SignalValue>(
        self: &Arc<Self>,
        signal: &Signal<T>,
        signal_id: u32,
        key: Option<KeyFn>,
    ) {
        let signal = signal.clone();
        self.entries.lock().unwrap().push(Entry {
            info: Attached {
                signal_id,
                kind: AttachKind::Plain,
                key,
                no_coalesce: false,
            },
            encoder: Some(Box::new(move |w| signal.get().encode(w))),
        });
    }

    /// Binds a computed field.
    pub fn attach_computed<T: SignalValue>(
        self: &Arc<Self>,
        _computed: &Computed<T>,
        signal_id: u32,
    ) {
        self.entries.lock().unwrap().push(Entry {
            info: Attached {
                signal_id,
                kind: AttachKind::Computed,
                key: None,
                no_coalesce: false,
            },
            encoder: None,
        });
    }

    /// Binds a lazy field.
    pub fn attach_lazy<T: 'static>(self: &Arc<Self>, _lazy: &Lazy<T>, signal_id: u32) {
        self.entries.lock().unwrap().push(Entry {
            info: Attached {
                signal_id,
                kind: AttachKind::Lazy,
                key: None,
                no_coalesce: false,
            },
            encoder: None,
        });
    }

    /// Marks a signal so every commit is delivered.
    pub fn set_no_coalesce(&self, signal_id: u32) {
        for entry in self.entries.lock().unwrap().iter_mut() {
            if entry.info.signal_id == signal_id {
                entry.info.no_coalesce = true;
            }
        }
    }

    /// Records the handle the runtime issued.
    pub fn set_handle(&self, handle: u64) {
        self.handle.store(handle, Ordering::SeqCst);
    }

    /// The recorded handle (`0` before insertion).
    pub fn handle(&self) -> u64 {
        self.handle.load(Ordering::SeqCst)
    }

    /// The type id the cell was created with.
    pub fn type_id(&self) -> u32 {
        self.type_id
    }

    /// What was attached, in attachment order.
    pub fn attached(&self) -> Vec<Attached> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.info.clone())
            .collect()
    }

    /// The store body of a snapshot (SPEC 5.9): the non-computed signals.
    pub fn encode_snapshot(&self, out: &mut Writer) {
        let entries = self.entries.lock().unwrap();
        let plain: Vec<&Entry> = entries.iter().filter(|e| e.encoder.is_some()).collect();
        out.write_u32(u32::try_from(plain.len()).unwrap());
        for entry in plain {
            out.write_u32(entry.info.signal_id);
            let mut value = Writer::new();
            (entry.encoder.as_ref().unwrap())(&mut value);
            out.write_bytes(value.as_slice());
        }
    }
}

/// Where a store keeps its cell: created and attached on first use.
#[derive(Debug, Default)]
pub struct CellSlot(OnceLock<Arc<StoreCell>>);

impl CellSlot {
    /// The cell, creating it with `init` the first time.
    pub fn get_or_init(&self, init: impl FnOnce() -> Arc<StoreCell>) -> &Arc<StoreCell> {
        self.0.get_or_init(init)
    }
}

impl core::fmt::Debug for StoreCell {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "StoreCell({})", self.type_id)
    }
}
