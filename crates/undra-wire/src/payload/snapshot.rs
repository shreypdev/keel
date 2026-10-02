//! The snapshot payload (SPEC 5.9).

use crate::writer::len_u32;
use crate::{Handle, Reader, WireError, Writer};

/// Smallest possible encoded store: `handle u64, type_id u32, signal_count u32`.
const STORE_MIN_LEN: usize = 16;
/// Smallest possible encoded signal: `signal_id u32, len u32`.
const SIGNAL_MIN_LEN: usize = 8;

/// The signal id of a **recreation record** (ADR-059): the one field of a record that is not a
/// store's but what an object that is not snapshotted as a store is built again from (a query
/// handle: its parameters). No store has a signal with this id (signals are numbered from 0 in
/// declaration order, so a store would need four billion of them; `u32::MAX` is `ALL_SIGNALS`), so
/// a record with exactly this one field is told apart from a store's by [`StoreSnapshot::recreation`].
pub const RECREATION_FIELD: u32 = 0xFFFF_FFFE;

/// One record of a [`Snapshot`]: a store's persisted state (its handle, its type and the encoded
/// value of every non-computed signal), or a **recreation record** (ADR-059): the handle of an
/// object that is built again from what it was made of, and under [`RECREATION_FIELD`] the bytes
/// the runtime's reviver for that type wrote. Both have the same shape on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreSnapshot {
    /// The store's handle; restore re-issues the same handle.
    pub handle: Handle,
    /// `fnv1a32("<TypeName>")` of the store type (for a recreation record, the object's type id).
    pub type_id: u32,
    /// `(signal_id, encoded value)` pairs. Computed signals are excluded; they are restored
    /// by recomputation. A recreation record has exactly one, under [`RECREATION_FIELD`].
    pub signals: Vec<(u32, Vec<u8>)>,
}

impl StoreSnapshot {
    /// The recreation bytes, when this record is a recreation record (exactly one field, under
    /// [`RECREATION_FIELD`]) and not a store's; `None` for a store, whatever its signals are.
    ///
    /// ```
    /// use undra_wire::payload::{RECREATION_FIELD, StoreSnapshot};
    /// use undra_wire::Handle;
    ///
    /// let record = StoreSnapshot {
    ///     handle: Handle::new(2, 1),
    ///     type_id: 7,
    ///     signals: vec![(RECREATION_FIELD, vec![1, 0])],
    /// };
    /// assert_eq!(record.recreation(), Some(&[1_u8, 0][..]));
    /// let store = StoreSnapshot { signals: vec![(0, vec![1, 0])], ..record };
    /// assert_eq!(store.recreation(), None);
    /// ```
    #[must_use]
    pub fn recreation(&self) -> Option<&[u8]> {
        match self.signals.as_slice() {
            [(RECREATION_FIELD, bytes)] => Some(bytes),
            _ => None,
        }
    }

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

/// The identity of one store type in a [`Snapshot`]: its type id and the fingerprint of its
/// signals' closure when the snapshot was taken (ADR-037).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SnapshotType {
    /// `fnv1a32("<TypeName>")` of the store type.
    pub type_id: u32,
    /// `fnv1a64` of the canonical closure of the store's non-computed signals (by name, type and
    /// `signal_id`, with every record and enum they reach). Restore compares it with the current
    /// build's: equal means the bytes decode as they are.
    pub fingerprint: u64,
}

/// Smallest possible encoded type entry: `type_id u32, fingerprint u64`.
const TYPE_LEN: usize = 12;

