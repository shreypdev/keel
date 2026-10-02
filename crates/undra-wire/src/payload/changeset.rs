//! The change-set payload (SPEC 3.5).
//!
//! Three ways to work with one:
//!
//! * [`ChangeSet`]: owned entries. Convenient in tests and hosts.
//! * [`ChangeSetRef`]: validates the message once and then iterates borrowed entries without
//!   allocating.
//! * [`ChangeSetBuilder`]: streams entries straight into a [`Writer`] without building any
//!   intermediate `Vec`, for the runtime's commit path.

use crate::macros::wire_u8_enum;
use crate::writer::len_u32;
use crate::{Handle, Reader, WireError, Writer};

/// Smallest possible encoded entry: `handle u64, signal_id u32, op u8, len u32` and no value.
const ENTRY_MIN_LEN: usize = 17;

wire_u8_enum! {
    /// How an entry's `value` is to be interpreted (SPEC 3.5).
    pub enum ChangeOp {
        /// `value` is the signal's `T`, encoded.
        Full = 0,
        /// `value` is a keyed patch (SPEC 3.8) for a `Signal<Vec<T>>`.
        KeyedPatch = 1,
        /// A lazy list changed (ADR-043); `value` is a [`LazyInvalidated`](super::LazyInvalidated)
        /// (`len u32, version u64`) and the host re-pages the window it shows.
        LazyInvalidated = 2,
    }
}

/// One signal update inside a change-set, borrowing its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangeEntryRef<'a> {
    /// The store the signal belongs to.
    pub handle: Handle,
    /// The signal within the store.
    pub signal_id: u32,
    /// How to interpret `value`.
    pub op: ChangeOp,
    /// The encoded value. Its length prefix on the wire lets a host skip entries it cannot
    /// decode.
    pub value: &'a [u8],
}

/// One signal update inside a change-set, owning its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeEntry {
    /// The store the signal belongs to.
    pub handle: Handle,
    /// The signal within the store.
    pub signal_id: u32,
    /// How to interpret `value`.
    pub op: ChangeOp,
    /// The encoded value.
    pub value: Vec<u8>,
}

impl ChangeEntry {
    /// Appends the entry to `w`: `handle u64, signal_id u32, op u8, len u32, value`.
    pub fn encode(&self, w: &mut Writer) {
        w.reserve(ENTRY_MIN_LEN + self.value.len());
        w.write_u64(self.handle.0);
        w.write_u32(self.signal_id);
        w.write_u8(self.op.as_u8());
        w.write_bytes(&self.value);
    }
}

impl From<&ChangeEntryRef<'_>> for ChangeEntry {
    fn from(e: &ChangeEntryRef<'_>) -> Self {
        ChangeEntry {
            handle: e.handle,
            signal_id: e.signal_id,
            op: e.op,
            value: e.value.to_vec(),
        }
    }
}

fn read_entry<'a>(r: &mut Reader<'a>) -> Result<ChangeEntryRef<'a>, WireError> {
    Ok(ChangeEntryRef {
        handle: Handle(r.read_u64()?),
        signal_id: r.read_u32()?,
        op: ChangeOp::read(r)?,
        value: r.read_bytes()?,
    })
}

/// The signal updates of one transaction (kind `ChangeSet`), fully owned.
///
/// Layout: `txn_id u64, count u32, count x { handle u64, signal_id u32, op u8, len u32, value }`.
///
/// # Example
///
/// ```
/// use undra_wire::payload::{ChangeEntry, ChangeOp, ChangeSet};
/// use undra_wire::{Handle, Reader, Writer};
///
/// let cs = ChangeSet {
///     txn_id: 42,
///     entries: vec![ChangeEntry {
///         handle: Handle::new(1, 1),
///         signal_id: 0,
///         op: ChangeOp::Full,
///         value: vec![2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0],
///     }],
/// };
/// let mut w = Writer::new();
/// cs.encode(&mut w);
/// assert_eq!(ChangeSet::decode(&mut Reader::new(w.as_slice())), Ok(cs));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSet {
    /// Monotonic transaction id.
    pub txn_id: u64,
    /// The updates, in the order the runtime produced them.
    pub entries: Vec<ChangeEntry>,
}

