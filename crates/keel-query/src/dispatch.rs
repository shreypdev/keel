//! What a platform can call: a query handle's constructor, its `refetch` and `invalidate`, and
//! every mutation as a function.
//!
//! `keel-bindgen` synthesizes, for each query `todos` with parameters `(page)`:
//!
//! * a `TodosQueryHandle` object whose type id and constructor method id are both the query id
//!   (`fnv1a32("query.todos")`), the constructor taking the query's parameters in order and
//!   answering the object's `u64` handle;
//! * two sync methods on that object, `refetch()` (`fnv1a32("query.refetch")`) and
//!   `invalidate()` (`fnv1a32("query.invalidate")`), the same ids on every handle type;
//!
//! and for each mutation an async free function whose method id is the mutation id
//! (`fnv1a32("mutation.add_todo")`), taking the input parameters in order and answering the
//! mutation's `Result<T, E>` (status 0 with the `T`, status 1 with the `E`).
//!
//! None of these ids is in the static dispatch table (they are generic instantiations, and
//! registering them as objects would put them in the schema twice), so this module is a
//! [`DispatchLayer`](keel_runtime::DispatchLayer): the runtime asks it when its own table has no
//! answer, and it finds the query or mutation through the registrations `#[keel::query]` and
//! `#[keel::mutation]` submit.

use core::any::Any;

use keel_meta::{DispatchCall, DispatchOutcome, ids};
use keel_runtime::inventory;
use keel_runtime::{DispatchLayer, DispatchResult, Runtime};
use keel_wire::Encode;

use crate::client::QueryClient;
use crate::erased::{MutationVTable, QueryVTable, registered_mutation, registered_query};
use crate::handle::{HandleObject, HandleObjectInner};
use crate::shared::shared_of;

/// The method id of `refetch()` on every query handle: `fnv1a32("query.refetch")`.
pub const REFETCH_METHOD_ID: u32 = ids::fnv1a32("query.refetch");

/// The method id of `invalidate()` on every query handle: `fnv1a32("query.invalidate")`.
pub const INVALIDATE_METHOD_ID: u32 = ids::fnv1a32("query.invalidate");

fn dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    DispatchOutcome::new(serve(rt, call))
}

inventory::submit! {
    DispatchLayer { name: "keel-query", dispatch }
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

/// `refetch()` and `invalidate()` on a handle.
fn handle_method(rt: &Runtime, call: DispatchCall<'_>) -> DispatchResult {
    let object = match rt.object::<HandleObjectInner>(call.handle) {
        Ok(object) => object,
        Err(e) => return DispatchResult::BadRequest(e.to_string()),
    };
    match call.method_id {
        REFETCH_METHOD_ID => object.ops.refetch(),
        INVALIDATE_METHOD_ID => object.ops.invalidate(),
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