/// A snapshot of every store, and of the objects that are built again (kind `Snapshot`, SPEC 5.9,
/// layout 2 of ADR-037).
///
/// Layout, all little-endian:
///
/// ```text
/// count u32, generation_floor u64,                              // ADR-022's leading words, the floor widened by ADR-040
/// schema_hash u64,                                              // of the core that wrote it
/// type_count u32, types x { type_id u32, fingerprint u64 },     // the fast path
/// description_len u32, description bytes,                       // canonical JSON (UTF-8)
/// count x { handle u64, type_id u32, signal_count u32, signals x { signal_id u32, len u32, value } }
/// ```
///
/// `count` is the number of records, each a store's or a **recreation record** (ADR-059: one field
/// under [`RECREATION_FIELD`], see [`StoreSnapshot::recreation`]); the layout is the same. Objects
/// that are neither are not part of a snapshot.
///
/// `generation_floor` is the highest handle generation the core had issued when the snapshot
/// was taken (`0` if it had issued none). A restore raises the core's generation counter to at
/// least that, so no handle issued before the snapshot (or between it and the restore) can be
/// issued again to a different object (ADR-022).
///
/// `types` lists each store type once with the fingerprint of its signals (and each type of a
/// recreation record once, with the fingerprint of what its record depends on); `description` names,
/// per store type, its signals (`signal_id`, name, type) and every record and enum they reach, so
/// a build whose types changed can decode the values by name and migrate them (ADR-037). It is
/// read only when a fingerprint differs. A store whose type is not in `types`, or a type listed
/// twice, does not decode: that, and the 64-bit hash after the floor, is what makes a snapshot in
/// the layout before ADR-037 fail with a typed error instead of decoding as something else.
///
/// # Example
///
/// ```
/// use undra_wire::payload::{Snapshot, SnapshotType, StoreSnapshot};
/// use undra_wire::{Handle, Reader, Writer};
///
/// let snap = Snapshot {
///     generation_floor: 1,
///     schema_hash: 0x0123_4567_89ab_cdef,
///     types: vec![SnapshotType { type_id: 0xc0ffee, fingerprint: 7 }],
///     description: "{}".to_owned(),
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
    /// The highest handle generation issued when the snapshot was taken; every store's
    /// generation is at most this. Restore resumes the generation counter above it.
    pub generation_floor: u64,
    /// The schema hash of the core that took the snapshot.
    pub schema_hash: u64,
    /// Each store type of the snapshot, once, with its fingerprint.
    pub types: Vec<SnapshotType>,
    /// The canonical JSON description of the store types' closures (ADR-037). Opaque to hosts.
    pub description: String,
    /// The records (stores, then recreation records), in the order the runtime produced them.
    pub stores: Vec<StoreSnapshot>,
}

impl Snapshot {
    /// The fingerprint recorded for `type_id`, if the snapshot lists it.
    pub fn fingerprint(&self, type_id: u32) -> Option<u64> {
        self.types
            .iter()
            .find(|t| t.type_id == type_id)
            .map(|t| t.fingerprint)
    }

    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_len(len_u32(self.stores.len()));
        w.write_u64(self.generation_floor);
        w.write_u64(self.schema_hash);
        w.write_len(len_u32(self.types.len()));
        for t in &self.types {
            w.write_u32(t.type_id);
            w.write_u64(t.fingerprint);
        }
        w.write_str(&self.description);
        for store in &self.stores {
            store.encode(w);
        }
    }

    /// Reads a payload. Fails on a store whose type is not listed, on a type listed twice, and on
    /// a description that is not UTF-8.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let count = r.read_count(STORE_MIN_LEN)?;
        let generation_floor = r.read_u64()?;
        let schema_hash = r.read_u64()?;
        let type_count = r.read_count(TYPE_LEN)?;
        let mut types: Vec<SnapshotType> = Vec::with_capacity(type_count.min(1024));
        for _ in 0..type_count {
            let at = r.position();
            let type_id = r.read_u32()?;
            let fingerprint = r.read_u64()?;
            if types.iter().any(|t| t.type_id == type_id) {
                return Err(WireError::DuplicateKey { at });
            }
            types.push(SnapshotType {
                type_id,
                fingerprint,
            });
        }
        let description = r.read_str()?.to_owned();
        let mut stores = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let at = r.position();
            let store = StoreSnapshot::decode(r)?;
            if !types.iter().any(|t| t.type_id == store.type_id) {
                return Err(WireError::InvalidTag {
                    tag: store.type_id,
                    at,
                    ty: "Snapshot store type (not in the type table)",
                });
            }
            stores.push(store);
        }
        Ok(Snapshot {
            generation_floor,
            schema_hash,
            types,
            description,
            stores,
        })
    }
}