impl ChangeSet {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u64(self.txn_id);
        w.write_len(len_u32(self.entries.len()));
        for entry in &self.entries {
            entry.encode(w);
        }
    }

    /// Reads a payload, copying every value out of the input.
    ///
    /// Use [`ChangeSetRef::decode`] to avoid the copies.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        ChangeSetRef::decode(r).map(|set| ChangeSet::from(&set))
    }
}

impl From<&ChangeSetRef<'_>> for ChangeSet {
    fn from(set: &ChangeSetRef<'_>) -> Self {
        let mut entries = Vec::with_capacity(set.len());
        entries.extend(set.iter().map(|e| ChangeEntry::from(&e)));
        ChangeSet {
            txn_id: set.txn_id,
            entries,
        }
    }
}

/// A validated, zero-copy view of a change-set payload.
///
/// [`decode`](ChangeSetRef::decode) walks the message once, checking every entry header, op
/// tag and length, and remembers where the entries start. [`iter`](ChangeSetRef::iter) then
/// yields [`ChangeEntryRef`]s that borrow their values from the input; neither step
/// allocates, and iteration cannot fail.
///
/// # Example
///
/// ```
/// use undra_wire::payload::{ChangeOp, ChangeSetBuilder, ChangeSetRef};
/// use undra_wire::{Handle, Reader, Writer};
///
/// let mut w = Writer::new();
/// let mut b = ChangeSetBuilder::new(&mut w, 7);
/// b.push(Handle::new(1, 1), 0, ChangeOp::Full, &[1, 2, 3]);
/// b.push(Handle::new(1, 1), 1, ChangeOp::LazyInvalidated, &[]);
/// b.finish();
///
/// let set = ChangeSetRef::decode(&mut Reader::new(w.as_slice())).unwrap();
/// assert_eq!(set.txn_id, 7);
/// let values: Vec<&[u8]> = set.iter().map(|e| e.value).collect();
/// assert_eq!(values, [&[1, 2, 3][..], &[][..]]);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangeSetRef<'a> {
    /// Monotonic transaction id.
    pub txn_id: u64,
    count: usize,
    entries: &'a [u8],
}

impl<'a> ChangeSetRef<'a> {
    /// Reads and validates a payload, borrowing the entries from the input.
    ///
    /// On success the reader is positioned after the last entry.
    pub fn decode(r: &mut Reader<'a>) -> Result<Self, WireError> {
        let txn_id = r.read_u64()?;
        let count = r.read_count(ENTRY_MIN_LEN)?;
        let start = r.position();
        for _ in 0..count {
            read_entry(r)?;
        }
        Ok(ChangeSetRef {
            txn_id,
            count,
            entries: r.consumed_since(start),
        })
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` if the change-set has no entries.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Iterates over the entries in wire order without allocating.
    pub fn iter(&self) -> ChangeEntries<'a> {
        ChangeEntries {
            reader: Reader::new(self.entries),
            left: self.count,
        }
    }
}

impl<'a> IntoIterator for &ChangeSetRef<'a> {
    type Item = ChangeEntryRef<'a>;
    type IntoIter = ChangeEntries<'a>;

    fn into_iter(self) -> ChangeEntries<'a> {
        self.iter()
    }
}

/// Iterator over the entries of a [`ChangeSetRef`].
#[derive(Clone, Debug)]
pub struct ChangeEntries<'a> {
    reader: Reader<'a>,
    left: usize,
}

impl<'a> Iterator for ChangeEntries<'a> {
    type Item = ChangeEntryRef<'a>;

    fn next(&mut self) -> Option<ChangeEntryRef<'a>> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        // The entries were validated by `ChangeSetRef::decode`, so this cannot fail; if it
        // somehow did, ending the iteration is the safe answer.
        match read_entry(&mut self.reader) {
            Ok(entry) => Some(entry),
            Err(_) => {
                self.left = 0;
                None
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl ExactSizeIterator for ChangeEntries<'_> {}

/// Writes a change-set straight into a [`Writer`], one entry at a time, with no intermediate
/// allocation.
///
/// The header (`txn_id` and a count) is written by [`new`](ChangeSetBuilder::new); the count
/// is kept up to date after every push, so the bytes in the writer are always a well-formed
/// change-set, even if the builder is dropped without calling
/// [`finish`](ChangeSetBuilder::finish).
///
/// # Example
///
/// ```
/// use undra_wire::payload::{ChangeOp, ChangeSetBuilder};
/// use undra_wire::{Encode, Handle, Writer};
///
/// let mut w = Writer::new();
/// let mut b = ChangeSetBuilder::new(&mut w, 1);
/// // Encode the value directly into the message; no temporary Vec.
/// b.push_with(Handle::new(1, 1), 0, ChangeOp::Full, |w| 5_i32.encode(w));
/// assert_eq!(b.finish(), 1);
/// ```
#[derive(Debug)]
pub struct ChangeSetBuilder<'w> {
    w: &'w mut Writer,
    count_at: usize,
    count: u32,
}

impl<'w> ChangeSetBuilder<'w> {
    /// Starts a change-set for transaction `txn_id`, appending its header to `w`.
    pub fn new(w: &'w mut Writer, txn_id: u64) -> Self {
        w.write_u64(txn_id);
        let count_at = w.len();
        w.write_u32(0);
        ChangeSetBuilder {
            w,
            count_at,
            count: 0,
        }
    }

