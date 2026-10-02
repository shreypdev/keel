//! What a platform can call: a query handle's constructor, its methods (`refetch`, `invalidate`,
//! `set_poll_interval` and, on an infinite query's handle, `fetch_next_page`), and every mutation
//! as a function.
//!
//! `undra-bindgen` synthesizes, for each query `todos` with parameters `(page)`:
//!
//! * a `TodosQueryHandle` object whose type id and constructor method id are both the query id
//!   (`fnv1a32("query.todos")`), the constructor taking the query's parameters in order and
//!   answering the object's `u64` handle;
//! * sync methods on that object, `refetch()` (`fnv1a32("query.refetch")`), `invalidate()`
//!   (`fnv1a32("query.invalidate")`) and `set_poll_interval(Option<Duration>)`
//!   ([`SET_POLL_INTERVAL_METHOD_ID`](undra_meta::ids::SET_POLL_INTERVAL_METHOD_ID)), the same ids
//!   on every handle type, and on the handle of an `infinite` query `fetch_next_page()`
//!   ([`FETCH_NEXT_PAGE_METHOD_ID`](undra_meta::ids::FETCH_NEXT_PAGE_METHOD_ID), no arguments);
//!
//! and for each mutation an async free function whose method id is the mutation id
//! (`fnv1a32("mutation.add_todo")`), taking the input parameters in order and answering the
//! mutation's `Result<T, E>` (status 0 with the `T`, status 1 with the `E`).
//!
//! None of these ids is in the static dispatch table (they are generic instantiations, and
//! registering them as objects would put them in the schema twice), so this module is a
//! [`DispatchLayer`](undra_runtime::DispatchLayer): the runtime asks it when its own table has no
//! answer, and it finds the query or mutation through the registrations `#[undra::query]` and
//! `#[undra::mutation]` submit. The macros submit the layer itself too
//! ([`crate::__private::LAYER`]), so a core without queries or mutations does not link it.

use core::any::Any;

use undra_meta::{DispatchCall, DispatchOutcome, ids};
use undra_runtime::{DispatchResult, Runtime};
use undra_wire::{Decode, Encode};

use crate::client::QueryClient;
use crate::erased::{MutationVTable, QueryVTable, registered_mutation, registered_query};
use crate::handle::{HandleObject, HandleObjectInner};
use crate::shared::shared_of;

/// The method id of `refetch()` on every query handle: `fnv1a32("query.refetch")`.
pub const REFETCH_METHOD_ID: u32 = ids::fnv1a32("query.refetch");

/// The method id of `invalidate()` on every query handle: `fnv1a32("query.invalidate")`.
pub const INVALIDATE_METHOD_ID: u32 = ids::fnv1a32("query.invalidate");

/// The method id of `set_poll_interval(Option<Duration>)` on every query handle:
/// `fnv1a32("QueryHandle.set_poll_interval")` (ADR-043).
pub const SET_POLL_INTERVAL_METHOD_ID: u32 = ids::SET_POLL_INTERVAL_METHOD_ID;

/// The method id of `fetch_next_page()` on the handle of an infinite query:
/// `fnv1a32("QueryHandle.fetch_next_page")` (ADR-043).
pub const FETCH_NEXT_PAGE_METHOD_ID: u32 = ids::FETCH_NEXT_PAGE_METHOD_ID;

pub(crate) fn dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    DispatchOutcome::new(serve(rt, call))
}

fn serve(rt: &Runtime, call: DispatchCall<'_>) -> DispatchResult {
    if call.handle != 0 {
        return handle_method(rt, call);
    }
    if let Some(vt) = registered_query(call.method_id) {
        return construct(rt, vt, call);
    }
    if let Some(vt) = registered_mutation(call.method_id) {
        return mutate(rt, vt, call);
    }
    DispatchResult::Unknown
}

/// `TodosQueryHandle(page)`: observes the query and answers the new object's handle.
fn construct(rt: &Runtime, vt: &'static QueryVTable, call: DispatchCall<'_>) -> DispatchResult {
    let ctx = rt.ctx();
    let shared = shared_of(rt);
    shared.start(&ctx);
    let client = QueryClient::bound(ctx, shared);
    let ops = match (vt.open)(&client, call.args) {
        Ok(ops) => ops,
        Err(e) => {
            return DispatchResult::BadRequest(format!(
                "bad arguments for the query handle of `{}`: {e}",
                vt.key
            ));
        }
    };
    let cell = match ops.make_cell() {
        Ok(cell) => cell,
        Err(e) => {
            return DispatchResult::BadRequest(format!(
                "the query handle of `{}` could not attach its signals: {e}",
                vt.key
            ));
        }
    };
    let handle = rt.insert(std::sync::Arc::new(HandleObject::new(vt.id, ops, cell)));
    DispatchResult::Sync(Ok(handle.encode_to_vec()))
}

/// A method of a handle: `refetch`, `invalidate`, `set_poll_interval` and `fetch_next_page`.
fn handle_method(rt: &Runtime, call: DispatchCall<'_>) -> DispatchResult {
    let object = match rt.object::<HandleObjectInner>(call.handle) {
        Ok(object) => object,
        Err(e) => return DispatchResult::BadRequest(e.to_string()),
    };
    match call.method_id {
        REFETCH_METHOD_ID => object.ops.refetch(),
        INVALIDATE_METHOD_ID => object.ops.invalidate(),
        ids::SET_POLL_INTERVAL_METHOD_ID => {
            match Option::<core::time::Duration>::decode_exact(call.args) {
                Ok(interval) => object.ops.set_poll_interval(interval),
                Err(e) => {
                    return DispatchResult::BadRequest(format!(
                        "bad arguments for `set_poll_interval` (an optional duration): {e}"
                    ));
                }
            }
        }
        // Only an infinite query's handle has one.
        ids::FETCH_NEXT_PAGE_METHOD_ID => {
            if !object.ops.fetch_next_page() {
                return DispatchResult::Unknown;
            }
        }
        _ => return DispatchResult::Unknown,
    }
    DispatchResult::Sync(Ok(Vec::new()))
}

/// A mutation called as a free function: runs `ctx.mutate` and encodes the result.
fn mutate(rt: &Runtime, vt: &'static MutationVTable, call: DispatchCall<'_>) -> DispatchResult {
    let ctx = rt.ctx();
    match (vt.dispatch)(ctx, call.args) {
        Ok(future) => DispatchResult::Async(future),
        Err(e) => {
            DispatchResult::BadRequest(format!("bad arguments for the mutation `{}`: {e}", vt.key))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_method_ids_are_the_ones_bindgen_hard_codes() {
        // `undra-bindgen::QUERY_REFETCH_ID` / `QUERY_INVALIDATE_ID`, and the runtimes' copies.
        assert_eq!(REFETCH_METHOD_ID, 0x21d1_b9e2);
        assert_eq!(INVALIDATE_METHOD_ID, 0x44ce_c2fa);
        assert_ne!(REFETCH_METHOD_ID, INVALIDATE_METHOD_ID);
    }
}
