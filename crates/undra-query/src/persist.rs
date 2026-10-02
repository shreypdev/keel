//! The persisted forms (format 2, ADR-037): cache entries, the offline queue, its dead letters
//! and the type closures that identify them, all stored in the `Kv` port.
//!
//! | Key | Value |
//! |---|---|
//! | [`cache_key`]: `undra.query.cache2.<query_id>.<fnv1a64(params)>` | `format u16 = 2, schema_hash u64, fingerprint u64, updated_at i64, data bytes` |
//! | [`QUEUE_KEY`]: `undra.query.queue2` | `format u16 = 2, schema_hash u64, count u32, count x { mutation_id u32, fingerprint u64, params bytes, idempotency_key Uuid }` |
//! | [`DEAD_LETTER_KEY`]: `undra.query.queue.dead` | the queue's layout plus `reason String` per item |
//! | [`types_key`]: `undra.types.<fingerprint>` | the canonical JSON of the [`TypeClosure`](undra_meta::TypeClosure) with that fingerprint |
//!
//! A cache entry's fingerprint is its query's (the closure of its success type), a queue item's
//! its mutation's (the closure of its parameters). Each closure is stored once, under its
//! fingerprint, before the first entry or item that needs it, and deleted at hydration when
//! nothing references it any more. A dead letter whose identity is unknown (written by a build
//! before format 2) has fingerprint `0`.
//!
//! The keys of format 1 (`undra.query.cache.<id>.<hash>`, `undra.query.queue`) are read once at
//! hydration with their own decoders ([`decode_entry_v1`], [`decode_queue_v1`]) and then deleted;
//! a value's layout is never guessed from its bytes.

use undra_meta::ids::fnv1a64;
use undra_wire::{Decode, Encode, Reader, Uuid, WireError, Writer};

/// The `Kv` key of the offline queue (format 2).
pub const QUEUE_KEY: &str = "undra.query.queue2";

/// The `Kv` key of the offline queue of format 1, read once at hydration and then deleted.
pub const QUEUE_KEY_V1: &str = "undra.query.queue";

/// The `Kv` key of the dead-letter queue: queued mutations whose input could not be migrated to
/// this build, kept until the app retries or discards them (ADR-037 decision 7).
pub const DEAD_LETTER_KEY: &str = "undra.query.queue.dead";

/// The prefix of every persisted cache entry's `Kv` key (format 2).
pub const CACHE_KEY_PREFIX: &str = "undra.query.cache2.";

/// The prefix of the cache entries of format 1, read once at hydration and then deleted.
pub const CACHE_KEY_PREFIX_V1: &str = "undra.query.cache.";

/// The prefix of the stored type closures: `undra.types.<fingerprint as 16 hex digits>`.
pub const TYPES_KEY_PREFIX: &str = "undra.types.";

/// The format number every format-2 value starts with.
pub(crate) const FORMAT: u16 = 2;

/// The `Kv` key of the cache entry of query `query_id` with the encoded parameters `params`:
/// `undra.query.cache2.<query_id>.<fnv1a64(params)>`, both as fixed-width lowercase hex.
///
/// ```
/// use undra_query::cache_key;
///
/// let key = cache_key(0x5420_9c7c, &[1, 0, 0, 0]);
/// assert!(key.starts_with("undra.query.cache2.54209c7c."));
/// assert_eq!(key.len(), "undra.query.cache2.".len() + 8 + 1 + 16);
/// ```
pub fn cache_key(query_id: u32, params: &[u8]) -> String {
    cache_key_of(query_id, fnv1a64(params))
}

/// [`cache_key`] from the parameters' hash.
pub(crate) fn cache_key_of(query_id: u32, params_hash: u64) -> String {
    format!("{CACHE_KEY_PREFIX}{query_id:08x}.{params_hash:016x}")
}

/// The `Kv` key of the closure with `fingerprint`.
///
/// ```
/// assert_eq!(undra_query::types_key(0xab), "undra.types.00000000000000ab");
/// ```
pub fn types_key(fingerprint: u64) -> String {
    format!("{TYPES_KEY_PREFIX}{fingerprint:016x}")
}

fn parse_id_hash(rest: &str) -> Option<(u32, u64)> {
    let (id, hash) = rest.split_once('.')?;
    if id.len() != 8 || hash.len() != 16 {
        return None;
    }
    Some((
        u32::from_str_radix(id, 16).ok()?,
        u64::from_str_radix(hash, 16).ok()?,
    ))
}