    /// Appends an entry whose value is already encoded.
    pub fn push(&mut self, handle: Handle, signal_id: u32, op: ChangeOp, value: &[u8]) {
        self.w.reserve(ENTRY_MIN_LEN + value.len());
        self.w.write_u64(handle.0);
        self.w.write_u32(signal_id);
        self.w.write_u8(op.as_u8());
        self.w.write_bytes(value);
        self.bump();
    }

    /// Appends an entry whose value is written by `value` directly into the message; the
    /// length prefix is back-patched afterwards.
    pub fn push_with(
        &mut self,
        handle: Handle,
        signal_id: u32,
        op: ChangeOp,
        value: impl FnOnce(&mut Writer),
    ) {
        self.w.write_u64(handle.0);
        self.w.write_u32(signal_id);
        self.w.write_u8(op.as_u8());
        let len_at = self.w.len();
        self.w.write_u32(0);
        value(self.w);
        let value_len = self.w.len().saturating_sub(len_at + 4);
        self.w.patch_u32(len_at, len_u32(value_len));
        self.bump();
    }

    /// Number of entries pushed so far.
    pub fn len(&self) -> u32 {
        self.count
    }

    /// Returns `true` if no entry has been pushed.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Ends the change-set and returns its entry count.
    pub fn finish(self) -> u32 {
        self.count
    }

