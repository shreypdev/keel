//! Dispatch (SPEC 5.6): what a generated dispatcher returns and how the runtime finds one.
//!
//! `undra-meta` stores each object's and function's dispatcher as
//! `fn(&dyn Any, DispatchCall<'_>) -> DispatchOutcome`. The runtime passes itself as
//! `&dyn Any` (the dispatcher downcasts it to [`Runtime`](crate::Runtime)) and unwraps the
//! [`DispatchOutcome`](undra_meta::DispatchOutcome) into a [`DispatchResult`].
//!
//! # What a dispatcher does
//!
//! 1. Resolves the receiver: `rt.object::<T>(Handle(call.handle))` (a stale or wrongly typed
//!    handle is [`DispatchResult::BadRequest`], status 5).
//! 2. Decodes `call.args` with `undra_wire` (a decode failure is
//!    [`DispatchResult::BadRequest`] too), and answers [`DispatchResult::Unknown`] for a
//!    `method_id` it does not implement.
//! 3. Runs the method and encodes the result: for a synchronous method the answer is
//!    [`Runtime::sync_ok`](crate::Runtime::sync_ok) / [`sync_err`](crate::Runtime::sync_err)
//!    (written into the caller's reply buffer under `call_sync`, a
//!    [`Sync`](DispatchResult::Sync) result anywhere else; ADR-028), `Ok`/`Err` bytes for
//!    [`Async`](DispatchResult::Async), items for [`Stream`](DispatchResult::Stream).
//! 4. For a constructor, inserts the new object with
//!    [`Runtime::insert_object`](crate::Runtime::insert_object) /
//!    [`insert_store`](crate::Runtime::insert_store) and encodes the returned handle (`u64`)
//!    as the `Ok` body.
//!
//! The dispatcher runs with the core lock held. It must not block; anything slow returns
//! `Async`.

use core::fmt;
use core::future::Future;
use core::hash::{BuildHasherDefault, Hasher};
use core::pin::Pin;
use std::collections::HashMap;

use undra_meta::{DispatchFn, FunctionMeta, ObjectMeta, Registration, TypeRefMeta};

/// Encoded bytes of a successful (`Ok`) or typed-error (`Err`) result.
pub type DispatchBytes = Result<Vec<u8>, Vec<u8>>;

/// What a generated dispatcher returns (boxed into a
/// [`DispatchOutcome`](undra_meta::DispatchOutcome)).
pub enum DispatchResult {
    /// A synchronous method finished: `Ok` is status 0, `Err` is status 1.
    ///
    /// What a hand-written dispatcher or a [`DispatchLayer`] returns, and what
    /// [`Runtime::sync_ok`](crate::Runtime::sync_ok) builds when no reply slot is armed;
    /// generated dispatchers call those two instead of building this (ADR-028).
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
    /// The call took what it was handed and then failed inside the core, without unwinding: status
    /// 2 with this message, contained and reported like a panic (without a backtrace). Unlike [`BadRequest`](DispatchResult::BadRequest) it transfers ownership:
    /// a refused call owns nothing and the host gives its callback references back, a failed call
    /// has made its proxies, which the core releases when it drops them (a constructor that took
    /// callbacks and could not publish what it built).
    Failed(String),
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
            DispatchResult::Failed(reason) => f.debug_tuple("Failed").field(reason).finish(),
        }
    }
}

/// A dispatcher for calls that no [`Registration`] claims: how a crate layered on the runtime
/// (`undra-query`) serves ids that exist only as generic instantiations. *Addition to SPEC
/// 16.2, see the crate docs.*
///
/// The static table built from `undra_meta::registrations()` is consulted first. A call that
/// misses it (an unknown function id, an unknown constructor type, or a method on an object
/// whose type has no registered dispatcher) is offered to every layer in turn, with the same
/// arguments a generated dispatcher gets: the runtime as `&dyn Any`, and a
/// [`DispatchCall`](undra_meta::DispatchCall) whose `handle` is `0` for a function or a
/// constructor. A layer answers [`DispatchResult::Unknown`] for ids it does not serve; the
/// first other answer wins. Layers are consulted in link order, so two layers must not claim
/// the same id.
///
/// The name identifies the layer: a layer submitted more than once under one name is consulted
/// once (the first submission linked). That lets the code that needs a layer submit it, rather
/// than the crate that implements it: every `#[undra::query]` and `#[undra::mutation]` submits
/// `undra-query`'s, so a core with no queries does not link the query runtime (ADR-052).
///
/// ```ignore
/// inventory::submit! {
///     undra_runtime::DispatchLayer { name: "undra-query", dispatch: undra_query::dispatch }
/// }
/// ```
pub struct DispatchLayer {
    /// Shown in logs, and the layer's identity: one layer per name is consulted.
    pub name: &'static str,
    /// The dispatcher: downcast the `&dyn Any` to [`Runtime`](crate::Runtime), decode
    /// `call.args`, answer with `DispatchOutcome::new(DispatchResult::..)`.
    pub dispatch: DispatchFn,
}

