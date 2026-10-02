//! Query handles across a restore (ADR-059): what a snapshot keeps of a handle, and what builds it
//! again when the host first uses the handle after a restore.
//!
//! A query handle is a view of the query cache, so a snapshot does not carry it as a store: it
//! carries a small **record** of what the handle is made of, a recreation record
//! ([`UndraObjectDyn::recreation`](undra_runtime::UndraObjectDyn::recreation)). A restore re-issues
//! the handle's value from the record without building anything (no observer, no fetch, no timer);
//! the object is built, in the same table entry, the first time the host uses the handle: what the
//! constructor call did the first time, with the recorded parameters. This module is the
//! [`Reviver`] the runtime asks for both, and the record's format.
//!
//! # The record
//!
//! ```text
//! format u16 = 1, params bytes, poll_ms Option<u64>
//! ```
//!
//! * `params` is the canonical encoding of the query's parameters: the cache key's bytes, which are
//!   the constructor's arguments. A query's parameters are values (an object or a callback is
//!   refused there), so the record never holds a handle or a callback instance.
//! * `poll_ms` is the interval this observer set for itself (`set_poll_interval`), if any, in
//!   milliseconds; it is set again once the object is built.
//! * No time, no data and nothing of the cache is in it: a query without `persist` is one the
//!   developer chose not to write down, and a snapshot is something apps write down. A
//!   non-persisted query therefore shows "loading" once after a dev reload, and then the data
//!   its freshly built code fetched; a persisted one shows its stored data at once and `stale`
//!   decides whether a fetch starts (exactly what constructing the handle answers).
//!
//! The reviver's fingerprint, which a restore compares with the one the snapshot was written
//! with, is the `fnv1a64` of the canonical closure of the query's parameters by name
//! (`Schema::closure_of_params`, ADR-037's function for a mutation's input): a record whose
//! parameter types changed is refused (its handle stays stale, the restore goes on). A query the
//! schema does not describe (a hand-written [`QueryDef`](crate::QueryDef)) has the fingerprint
//! `0` in both builds, so only a record that no longer decodes is refused.

use std::sync::Arc;

use undra_meta::QueryKind;
use undra_runtime::{AnyObject, Reviver, Runtime};
use undra_wire::{Decode, Encode, Reader, WireError, Writer};

use crate::client::QueryClient;
use crate::erased::registered_query;
use crate::handle::HandleObject;
use crate::shared::shared_of;

/// The version of the record's layout.
const FORMAT: u16 = 1;

/// What a snapshot keeps of a query handle: the encoded parameters and the polling interval its
/// observer set for itself.
pub(crate) fn encode_record(params: &[u8], poll_ms: Option<u64>) -> Vec<u8> {
    let mut w = Writer::with_capacity(2 + 4 + params.len() + 9);
    FORMAT.encode(&mut w);
    w.write_bytes(params);
    poll_ms.encode(&mut w);
    w.into_vec()
}

/// Reads a record: the encoded parameters and the polling interval.
fn decode_record(record: &[u8]) -> Result<(&[u8], Option<u64>), WireError> {
    let mut r = Reader::new(record);
    let at = r.position();
    let format = u16::decode(&mut r)?;
    if format != FORMAT {
        return Err(WireError::InvalidTag {
            tag: u32::from(format),
            at,
            ty: "query handle record format",
        });
    }
    let params = r.read_bytes()?;
    let poll_ms = Option::<u64>::decode(&mut r)?;
    r.finish()?;
    Ok((params, poll_ms))
}

/// What builds query handles again after a restore. Added to a runtime by [`shared_of`], where the
/// client's stats section is, so a core that never uses the query runtime links none of it.
pub(crate) const REVIVER: Reviver = Reviver {
    name: "undra-query",
    object_name: "QueryHandle",
    fingerprint,
    check,
    revive,
};

/// The fingerprint of the closure of query `type_id`'s parameters; `None` for a type that is not a
/// registered query.
fn fingerprint(rt: &Runtime, type_id: u32) -> Option<u64> {
    registered_query(type_id)?;
    let schema = rt.schema();
    Some(
        schema
            .queries
            .iter()
            .find(|q| q.query_id == type_id && q.kind == QueryKind::Query)
            .map_or(0, |q| schema.closure_of_params(&q.params).fingerprint()),
    )
}

/// Decodes the record and the parameters, so a record that passed a restore cannot fail to open.
fn check(_rt: &Runtime, type_id: u32, record: &[u8]) -> Result<(), String> {
    let vt = registered_query(type_id).ok_or("the query is not registered")?;
    let (params, _) =
        decode_record(record).map_err(|e| format!("the record does not decode: {e}"))?;
    (vt.check_params)(params).map_err(|e| {
        format!(
            "the parameters of `{}` no longer decode as they were recorded: {e}",
            vt.key
        )
    })
}

/// What the constructor call does (`vt.open` with the recorded parameters: an observer on the cache
/// entry, a fetch if the entry is stale or missing by the rules of SPEC 9, the entry's current view
/// shown), then the recorded polling interval is set again.
fn revive(rt: &Runtime, type_id: u32, record: &[u8]) -> Result<Arc<dyn AnyObject>, String> {
    let vt = registered_query(type_id).ok_or("the query is not registered")?;
    let (params, poll_ms) =
        decode_record(record).map_err(|e| format!("the record does not decode: {e}"))?;
    let ctx = rt.ctx();
    let shared = shared_of(rt);
    shared.start(&ctx);
    let client = QueryClient::bound(ctx, shared);
    let ops = (vt.open)(&client, params).map_err(|e| e.to_string())?;
    let cell = ops.make_cell().map_err(|e| e.to_string())?;
    if let Some(ms) = poll_ms {
        ops.set_poll_interval(Some(core::time::Duration::from_millis(ms)));
    }
    Ok(Arc::new(HandleObject::new(vt.id, ops, cell)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_is_the_format_the_parameters_and_the_polling_interval() {
        let record = encode_record(&[7, 0, 0, 0], Some(5_000));
        assert_eq!(
            record,
            [
                1, 0, // format
                4, 0, 0, 0, 7, 0, 0, 0, // params
                1, 0x88, 0x13, 0, 0, 0, 0, 0, 0, // Some(5000)
            ]
        );
        assert_eq!(
            decode_record(&record).unwrap(),
            (&[7_u8, 0, 0, 0][..], Some(5_000))
        );
        assert_eq!(
            decode_record(&encode_record(&[], None)).unwrap(),
            (&[][..], None)
        );
    }

    #[test]
    fn a_record_of_another_format_or_with_trailing_bytes_does_not_decode() {
        let mut record = encode_record(&[1], None);
        assert!(
            decode_record(&record[..record.len() - 1]).is_err(),
            "cut short"
        );
        record.push(0);
        assert!(decode_record(&record).is_err(), "a trailing byte");
        let mut other = encode_record(&[1], None);
        other[0] = 2;
        assert!(
            matches!(
                decode_record(&other),
                Err(WireError::InvalidTag { tag: 2, .. })
            ),
            "a format this build does not read"
        );
        assert!(decode_record(&[]).is_err());
    }
}