    fn bump(&mut self) {
        self.count = len_u32(self.count as usize + 1);
        self.w.patch_u32(self.count_at, self.count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChangeSet {
        ChangeSet {
            txn_id: 42,
            entries: vec![
                ChangeEntry {
                    handle: Handle::new(1, 1),
                    signal_id: 0,
                    op: ChangeOp::Full,
                    value: vec![2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0],
                },
                ChangeEntry {
                    handle: Handle::new(2, 1),
                    signal_id: 3,
                    op: ChangeOp::KeyedPatch,
                    value: vec![0, 0, 0, 0],
                },
                ChangeEntry {
                    handle: Handle::new(2, 1),
                    signal_id: 4,
                    op: ChangeOp::LazyInvalidated,
                    value: vec![],
                },
            ],
        }
    }

    fn encode(cs: &ChangeSet) -> Vec<u8> {
        let mut w = Writer::new();
        cs.encode(&mut w);
        w.into_vec()
    }

    #[test]
    fn layout_matches_the_vector() {
        let cs = ChangeSet {
            txn_id: 42,
            entries: vec![ChangeEntry {
                handle: Handle(4_294_967_297),
                signal_id: 0,
                op: ChangeOp::Full,
                value: vec![2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0],
            }],
        };
        assert_eq!(
            encode(&cs),
            [
                42, 0, 0, 0, 0, 0, 0, 0, // txn_id
                1, 0, 0, 0, // count
                1, 0, 0, 0, 1, 0, 0, 0, // handle
                0, 0, 0, 0, // signal_id
                0, // op
                12, 0, 0, 0, // len
                2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, // value
            ]
        );
    }

    #[test]
    fn owned_round_trip() {
        let cs = sample();
        let b = encode(&cs);
        let mut r = Reader::new(&b);
        assert_eq!(ChangeSet::decode(&mut r), Ok(cs));
        assert!(r.finish().is_ok());
    }

    #[test]
    fn empty_change_set() {
        let cs = ChangeSet {
            txn_id: 1,
            entries: vec![],
        };
        let b = encode(&cs);
        assert_eq!(b.len(), 12);
        let set = ChangeSetRef::decode(&mut Reader::new(&b)).unwrap();
        assert!(set.is_empty());
        assert_eq!(set.iter().count(), 0);
    }

    #[test]
    fn ref_iterates_without_copying() {
        let cs = sample();
        let b = encode(&cs);
        let set = ChangeSetRef::decode(&mut Reader::new(&b)).unwrap();
        assert_eq!(set.txn_id, 42);
        assert_eq!(set.len(), 3);
        let iter = set.iter();
        assert_eq!(iter.len(), 3);
        let range = b.as_ptr_range();
        let mut seen = 0;
        for (entry, expected) in set.iter().zip(&cs.entries) {
            assert_eq!(entry.handle, expected.handle);
            assert_eq!(entry.signal_id, expected.signal_id);
            assert_eq!(entry.op, expected.op);
            assert_eq!(entry.value, &expected.value[..]);
            if !entry.value.is_empty() {
                assert!(range.contains(&entry.value.as_ptr()), "value must borrow");
            }
            seen += 1;
        }
        assert_eq!(seen, 3);
        // `&ChangeSetRef` is iterable too, and iteration can be repeated.
        assert_eq!((&set).into_iter().count(), 3);
    }

    #[test]
    fn ref_leaves_the_reader_after_the_entries() {
        let mut b = encode(&sample());
        b.extend_from_slice(&[0xaa, 0xbb]);
        let mut r = Reader::new(&b);
        ChangeSetRef::decode(&mut r).unwrap();
        assert_eq!(r.remaining(), 2);
        assert_eq!(r.finish(), Err(WireError::TrailingBytes { count: 2 }));
    }

    #[test]
    fn unknown_op_is_rejected() {
        let mut b = encode(&sample());
        // First entry's op byte: 8 (txn) + 4 (count) + 8 (handle) + 4 (signal) = 24.
        b[24] = 3;
        assert_eq!(
            ChangeSetRef::decode(&mut Reader::new(&b)).unwrap_err(),
            WireError::InvalidTag {
                tag: 3,
                at: 24,
                ty: "ChangeOp"
            }
        );
    }

    #[test]
    fn hostile_entry_count_is_rejected_before_iteration() {
        let mut b = Vec::new();
        b.extend_from_slice(&1_u64.to_le_bytes());
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            ChangeSetRef::decode(&mut Reader::new(&b)),
            Err(WireError::LengthTooLarge { .. })
        ));
    }

    #[test]
    fn value_length_beyond_the_message_is_rejected() {
        let mut b = encode(&sample());
        // First entry's len field: 24 (op at 24) + 1 = 25.
        b[25..29].copy_from_slice(&1000_u32.to_le_bytes());
        assert!(matches!(
            ChangeSetRef::decode(&mut Reader::new(&b)),
            Err(WireError::LengthTooLarge { len: 1000, .. })
        ));
    }

    #[test]
    fn truncation_is_an_error_at_every_length() {
        let b = encode(&sample());
        for cut in 0..b.len() {
            assert!(
                ChangeSetRef::decode(&mut Reader::new(&b[..cut])).is_err(),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn builder_matches_owned_encoding() {
        let cs = sample();
        let mut w = Writer::new();
        let mut b = ChangeSetBuilder::new(&mut w, cs.txn_id);
        assert!(b.is_empty());
        b.push(
            cs.entries[0].handle,
            cs.entries[0].signal_id,
            cs.entries[0].op,
            &cs.entries[0].value,
        );
        let e1 = &cs.entries[1];
        b.push_with(e1.handle, e1.signal_id, e1.op, |w| w.write_raw(&e1.value));
        let e2 = &cs.entries[2];
        b.push_with(e2.handle, e2.signal_id, e2.op, |_| {});
        assert_eq!(b.len(), 3);
        assert_eq!(b.finish(), 3);
        assert_eq!(w.as_slice(), &encode(&cs)[..]);
    }

    #[test]
    fn builder_keeps_the_count_current_without_finish() {
        let mut w = Writer::new();
        {
            let mut b = ChangeSetBuilder::new(&mut w, 1);
            b.push(Handle(1), 0, ChangeOp::Full, &[1]);
        }
        let set = ChangeSetRef::decode(&mut Reader::new(w.as_slice())).unwrap();
        assert_eq!(set.len(), 1);
    }
}