/// Splits a format-2 cache key into `(query_id, fnv1a64(params))`, or `None` if it is not one.
pub(crate) fn parse_cache_key(key: &str) -> Option<(u32, u64)> {
    parse_id_hash(key.strip_prefix(CACHE_KEY_PREFIX)?)
}

/// Splits a format-1 cache key, or `None` if it is not one.
pub(crate) fn parse_cache_key_v1(key: &str) -> Option<(u32, u64)> {
    parse_id_hash(key.strip_prefix(CACHE_KEY_PREFIX_V1)?)
}

/// The fingerprint of a [`types_key`], or `None` if it is not one.
pub(crate) fn parse_types_key(key: &str) -> Option<u64> {
    let hex = key.strip_prefix(TYPES_KEY_PREFIX)?;
    if hex.len() != 16 {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

fn read_format(r: &mut Reader<'_>) -> Result<(), WireError> {
    let at = r.position();
    let format = r.read_u16()?;
    if format == FORMAT {
        Ok(())
    } else {
        Err(WireError::InvalidTag {
            tag: u32::from(format),
            at,
            ty: "persisted format (expected 2)",
        })
    }
}

// ----- cache entries --------------------------------------------------------------------------

/// A cache entry as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredEntry {
    pub schema_hash: u64,
    pub fingerprint: u64,
    pub updated_at: i64,
    pub data: Vec<u8>,
}

pub(crate) fn encode_entry(entry: &StoredEntry) -> Vec<u8> {
    let mut w = Writer::with_capacity(30 + entry.data.len());
    w.write_u16(FORMAT);
    w.write_u64(entry.schema_hash);
    w.write_u64(entry.fingerprint);
    w.write_i64(entry.updated_at);
    w.write_bytes(&entry.data);
    w.into_vec()
}

pub(crate) fn decode_entry(bytes: &[u8]) -> Result<StoredEntry, WireError> {
    let mut r = Reader::new(bytes);
    read_format(&mut r)?;
    let schema_hash = r.read_u64()?;
    let fingerprint = r.read_u64()?;
    let updated_at = r.read_i64()?;
    let data = r.read_bytes()?.to_vec();
    r.finish()?;
    Ok(StoredEntry {
        schema_hash,
        fingerprint,
        updated_at,
        data,
    })
}

/// A cache entry of format 1: `schema_hash u64, updated_at i64, data bytes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EntryV1 {
    pub schema_hash: u64,
    pub updated_at: i64,
    pub data: Vec<u8>,
}

pub(crate) fn decode_entry_v1(bytes: &[u8]) -> Result<EntryV1, WireError> {
    let mut r = Reader::new(bytes);
    let schema_hash = r.read_u64()?;
    let updated_at = r.read_i64()?;
    let data = r.read_bytes()?.to_vec();
    r.finish()?;
    Ok(EntryV1 {
        schema_hash,
        updated_at,
        data,
    })
}

#[cfg(test)]
pub(crate) fn encode_entry_v1(schema_hash: u64, updated_at: i64, data: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u64(schema_hash);
    w.write_i64(updated_at);
    w.write_bytes(data);
    w.into_vec()
}

// ----- the queue and its dead letters ---------------------------------------------------------

/// One mutation waiting for the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueuedMutation {
    /// The mutation's id (`fnv1a32("mutation.<fn>")`).
    pub mutation_id: u32,
    /// The fingerprint of the closure `params` was encoded with.
    pub fingerprint: u64,
    /// The encoded input.
    pub params: Vec<u8>,
    /// Stays the same across replays, so a server can drop a duplicate.
    pub idempotency_key: Uuid,
}

/// A queued mutation that could not be migrated, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeadItem {
    pub mutation: QueuedMutation,
    pub reason: String,
}

/// Smallest encoded item: `mutation_id u32, fingerprint u64, params len u32, key 16`.
const ITEM_MIN_LEN: usize = 32;

fn write_item(w: &mut Writer, item: &QueuedMutation) {
    w.write_u32(item.mutation_id);
    w.write_u64(item.fingerprint);
    w.write_bytes(&item.params);
    item.idempotency_key.encode(w);
}

fn read_item(r: &mut Reader<'_>) -> Result<QueuedMutation, WireError> {
    Ok(QueuedMutation {
        mutation_id: r.read_u32()?,
        fingerprint: r.read_u64()?,
        params: r.read_bytes()?.to_vec(),
        idempotency_key: Uuid::decode(r)?,
    })
}

