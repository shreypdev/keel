//! Dispatch (SPEC 5.6): what a generated dispatcher returns and how the runtime finds one.
//!
//! `keel-meta` stores each object's and function's dispatcher as
//! `fn(&dyn Any, DispatchCall<'_>) -> DispatchOutcome`. The runtime passes itself as
//! `&dyn Any` (the dispatcher downcasts it to [`Runtime`](crate::Runtime)) and unwraps the
//! [`DispatchOutcome`](keel_meta::DispatchOutcome) into a [`DispatchResult`].
//!
//! # What a dispatcher does
//!
//! 1. Resolves the receiver: `rt.object::<T>(Handle(call.handle))` (a stale or wrongly typed
//!    handle is [`DispatchResult::BadRequest`], status 5).
//! 2. Decodes `call.args` with `keel_wire` (a decode failure is
//!    [`DispatchResult::BadRequest`] too), and answers [`DispatchResult::Unknown`] for a
//!    `method_id` it does not implement.
//! 3. Runs the method and encodes the result: `Ok`/`Err` bytes for
//!    [`Sync`](DispatchResult::Sync) and [`Async`](DispatchResult::Async), items for
//!    [`Stream`](DispatchResult::Stream).
//! 4. For a constructor, inserts the new object with
//!    [`Runtime::insert_object`](crate::Runtime::insert_object) /
//!    [`insert_store`](crate::Runtime::insert_store) and encodes the returned handle (`u64`)
//!    as the `Ok` body.
//!
//! The dispatcher runs with the core lock held. It must not block; anything slow returns
//! `Async`.

use core::fmt;
use core::future::Future;
use core::pin::Pin;
use std::collections::HashMap;

use keel_meta::{DispatchFn, FunctionMeta, ObjectMeta, Registration, TypeRefMeta};

/// Encoded bytes of a successful (`Ok`) or typed-error (`Err`) result.
pub type DispatchBytes = Result<Vec<u8>, Vec<u8>>;

/// What a generated dispatcher returns (boxed into a
/// [`DispatchOutcome`](keel_meta::DispatchOutcome)).
pub enum DispatchResult {
    /// A synchronous method finished: `Ok` is status 0, `Err` is status 1.
    Sync(DispatchBytes),
    /// An `async` method: the runtime spawns it as a task keyed by `call_id`.
    Async(Pin<Box<dyn Future<Output = DispatchBytes> + Send>>),
    /// A method returning a stream: the runtime replies status 4, then drives the stream
    /// under credit flow control. `Err` items end the stream (flag 2).
    Stream(Pin<Box<dyn futures_core::Stream<Item = DispatchBytes> + Send>>),
    /// The dispatcher does not implement this `method_id` (status 5).
    Unknown,
    /// The request could not be served: undecodable arguments, stale handle, wrong receiver
    /// type (status 5 with this reason). *Addition to SPEC 16.2, see the crate docs.*
    BadRequest(String),
}

impl fmt::Debug for DispatchResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DispatchResult::Sync(r) => f.debug_tuple("Sync").field(r).finish(),
            DispatchResult::Async(_) => f.write_str("Async(..)"),
            DispatchResult::Stream(_) => f.write_str("Stream(..)"),
            DispatchResult::Unknown => f.write_str("Unknown"),
            DispatchResult::BadRequest(reason) => {
                f.debug_tuple("BadRequest").field(reason).finish()
            }
        }
    }
}

