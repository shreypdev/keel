//! Shared fixtures for the integration tests.
#![allow(dead_code)] // each test binary uses a different subset

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use undra_signals::testing::CaptureSink;
use undra_signals::{ALL_SIGNALS, StoreCell, with_sink};
use undra_wire::payload::{ChangeEntry, ChangeOp, ChangeSet};
use undra_wire::{Decode, Encode, Handle, KeyedPatch, Reader, WireError, Writer};

/// A list item with a key (`id`) and payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    pub id: u32,
    pub title: String,
    pub done: bool,
}

pub fn todo(id: u32, title: &str, done: bool) -> Todo {
    Todo {
        id,
        title: title.to_string(),
        done,
    }
}

/// `id`s 1..=n with titles "t<id>", none done.
pub fn todos(n: u32) -> Vec<Todo> {
    (1..=n)
        .map(|id| todo(id, &format!("t{id}"), false))
        .collect()
}

impl Encode for Todo {
    fn encode(&self, w: &mut Writer) {
        self.id.encode(w);
        self.title.encode(w);
        self.done.encode(w);
    }
}

impl Decode for Todo {
    const MIN_ENCODED_LEN: usize = 9;
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Todo {
            id: u32::decode(r)?,
            title: String::decode(r)?,
            done: bool::decode(r)?,
        })
    }
}

pub fn todo_key(t: &Todo) -> u64 {
    u64::from(t.id)
}

/// The handle every fixture store gets.
pub const HANDLE: u64 = 0x0000_0001_0000_0007;

pub fn handle() -> Handle {
    Handle(HANDLE)
}

/// A store cell with a handle, plus a sink that captures what its commits deliver.
pub struct Rig {
    pub cell: Arc<StoreCell>,
    pub sink: Arc<CaptureSink>,
}

impl Rig {
    pub fn new() -> Rig {
        let cell = StoreCell::new(0xF00D);
        cell.set_handle(HANDLE);
        Rig {
            cell,
            sink: CaptureSink::new(),
        }
    }

    /// Runs `f` with this rig's sink installed for the calling thread.
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        with_sink(self.sink.clone(), f)
    }

    /// The change-sets delivered so far (drains them).
    pub fn sets(&self) -> Vec<ChangeSet> {
        self.sink.take_decoded()
    }

    /// Exactly one change-set was delivered; returns it.
    pub fn one_set(&self) -> ChangeSet {
        let mut sets = self.sets();
        assert_eq!(
            sets.len(),
            1,
            "expected exactly one change-set, got {sets:?}"
        );
        sets.remove(0)
    }

    pub fn observe_all(&self) -> Vec<ChangeEntry> {
        observe(&self.cell, ALL_SIGNALS, true)
    }

    pub fn observe_on(&self, id: u32) -> Vec<ChangeEntry> {
        observe(&self.cell, id, true)
    }

    pub fn observe_off(&self, id: u32) {
        let mut out = Writer::new();
        assert_eq!(self.cell.observe(id, false, &mut out), 0);
        assert!(out.is_empty());
    }
}

/// Calls `cell.observe(..)` and decodes the entries it appended.
pub fn observe(cell: &StoreCell, id: u32, on: bool) -> Vec<ChangeEntry> {
    let mut entries = Writer::new();
    let count = cell.observe(id, on, &mut entries);
    let mut payload = Writer::new();
    payload.write_u64(undra_signals::next_txn_id());
    payload.write_u32(count);
    payload.write_raw(entries.as_slice());
    let mut r = Reader::new(payload.as_slice());
    let set = ChangeSet::decode(&mut r).expect("observe must append valid entries");
    r.finish().expect("no trailing bytes");
    assert_eq!(set.entries.len() as u32, count);
    set.entries
}

/// Decodes an entry's value as a `T`.
pub fn value_of<T: Decode>(entry: &ChangeEntry) -> T {
    T::decode_exact(&entry.value).expect("entry value decodes")
}

/// Decodes an entry's value as a keyed patch of `T`.
pub fn patch_of<T: Decode>(entry: &ChangeEntry) -> KeyedPatch<T> {
    assert_eq!(entry.op, ChangeOp::KeyedPatch, "entry is not a patch");
    let mut r = Reader::new(&entry.value);
    let patch = KeyedPatch::<T>::decode(&mut r).expect("patch decodes");
    r.finish().expect("no trailing bytes");
    patch
}

/// The entry for signal `id` in `set`, which must be present exactly once.
pub fn entry(set: &ChangeSet, id: u32) -> &ChangeEntry {
    let mut found = set.entries.iter().filter(|e| e.signal_id == id);
    let first = found
        .next()
        .unwrap_or_else(|| panic!("no entry for signal {id} in {set:?}"));
    assert!(found.next().is_none(), "duplicate entry for signal {id}");
    first
}

pub fn ids(set: &ChangeSet) -> Vec<u32> {
    set.entries.iter().map(|e| e.signal_id).collect()
}

/// A value whose encodings are counted, to observe when and how often the crate encodes.
#[derive(Clone, Debug)]
pub struct Probe {
    pub value: u32,
    pub encodes: Arc<AtomicUsize>,
}

impl Probe {
    pub fn new(value: u32, encodes: &Arc<AtomicUsize>) -> Probe {
        Probe {
            value,
            encodes: Arc::clone(encodes),
        }
    }
}

impl Encode for Probe {
    fn encode(&self, w: &mut Writer) {
        self.encodes.fetch_add(1, Ordering::SeqCst);
        self.value.encode(w);
    }
}

pub fn count(c: &Arc<AtomicUsize>) -> usize {
    c.load(Ordering::SeqCst)
}

/// A value whose **encoding** panics while `armed`: a plain slot holding one makes its store's
/// commit (or observe) abandon the whole change-set, the ADR-019 (H1) path. (A computed that
/// panics no longer does: since the ADR-019 amendment it is held back on its own.)
#[derive(Clone, Debug)]
pub struct EncodeBomb {
    pub armed: Arc<std::sync::atomic::AtomicBool>,
    pub value: u32,
}

impl EncodeBomb {
    pub fn new(armed: &Arc<std::sync::atomic::AtomicBool>, value: u32) -> EncodeBomb {
        EncodeBomb {
            armed: Arc::clone(armed),
            value,
        }
    }
}

impl Encode for EncodeBomb {
    fn encode(&self, w: &mut Writer) {
        assert!(!self.armed.load(Ordering::SeqCst), "encoder failure");
        self.value.encode(w);
    }
}