inventory::collect!(DispatchLayer);

/// Hashes the `u32` ids of the dispatch table: they are already the output of a hash
/// (`fnv1a32` of a name), so one multiplication to spread them over the high bits `HashMap`
/// reads is all the hashing they need. The default `SipHash` costs more than the rest of a
/// lookup, on the path of every call.
#[derive(Default)]
pub(crate) struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // Only `u32` keys are used; fold anything else in without pretending to be good at it.
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }

    fn write_u32(&mut self, id: u32) {
        self.0 = u64::from(id).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// A map keyed by a dispatch id.
pub(crate) type IdMap<V> = HashMap<u32, V, BuildHasherDefault<IdHasher>>;

/// A registered object type and what `call_sync` needs to know about its methods without
/// scanning them on every call.
pub(crate) struct ObjectEntry {
    /// The registration.
    pub(crate) meta: &'static ObjectMeta,
    /// The ids of the methods that cannot be served by `call_sync` (`async`, or returning a
    /// stream); usually none or a few.
    async_methods: Box<[u32]>,
    /// The same for constructors.
    async_constructors: Box<[u32]>,
}

impl ObjectEntry {
    fn new(meta: &'static ObjectMeta) -> ObjectEntry {
        let async_ids = |methods: &'static [undra_meta::MethodMeta]| -> Box<[u32]> {
            methods
                .iter()
                .filter(|m| needs_async(m.is_async, &m.returns))
                .map(|m| m.method_id)
                .collect()
        };
        ObjectEntry {
            meta,
            async_methods: async_ids(meta.methods),
            async_constructors: async_ids(meta.constructors),
        }
    }

    /// Whether method `method_id` cannot be served synchronously (by its metadata).
    pub(crate) fn method_needs_async(&self, method_id: u32) -> bool {
        self.async_methods.contains(&method_id)
    }

    /// Whether constructor `method_id` cannot be served synchronously.
    pub(crate) fn constructor_needs_async(&self, method_id: u32) -> bool {
        self.async_constructors.contains(&method_id)
    }

    /// The name of the method or constructor `method_id`, for the "is asynchronous" reason.
    pub(crate) fn name_of(&self, method_id: u32, constructor: bool) -> &'static str {
        let list = if constructor {
            self.meta.constructors
        } else {
            self.meta.methods
        };
        list.iter()
            .find(|m| m.method_id == method_id)
            .map_or("?", |m| m.name)
    }
}

/// The dispatchers registered with `undra-meta`, indexed for lookup by id.
#[derive(Default)]
pub(crate) struct DispatchTable {
    pub(crate) functions: IdMap<&'static FunctionMeta>,
    pub(crate) objects: IdMap<ObjectEntry>,
    /// The layers that serve what the two maps above miss, in registration order, one per name.
    pub(crate) layers: Vec<&'static DispatchLayer>,
    /// Ids that more than one registration claimed (first wins); reported at init.
    pub(crate) collisions: Vec<(u32, &'static str, &'static str)>,
}

impl DispatchTable {
    /// Builds the table from every registration linked into the process.
    pub(crate) fn collect() -> DispatchTable {
        let mut table = DispatchTable::default();
        for registration in undra_meta::registrations() {
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
                        table
                            .collisions
                            .push((meta.type_id, first.meta.name, meta.name));
                    } else {
                        table.objects.insert(meta.type_id, ObjectEntry::new(meta));
                    }
                }
                _ => {}
            }
        }
        for layer in inventory::iter::<DispatchLayer> {
            if !table.layers.iter().any(|known| known.name == layer.name) {
                table.layers.push(layer);
            }
        }
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