pub(crate) fn encode_queue(schema_hash: u64, items: &[QueuedMutation]) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u16(FORMAT);
    w.write_u64(schema_hash);
    w.write_len(u32::try_from(items.len()).unwrap_or(u32::MAX));
    for item in items {
        write_item(&mut w, item);
    }
    w.into_vec()
}

/// The queue and the schema hash it was written under.
pub(crate) fn decode_queue(bytes: &[u8]) -> Result<(u64, Vec<QueuedMutation>), WireError> {
    let mut r = Reader::new(bytes);
    read_format(&mut r)?;
    let schema_hash = r.read_u64()?;
    let count = r.read_count(ITEM_MIN_LEN)?;
    let mut items = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        items.push(read_item(&mut r)?);
    }
    r.finish()?;
    Ok((schema_hash, items))
}

pub(crate) fn encode_dead(schema_hash: u64, items: &[DeadItem]) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u16(FORMAT);
    w.write_u64(schema_hash);
    w.write_len(u32::try_from(items.len()).unwrap_or(u32::MAX));
    for item in items {
        write_item(&mut w, &item.mutation);
        w.write_str(&item.reason);
    }
    w.into_vec()
}

pub(crate) fn decode_dead(bytes: &[u8]) -> Result<Vec<DeadItem>, WireError> {
    let mut r = Reader::new(bytes);
    read_format(&mut r)?;
    let _schema_hash = r.read_u64()?;
    let count = r.read_count(ITEM_MIN_LEN + 4)?;
    let mut items = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let mutation = read_item(&mut r)?;
        let reason = r.read_str()?.to_owned();
        items.push(DeadItem { mutation, reason });
    }
    r.finish()?;
    Ok(items)
}

/// One item of a format-1 queue: `mutation_id u32, params bytes, idempotency_key Uuid`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueuedV1 {
    pub mutation_id: u32,
    pub params: Vec<u8>,
    pub idempotency_key: Uuid,
}

/// A format-1 queue: `schema_hash u64, count u32, items`.
pub(crate) fn decode_queue_v1(bytes: &[u8]) -> Result<(u64, Vec<QueuedV1>), WireError> {
    let mut r = Reader::new(bytes);
    let schema_hash = r.read_u64()?;
    let count = r.read_count(24)?;
    let mut items = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let mutation_id = r.read_u32()?;
        let params = r.read_bytes()?.to_vec();
        let idempotency_key = Uuid::decode(&mut r)?;
        items.push(QueuedV1 {
            mutation_id,
            params,
            idempotency_key,
        });
    }
    r.finish()?;
    Ok((schema_hash, items))
}

