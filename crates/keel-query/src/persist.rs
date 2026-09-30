//! The persisted forms: cache entries and the offline queue, both stored in the `Kv` port.
//!
//! * A cache entry lives under [`cache_key`] as `{ schema_hash u64, updated_at i64, data bytes }`
//!   (SPEC 9). It carries no parameters: the key holds `fnv1a64` of them, so an entry is picked
//!   up again when a query with those parameters is observed (see `Shared::observe`).
//! * The offline queue lives under [`QUEUE_KEY`] as
//!   `{ schema_hash u64, count u32, count x { mutation_id u32, params bytes, idempotency_key Uuid } }`.
//!   The schema hash is an addition to the spec's field list: the parameter bytes are only
//!   meaningful for the schema that produced them, so a queue written by another build is
//!   dropped rather than replayed with mis-decoded arguments (R7).

use keel_meta::ids::fnv1a64;
use keel_wire::{Decode, Encode, Reader, Uuid, WireError, Writer};

/// The `Kv` key of the offline queue.
pub const QUEUE_KEY: &str = "keel.query.queue";

/// The prefix of every persisted cache entry's `Kv` key.
pub const CACHE_KEY_PREFIX: &str = "keel.query.cache.";

/// The `Kv` key of the cache entry of query `query_id` with the encoded parameters `params`:
/// `keel.query.cache.<query_id>.<fnv1a64(params)>`, both as fixed-width lowercase hex.
///
/// ```
/// use keel_query::cache_key;
///
/// let key = cache_key(0x5420_9c7c, &[1, 0, 0, 0]);
/// assert!(key.starts_with("keel.query.cache.54209c7c."));
/// assert_eq!(key.len(), "keel.query.cache.".len() + 8 + 1 + 16);
/// ```
pub fn cache_key(query_id: u32, params: &[u8]) -> String {
    format!("{CACHE_KEY_PREFIX}{query_id:08x}.{:016x}", fnv1a64(params))
}

/// Splits a cache key into `(query_id, fnv1a64(params))`, or `None` if it is not one.
pub(crate) fn parse_cache_key(key: &str) -> Option<(u32, u64)> {
    let rest = key.strip_prefix(CACHE_KEY_PREFIX)?;
    let (id, hash) = rest.split_once('.')?;
    if id.len() != 8 || hash.len() != 16 {
        return None;
    }
    Some((
        u32::from_str_radix(id, 16).ok()?,
        u64::from_str_radix(hash, 16).ok()?,
    ))
}

/// A cache entry as read back from the `Kv` port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Persisted {
    pub schema_hash: u64,
    pub updated_at: i64,
    pub data: Vec<u8>,
}

pub(crate) fn encode_persisted(schema_hash: u64, updated_at: i64, data: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(20 + data.len());
    w.write_u64(schema_hash);
    w.write_i64(updated_at);
    w.write_bytes(data);
    w.into_vec()
}

pub(crate) fn decode_persisted(bytes: &[u8]) -> Result<Persisted, WireError> {
    let mut r = Reader::new(bytes);
    let schema_hash = r.read_u64()?;
    let updated_at = r.read_i64()?;
    let data = r.read_bytes()?.to_vec();
    r.finish()?;
    Ok(Persisted {
        schema_hash,
        updated_at,
        data,
    })
}

/// One mutation waiting for the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueuedMutation {
    /// The mutation's id (`fnv1a32("mutation.<fn>")`).
    pub mutation_id: u32,
    /// The encoded input.
    pub params: Vec<u8>,
    /// Stays the same across replays, so a server can drop a duplicate.
    pub idempotency_key: Uuid,
}

pub(crate) fn encode_queue(schema_hash: u64, items: &[QueuedMutation]) -> Vec<u8> {
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

/// The queue and the schema hash it was written under.
pub(crate) fn decode_queue(bytes: &[u8]) -> Result<(u64, Vec<QueuedMutation>), WireError> {
    let mut r = Reader::new(bytes);
    let schema_hash = r.read_u64()?;
    // Each item is at least 4 + 4 + 16 bytes.
    let count = r.read_count(24)?;
    let mut items = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let mutation_id = r.read_u32()?;
        let params = r.read_bytes()?.to_vec();
        let idempotency_key = Uuid::decode(&mut r)?;
        items.push(QueuedMutation {
            mutation_id,
            params,
            idempotency_key,
        });
    }
    r.finish()?;
    Ok((schema_hash, items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_round_trip_and_reject_lookalikes() {
        let key = cache_key(0xdead_beef, b"params");
        assert_eq!(
            parse_cache_key(&key),
            Some((0xdead_beef, fnv1a64(b"params")))
        );
        assert_eq!(parse_cache_key(QUEUE_KEY), None);
        assert_eq!(parse_cache_key("keel.query.cache."), None);
        assert_eq!(parse_cache_key("keel.query.cache.zz.yy"), None);
        assert_eq!(parse_cache_key("keel.query.cache.1.2"), None);
        assert_eq!(
            parse_cache_key("keel.query.cache.deadbeef.0123456789abcdeg"),
            None
        );
        assert_eq!(parse_cache_key("other.deadbeef.0123456789abcdef"), None);
    }

    #[test]
    fn persisted_entries_use_the_spec_layout() {
        let bytes = encode_persisted(0x0102_0304_0506_0708, -2, &[9, 8]);
        assert_eq!(
            bytes,
            [
                8, 7, 6, 5, 4, 3, 2, 1, 0xfe, 255, 255, 255, 255, 255, 255, 255, 2, 0, 0, 0, 9, 8
            ]
        );
        assert_eq!(
            decode_persisted(&bytes),
            Ok(Persisted {
                schema_hash: 0x0102_0304_0506_0708,
                updated_at: -2,
                data: vec![9, 8]
            })
        );
        assert!(decode_persisted(&bytes[..bytes.len() - 1]).is_err());
        let mut long = bytes;
        long.push(0);
        assert!(decode_persisted(&long).is_err());
    }

    #[test]
    fn the_queue_round_trips_and_rejects_truncation() {
        let items = vec![
            QueuedMutation {
                mutation_id: 7,
                params: vec![1, 2, 3],
                idempotency_key: Uuid([5; 16]),
            },
            QueuedMutation {
                mutation_id: 8,
                params: Vec::new(),
                idempotency_key: Uuid([6; 16]),
            },
        ];
        let bytes = encode_queue(99, &items);
        assert_eq!(decode_queue(&bytes), Ok((99, items)));
        for cut in 0..bytes.len() {
            assert!(decode_queue(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        assert_eq!(decode_queue(&encode_queue(1, &[])), Ok((1, Vec::new())));
    }

    #[test]
    fn a_huge_count_is_refused_before_allocating() {
        let mut bytes = 1_u64.to_le_bytes().to_vec();
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_queue(&bytes).is_err());
    }
}
