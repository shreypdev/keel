//! The snapshot payload (SPEC 5.9).

use crate::writer::len_u32;
use crate::{Handle, Reader, WireError, Writer};

/// Smallest possible encoded store: `handle u64, type_id u32, signal_count u32`.
const STORE_MIN_LEN: usize = 16;
/// Smallest possible encoded signal: `signal_id u32, len u32`.
const SIGNAL_MIN_LEN: usize = 8;

/// The persisted state of one store: its handle, its type and the encoded value of every
/// non-computed signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreSnapshot {
    /// The store's handle; restore re-issues the same handle.
    pub handle: Handle,
    /// `fnv1a32("<TypeName>")` of the store type.
    pub type_id: u32,
    /// `(signal_id, encoded value)` pairs. Computed signals are excluded; they are restored
    /// by recomputation.
    pub signals: Vec<(u32, Vec<u8>)>,
}

impl StoreSnapshot {
    /// Appends the store to `w`:
    /// `handle u64, type_id u32, signal_count u32, signal_count x { signal_id u32, len u32, value }`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u64(self.handle.0);
        w.write_u32(self.type_id);
        w.write_len(len_u32(self.signals.len()));
        for (signal_id, value) in &self.signals {
            w.write_u32(*signal_id);
            w.write_bytes(value);
        }
    }

    /// Reads one store.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let handle = Handle(r.read_u64()?);
        let type_id = r.read_u32()?;
        let count = r.read_count(SIGNAL_MIN_LEN)?;
        // Bounded reservation: the count is validated against the input, but each entry
        // still owns a heap allocation, so grow as they actually decode.
        let mut signals = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let signal_id = r.read_u32()?;
            let value = r.read_bytes()?.to_vec();
            signals.push((signal_id, value));
        }
        Ok(StoreSnapshot {
            handle,
            type_id,
            signals,
        })
    }
}

/// A snapshot of every store (kind `Snapshot`, SPEC 5.9).
///
/// Layout: `count u32, count x StoreSnapshot`. Objects that are not stores are not part of a
/// snapshot.
///
/// # Example
///
/// ```
/// use keel_wire::payload::{Snapshot, StoreSnapshot};
/// use keel_wire::{Handle, Reader, Writer};
///
/// let snap = Snapshot {
///     stores: vec![StoreSnapshot {
///         handle: Handle::new(3, 1),
///         type_id: 0xc0ffee,
///         signals: vec![(0, vec![1, 0, 0, 0]), (1, vec![])],
///     }],
/// };
/// let mut w = Writer::new();
/// snap.encode(&mut w);
/// assert_eq!(Snapshot::decode(&mut Reader::new(w.as_slice())), Ok(snap));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// The stores, in the order the runtime produced them.
    pub stores: Vec<StoreSnapshot>,
}

impl Snapshot {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_len(len_u32(self.stores.len()));
        for store in &self.stores {
            store.encode(w);
        }
    }

    /// Reads a payload.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let count = r.read_count(STORE_MIN_LEN)?;
        let mut stores = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            stores.push(StoreSnapshot::decode(r)?);
        }
        Ok(Snapshot { stores })
    }
}

/// The payload of a `Restore` message: identical to [`Snapshot`].
pub type Restore = Snapshot;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            stores: vec![
                StoreSnapshot {
                    handle: Handle::new(1, 1),
                    type_id: 7,
                    signals: vec![(0, vec![1, 2, 3]), (2, vec![])],
                },
                StoreSnapshot {
                    handle: Handle::new(2, 5),
                    type_id: 8,
                    signals: vec![],
                },
            ],
        }
    }

    fn encode(s: &Snapshot) -> Vec<u8> {
        let mut w = Writer::new();
        s.encode(&mut w);
        w.into_vec()
    }

    #[test]
    fn layout_matches_the_spec() {
        let snap = Snapshot {
            stores: vec![StoreSnapshot {
                handle: Handle::new(1, 1),
                type_id: 7,
                signals: vec![(2, vec![9])],
            }],
        };
        assert_eq!(
            encode(&snap),
            [
                1, 0, 0, 0, // store count
                1, 0, 0, 0, 1, 0, 0, 0, // handle
                7, 0, 0, 0, // type_id
                1, 0, 0, 0, // signal_count
                2, 0, 0, 0, // signal_id
                1, 0, 0, 0, 9, // len + value
            ]
        );
    }

    #[test]
    fn round_trip() {
        let snap = sample();
        let b = encode(&snap);
        let mut r = Reader::new(&b);
        assert_eq!(Snapshot::decode(&mut r), Ok(snap));
        assert!(r.finish().is_ok());
    }

    #[test]
    fn empty_snapshot() {
        let b = encode(&Snapshot::default());
        assert_eq!(b, [0, 0, 0, 0]);
        assert_eq!(
            Snapshot::decode(&mut Reader::new(&b)),
            Ok(Snapshot::default())
        );
    }

    #[test]
    fn hostile_counts_are_rejected() {
        // Store count larger than the input could hold.
        let b = [0xff, 0xff, 0xff, 0xff];
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&b)),
            Err(WireError::LengthTooLarge { .. })
        ));
        // Signal count larger than the input could hold.
        let mut b = Vec::new();
        b.extend_from_slice(&1_u32.to_le_bytes());
        b.extend_from_slice(&1_u64.to_le_bytes());
        b.extend_from_slice(&1_u32.to_le_bytes());
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&b)),
            Err(WireError::LengthTooLarge { .. })
        ));
    }

    #[test]
    fn truncation_is_an_error_at_every_length() {
        let b = encode(&sample());
        for cut in 0..b.len() {
            assert!(
                Snapshot::decode(&mut Reader::new(&b[..cut])).is_err(),
                "{cut}"
            );
        }
    }
}