#[cfg(test)]
pub(crate) fn encode_queue_v1(schema_hash: u64, items: &[QueuedV1]) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u64(schema_hash);
    w.write_len(u32::try_from(items.len()).unwrap_or(u32::MAX));
    for item in items {
        w.write_u32(item.mutation_id);
        w.write_bytes(&item.params);
        item.idempotency_key.encode(&mut w);
    }
    w.into_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u32, params: Vec<u8>, key: u8) -> QueuedMutation {
        QueuedMutation {
            mutation_id: id,
            fingerprint: u64::from(id) * 3,
            params,
            idempotency_key: Uuid([key; 16]),
        }
    }

    #[test]
    fn keys_round_trip_and_reject_lookalikes() {
        let key = cache_key(0xdead_beef, b"params");
        assert_eq!(
            parse_cache_key(&key),
            Some((0xdead_beef, fnv1a64(b"params")))
        );
        assert_eq!(
            parse_cache_key_v1(&key),
            None,
            "a format-2 key is not a format-1 key"
        );
        let v1 = format!("undra.query.cache.deadbeef.{:016x}", fnv1a64(b"params"));
        assert_eq!(
            parse_cache_key_v1(&v1),
            Some((0xdead_beef, fnv1a64(b"params")))
        );
        assert_eq!(parse_cache_key(&v1), None);
        assert_eq!(parse_cache_key(QUEUE_KEY), None);
        assert_eq!(parse_cache_key("undra.query.cache2."), None);
        assert_eq!(parse_cache_key("undra.query.cache2.zz.yy"), None);
        assert_eq!(parse_cache_key("undra.query.cache2.1.2"), None);
        assert_eq!(
            parse_cache_key("undra.query.cache2.deadbeef.0123456789abcdeg"),
            None
        );
        assert_eq!(parse_types_key(&types_key(u64::MAX)), Some(u64::MAX));
        assert_eq!(parse_types_key("undra.types.12"), None);
        assert_eq!(parse_types_key("undra.types.zzzzzzzzzzzzzzzz"), None);
        // The format-1 prefixes do not list the format-2 keys.
        assert!(!cache_key(1, &[]).starts_with(CACHE_KEY_PREFIX_V1));
    }

    #[test]
    fn entries_use_the_format_2_layout() {
        let entry = StoredEntry {
            schema_hash: 0x0102_0304_0506_0708,
            fingerprint: 0x11,
            updated_at: -2,
            data: vec![9, 8],
        };
        let bytes = encode_entry(&entry);
        assert_eq!(
            bytes,
            [
                2, 0, // format
                8, 7, 6, 5, 4, 3, 2, 1, // schema hash
                0x11, 0, 0, 0, 0, 0, 0, 0, // fingerprint
                0xfe, 255, 255, 255, 255, 255, 255, 255, // updated_at
                2, 0, 0, 0, 9, 8
            ]
        );
        assert_eq!(decode_entry(&bytes), Ok(entry));
        for cut in 0..bytes.len() {
            assert!(decode_entry(&bytes[..cut]).is_err(), "{cut}");
        }
        let mut long = bytes.clone();
        long.push(0);
        assert!(decode_entry(&long).is_err());
        // A format-1 value under a format-2 key is refused by its format, not guessed.
        assert!(decode_entry(&encode_entry_v1(1, 2, &[3])).is_err());
        assert_eq!(
            decode_entry_v1(&encode_entry_v1(1, 2, &[3])),
            Ok(EntryV1 {
                schema_hash: 1,
                updated_at: 2,
                data: vec![3]
            })
        );
    }

    #[test]
    fn the_queue_and_the_dead_letters_round_trip_and_reject_truncation() {
        let items = vec![item(7, vec![1, 2, 3], 5), item(8, Vec::new(), 6)];
        let bytes = encode_queue(99, &items);
        assert_eq!(decode_queue(&bytes), Ok((99, items.clone())));
        for cut in 0..bytes.len() {
            assert!(decode_queue(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        assert_eq!(decode_queue(&encode_queue(1, &[])), Ok((1, Vec::new())));
        let dead: Vec<DeadItem> = items
            .into_iter()
            .map(|mutation| DeadItem {
                mutation,
                reason: "why".into(),
            })
            .collect();
        let bytes = encode_dead(3, &dead);
        assert_eq!(decode_dead(&bytes), Ok(dead));
        for cut in 0..bytes.len() {
            assert!(decode_dead(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let v1 = vec![QueuedV1 {
            mutation_id: 4,
            params: vec![1],
            idempotency_key: Uuid([2; 16]),
        }];
        assert_eq!(decode_queue_v1(&encode_queue_v1(5, &v1)), Ok((5, v1)));
    }

    proptest::proptest! {
        /// The byte-fuzz of the storage decoders: whatever is in the store, reading it is an
        /// `Ok` or an `Err`, never a panic and never a huge allocation.
        #[test]
        fn arbitrary_bytes_never_panic_the_decoders(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..96)) {
            let _ = decode_entry(&bytes);
            let _ = decode_entry_v1(&bytes);
            let _ = decode_queue(&bytes);
            let _ = decode_queue_v1(&bytes);
            let _ = decode_dead(&bytes);
            let _ = parse_cache_key(&String::from_utf8_lossy(&bytes));
            let _ = parse_types_key(&String::from_utf8_lossy(&bytes));
        }

        #[test]
        fn a_queue_survives_a_round_trip(
            hash in proptest::prelude::any::<u64>(),
            items in proptest::collection::vec(
                (proptest::prelude::any::<u32>(), proptest::prelude::any::<u64>(), proptest::collection::vec(proptest::prelude::any::<u8>(), 0..16), proptest::prelude::any::<[u8; 16]>()),
                0..6,
            ),
        ) {
            let items: Vec<QueuedMutation> = items
                .into_iter()
                .map(|(id, fingerprint, params, key)| QueuedMutation { mutation_id: id, fingerprint, params, idempotency_key: Uuid(key) })
                .collect();
            proptest::prop_assert_eq!(decode_queue(&encode_queue(hash, &items)), Ok((hash, items)));
        }
    }

    #[test]
    fn a_huge_count_is_refused_before_allocating() {
        let mut bytes = 2_u16.to_le_bytes().to_vec();
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_queue(&bytes).is_err());
        assert!(decode_dead(&bytes).is_err());
    }
}