/// A dispatcher for calls that no [`Registration`] claims: how a crate layered on the runtime
/// (`keel-query`) serves ids that exist only as generic instantiations. *Addition to SPEC
/// 16.2, see the crate docs.*
///
/// The static table built from `keel_meta::registrations()` is consulted first. A call that
/// misses it (an unknown function id, an unknown constructor type, or a method on an object
/// whose type has no registered dispatcher) is offered to every layer in turn, with the same
/// arguments a generated dispatcher gets: the runtime as `&dyn Any`, and a
/// [`DispatchCall`](keel_meta::DispatchCall) whose `handle` is `0` for a function or a
/// constructor. A layer answers [`DispatchResult::Unknown`] for ids it does not serve; the
/// first other answer wins. Layers are consulted in link order, so two layers must not claim
/// the same id.
///
/// ```ignore
/// inventory::submit! {
///     keel_runtime::DispatchLayer { name: "keel-query", dispatch: keel_query::dispatch }
/// }
/// ```
pub struct DispatchLayer {
    /// Shown in logs.
    pub name: &'static str,
    /// The dispatcher: downcast the `&dyn Any` to [`Runtime`](crate::Runtime), decode
    /// `call.args`, answer with `DispatchOutcome::new(DispatchResult::..)`.
    pub dispatch: DispatchFn,
}

inventory::collect!(DispatchLayer);

/// The dispatchers registered with `keel-meta`, indexed for lookup by id.
#[derive(Default)]
pub(crate) struct DispatchTable {
    pub(crate) functions: HashMap<u32, &'static FunctionMeta>,
    pub(crate) objects: HashMap<u32, &'static ObjectMeta>,
    /// The layers that serve what the two maps above miss, in registration order.
    pub(crate) layers: Vec<&'static DispatchLayer>,
    /// Ids that more than one registration claimed (first wins); reported at init.
    pub(crate) collisions: Vec<(u32, &'static str, &'static str)>,
}

impl DispatchTable {
    /// Builds the table from every registration linked into the process.
    pub(crate) fn collect() -> DispatchTable {
        let mut table = DispatchTable::default();
        for registration in keel_meta::registrations() {
            match registration {
                Registration::Function(meta) => {
                    if let Some(first) = table.functions.get(&meta.method_id) {
                        table
                            .collisions
                            .push((meta.method_id, first.name, meta.name));
                    } else {
                        table.functions.insert(meta.method_id, meta);
                    }
                }
                Registration::Object(meta) => {
                    if let Some(first) = table.objects.get(&meta.type_id) {
                        table.collisions.push((meta.type_id, first.name, meta.name));
                    } else {
                        table.objects.insert(meta.type_id, meta);
                    }
                }
                _ => {}
            }
        }
        table.layers.extend(inventory::iter::<DispatchLayer>);
        table
    }
}

/// Whether a method's shape means it cannot be served by `call_sync`.
pub(crate) fn needs_async(is_async: bool, returns: &TypeRefMeta) -> bool {
    is_async
        || match returns {
            TypeRefMeta::Stream(_) => true,
            TypeRefMeta::Result(ok, _) => matches!(ok, TypeRefMeta::Stream(_)),
            _ => false,
        }
}

/// The reason text for a call made from inside a host callback.
pub(crate) const E_REENTRANT: &str = "E_REENTRANT: the runtime was called from inside one of its own host callbacks on the thread that holds the core lock; hand the call to another thread";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_shapes_are_detected_from_metadata() {
        assert!(needs_async(true, &TypeRefMeta::Unit));
        assert!(needs_async(false, &TypeRefMeta::Stream(&TypeRefMeta::I32)));
        assert!(needs_async(
            false,
            &TypeRefMeta::Result(
                &TypeRefMeta::Stream(&TypeRefMeta::I32),
                &TypeRefMeta::String
            )
        ));
        assert!(!needs_async(false, &TypeRefMeta::I32));
        assert!(!needs_async(
            false,
            &TypeRefMeta::Result(&TypeRefMeta::I32, &TypeRefMeta::String)
        ));
    }

    #[test]
    fn dispatch_result_debug_does_not_need_debug_futures() {
        assert_eq!(format!("{:?}", DispatchResult::Unknown), "Unknown");
        assert_eq!(
            format!("{:?}", DispatchResult::Sync(Ok(vec![1]))),
            "Sync(Ok([1]))"
        );
        let fut: Pin<Box<dyn Future<Output = DispatchBytes> + Send>> =
            Box::pin(async { Ok(Vec::new()) });
        assert_eq!(format!("{:?}", DispatchResult::Async(fut)), "Async(..)");
    }
}