/// The payload of a `Restore` message: identical to [`Snapshot`].
pub type Restore = Snapshot;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            generation_floor: 5,
            schema_hash: 0xfeed_beef_0000_0001,
            types: vec![
                SnapshotType {
                    type_id: 7,
                    fingerprint: 0x11,
                },
                SnapshotType {
                    type_id: 8,
                    fingerprint: 0x22,
                },
            ],
            description: r#"{"stores":[]}"#.to_owned(),
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
            generation_floor: 0x0102_0304_0506_0708,
            schema_hash: 0x0807_0605_0403_0201,
            types: vec![SnapshotType {
                type_id: 7,
                fingerprint: 0x0a,
            }],
            description: "{}".to_owned(),
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
                8, 7, 6, 5, 4, 3, 2, 1, // generation_floor (u64)
                1, 2, 3, 4, 5, 6, 7, 8, // schema_hash
                1, 0, 0, 0, // type_count
                7, 0, 0, 0, 0x0a, 0, 0, 0, 0, 0, 0, 0, // type_id, fingerprint
                2, 0, 0, 0, b'{', b'}', // description
                1, 0, 0, 1, 0, 0, 0, 0, // handle (index 1, generation 1 << 24)
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
        assert_eq!(Snapshot::decode(&mut r), Ok(snap.clone()));
        assert!(r.finish().is_ok());
        assert_eq!(snap.fingerprint(8), Some(0x22));
        assert_eq!(snap.fingerprint(9), None);
    }

    #[test]
    fn empty_snapshot() {
        let b = encode(&Snapshot::default());
        assert_eq!(b, [0; 28]);
        assert_eq!(
            Snapshot::decode(&mut Reader::new(&b)),
            Ok(Snapshot::default())
        );
    }

    #[test]
    fn hostile_counts_are_rejected() {
        // Store count larger than the input could hold.
        let b = [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0];
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&b)),
            Err(WireError::LengthTooLarge { .. })
        ));
        // Type count larger than the input could hold.
        let mut b = Vec::new();
        b.extend_from_slice(&0_u32.to_le_bytes());
        b.extend_from_slice(&0_u64.to_le_bytes());
        b.extend_from_slice(&0_u64.to_le_bytes());
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&b)),
            Err(WireError::LengthTooLarge { .. })
        ));
        // Signal count larger than the input could hold.
        let mut snap = sample();
        snap.stores.truncate(1);
        snap.stores[0].signals.clear();
        let mut b = encode(&snap);
        let at = b.len() - 4;
        b[at..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&b)),
            Err(WireError::LengthTooLarge { .. })
        ));
    }

    #[test]
    fn a_store_of_an_unlisted_type_or_a_type_listed_twice_is_refused() {
        let mut snap = sample();
        snap.types.pop();
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&encode(&snap))),
            Err(WireError::InvalidTag { tag: 8, .. })
        ));
        let mut snap = sample();
        snap.types[1].type_id = 7;
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&encode(&snap))),
            Err(WireError::DuplicateKey { .. })
        ));
    }

    #[test]
    fn a_description_that_is_not_utf8_is_refused() {
        let mut b = encode(&Snapshot {
            description: "ab".to_owned(),
            ..Snapshot::default()
        });
        let at = b.len() - 2;
        b[at] = 0xff;
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&b)),
            Err(WireError::InvalidUtf8 { .. })
        ));
    }

    #[test]
    fn a_pre_floor_snapshot_is_refused() {
        // The layout before ADR-022 (`count u32, stores`): an empty one is four bytes, which
        // now ends where the floor should be.
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&[0, 0, 0, 0])),
            Err(WireError::UnexpectedEof { .. })
        ));
    }

    /// The layout of ADR-022 (`count u32, generation_floor u32, stores`), which ADR-037 replaced:
    /// it never decodes as the new one, so a host that kept one gets a typed refusal.
    fn layout_1(stores: &[StoreSnapshot], floor: u32) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_len(len_u32(stores.len()));
        w.write_u32(floor);
        for store in stores {
            store.encode(&mut w);
        }
        w.into_vec()
    }

    #[test]
    fn a_snapshot_in_the_layout_before_adr_037_is_refused() {
        // Empty: eight bytes, which end where the schema hash should be.
        assert!(matches!(
            Snapshot::decode(&mut Reader::new(&layout_1(&[], 0))),
            Err(WireError::UnexpectedEof { .. })
        ));
        // With stores: the first handle reads as the hash, the type id as the type count, and the
        // type table does not fit (or names nothing the stores use).
        let old = layout_1(&sample().stores, 5);
        assert!(Snapshot::decode(&mut Reader::new(&old)).is_err());
        for type_id in [1_u32, 7, 0x00c0_ffee, u32::MAX] {
            let stores = [StoreSnapshot {
                handle: Handle::new(1, 1),
                type_id,
                signals: vec![(0, vec![1, 2, 3, 4])],
            }];
            let bytes = layout_1(&stores, 1);
            let mut r = Reader::new(&bytes);
            let decoded = Snapshot::decode(&mut r).and_then(|s| r.finish().map(|()| s));
            assert!(decoded.is_err(), "type id {type_id:#x}");
        }
    }

    /// Layout 2 of ADR-037 wrote the floor as a `u32`; ADR-040 widened it to `u64`. The old bytes
    /// never decode as the new layout, so a host that kept a snapshot gets a typed refusal.
    #[test]
    fn a_snapshot_with_a_32_bit_floor_is_refused() {
        let snap = sample();
        let mut w = Writer::new();
        w.write_len(len_u32(snap.stores.len()));
        w.write_u32(5); // the old 32-bit floor
        w.write_u64(snap.schema_hash);
        w.write_len(len_u32(snap.types.len()));
        for t in &snap.types {
            w.write_u32(t.type_id);
            w.write_u64(t.fingerprint);
        }
        w.write_str(&snap.description);
        for store in &snap.stores {
            store.encode(&mut w);
        }
        let old = w.into_vec();
        let mut r = Reader::new(&old);
        let decoded = Snapshot::decode(&mut r).and_then(|s| r.finish().map(|()| s));
        assert!(decoded.is_err(), "{decoded:?}");
    }

    /// A recreation record (ADR-059) is a record of the layout every runtime reads, with one field
    /// under the reserved id: it encodes and decodes as a store's does and is told apart by its field.
    #[test]
    fn a_recreation_record_is_a_record_with_one_reserved_field() {
        let record = StoreSnapshot {
            handle: Handle::new(3, 2),
            type_id: 9,
            signals: vec![(RECREATION_FIELD, vec![1, 0, 7])],
        };
        assert_eq!(RECREATION_FIELD, 0xFFFF_FFFE);
        assert_eq!(record.recreation(), Some(&[1_u8, 0, 7][..]));
        let mut snap = sample();
        snap.types.push(SnapshotType {
            type_id: 9,
            fingerprint: 0x33,
        });
        snap.stores.push(record.clone());
        let b = encode(&snap);
        let mut r = Reader::new(&b);
        let decoded = Snapshot::decode(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(decoded, snap);
        assert_eq!(
            decoded
                .stores
                .iter()
                .map(|s| s.recreation().is_some())
                .collect::<Vec<_>>(),
            [false, false, true],
            "only the record with the reserved field is a recreation record"
        );
        // The bytes of the record are the bytes of a store with that one signal.
        let mut w = Writer::new();
        record.encode(&mut w);
        assert_eq!(
            w.as_slice(),
            [
                3, 0, 0, 2, 0, 0, 0, 0, // handle (index 3, generation 2 << 24)
                9, 0, 0, 0, // type_id
                1, 0, 0, 0, // signal_count
                0xfe, 0xff, 0xff, 0xff, // the reserved signal_id
                3, 0, 0, 0, 1, 0, 7, // len + record
            ]
        );
    }

    #[test]
    fn only_exactly_one_reserved_field_makes_a_recreation_record() {
        let with = |signals: Vec<(u32, Vec<u8>)>| StoreSnapshot {
            handle: Handle::new(1, 1),
            type_id: 7,
            signals,
        };
        assert_eq!(with(vec![]).recreation(), None);
        assert_eq!(with(vec![(0, vec![1])]).recreation(), None);
        assert_eq!(with(vec![(u32::MAX, vec![1])]).recreation(), None);
        assert_eq!(
            with(vec![(RECREATION_FIELD, vec![]), (0, vec![1])]).recreation(),
            None,
            "a store that also has other signals is a store"
        );
        assert_eq!(
            with(vec![(RECREATION_FIELD, vec![]), (RECREATION_FIELD, vec![])]).recreation(),
            None
        );
        assert_eq!(
            with(vec![(RECREATION_FIELD, vec![])]).recreation(),
            Some(&[][..]),
            "an empty record is still one"
        );
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
