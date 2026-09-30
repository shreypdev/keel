//! The type-erased side of queries and mutations.
//!
//! The cache stores encoded bytes (SPEC 9), and the platforms and the offline queue name
//! queries by id, so the client engine cannot be generic over `Q: QueryDef`. Each definition
//! contributes one [`QueryVTable`] / [`MutationVTable`]: a table of plain function pointers,
//! instantiated per definition, that decodes, runs, encodes and opens handles for it.
//!
//! `#[keel::query]` and `#[keel::mutation]` submit a [`QueryRegistration`] /
//! [`MutationRegistration`] through `inventory`, which is how a platform-issued constructor
//! or mutation call (which carries only an id) finds the definition, and how the offline queue
//! replays a mutation after a restart.

use core::any::Any;
use core::marker::PhantomData;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use keel_runtime::Ctx;
use keel_runtime::inventory;
use keel_wire::{Decode, Encode, WireError};

use crate::client::CtxQuery;
use crate::defs::{BoxFuture, CacheValue, MutationDef, QueryDef};
use crate::handle::HandleOps;

/// A wire value together with its encoding: the typed value serves observers without decoding
/// it again, the bytes serve equality, persistence and the change-sets.
#[derive(Clone)]
pub(crate) struct Erased {
    /// The value as encoded on the wire.
    pub(crate) bytes: Arc<[u8]>,
    /// The value itself, a `T: CacheValue`.
    pub(crate) value: Arc<dyn Any + Send + Sync>,
}

impl Erased {
    pub(crate) fn new<T: CacheValue>(value: T) -> Erased {
        Erased {
            bytes: Arc::from(value.encode_to_vec()),
            value: Arc::new(value),
        }
    }

    pub(crate) fn decode<T: CacheValue>(bytes: &[u8]) -> Result<Erased, WireError> {
        let value = T::decode_exact(bytes)?;
        Ok(Erased {
            bytes: Arc::from(bytes),
            value: Arc::new(value),
        })
    }

    /// The value as a `T`, if that is what it is.
    pub(crate) fn typed<T: Clone + 'static>(&self) -> Option<T> {
        self.value.downcast_ref::<T>().cloned()
    }
}

/// Why a fetch or an execution produced no value.
pub(crate) enum Failure {
    /// The function ran and returned its error.
    Error(Erased),
    /// It could not run at all (its arguments did not decode).
    Broken(String),
}

impl Failure {
    /// Whether trying again could change the outcome.
    pub(crate) fn retryable(&self) -> bool {
        matches!(self, Failure::Error(_))
    }
}

/// What an erased fetch or execution resolves to.
pub(crate) type Outcome = Result<Erased, Failure>;

/// Observes a query with encoded parameters and answers the handle.
pub(crate) type OpenFn = fn(&crate::QueryClient, &[u8]) -> Result<Box<dyn HandleOps>, WireError>;

/// The encoded result of a mutation dispatched from the platform: `Ok` body or typed error.
pub(crate) type EncodedResult = Result<Vec<u8>, Vec<u8>>;

/// The type-erased half of one `#[keel::query]`. Built with [`QueryRegistration::of`]; the
/// fields are the engine's business.
pub struct QueryVTable {
    pub(crate) id: u32,
    pub(crate) key: &'static str,
    pub(crate) stale_ms: Option<u64>,
    pub(crate) persist: bool,
    pub(crate) retry: u32,
    /// Runs the query with encoded parameters.
    pub(crate) fetch: fn(Ctx, &[u8]) -> BoxFuture<Outcome>,
    /// Decodes an encoded output (a persisted entry).
    pub(crate) decode_data: fn(&[u8]) -> Result<Erased, WireError>,
    /// Observes the query with encoded parameters: what a platform constructor call does.
    pub(crate) open: OpenFn,
}

/// The type-erased half of one `#[keel::mutation]`. Built with [`MutationRegistration::of`].
pub struct MutationVTable {
    pub(crate) id: u32,
    pub(crate) key: &'static str,
    pub(crate) retry: u32,
    /// Runs the mutation once, with encoded input (the offline queue's replay).
    pub(crate) execute: fn(Ctx, &[u8]) -> BoxFuture<Outcome>,
    /// Runs the whole mutation (`ctx.mutate`) with encoded input and encodes the result: what
    /// a platform call of the mutation function does.
    pub(crate) dispatch: fn(Ctx, &[u8]) -> Result<BoxFuture<EncodedResult>, WireError>,
}

struct QueryHolder<Q>(PhantomData<fn() -> Q>);

impl<Q: QueryDef> QueryHolder<Q> {
    const VTABLE: QueryVTable = QueryVTable {
        id: Q::ID,
        key: Q::KEY,
        stale_ms: Q::STALE_MS,
        persist: Q::PERSIST,
        retry: Q::RETRY,
        fetch: fetch_erased::<Q>,
        decode_data: Erased::decode::<Q::Output>,
        open: open_erased::<Q>,
    };
}

fn fetch_erased<Q: QueryDef>(ctx: Ctx, params: &[u8]) -> BoxFuture<Outcome> {
    match Q::Params::decode_exact(params) {
        Ok(params) => {
            let fetch = Q::fetch(ctx, params);
            Box::pin(async move {
                match fetch.await {
                    Ok(value) => Ok(Erased::new(value)),
                    Err(error) => Err(Failure::Error(Erased::new(error))),
                }
            })
        }
        Err(e) => {
            let message = format!("the parameters of query `{}` do not decode: {e}", Q::KEY);
            Box::pin(async move { Err(Failure::Broken(message)) })
        }
    }
}

fn open_erased<Q: QueryDef>(
    client: &crate::QueryClient,
    args: &[u8],
) -> Result<Box<dyn HandleOps>, WireError> {
    let params = Q::Params::decode_exact(args)?;
    Ok(Box::new(client.observe::<Q>(params)))
}

/// The table of query `Q`.
pub(crate) fn query_vtable<Q: QueryDef>() -> &'static QueryVTable {
    &QueryHolder::<Q>::VTABLE
}

struct MutationHolder<M>(PhantomData<fn() -> M>);

impl<M: MutationDef> MutationHolder<M> {
    const VTABLE: MutationVTable = MutationVTable {
        id: M::ID,
        key: M::KEY,
        retry: M::RETRY,
        execute: execute_erased::<M>,
        dispatch: dispatch_erased::<M>,
    };
}

fn execute_erased<M: MutationDef>(ctx: Ctx, input: &[u8]) -> BoxFuture<Outcome> {
    match M::Input::decode_exact(input) {
        Ok(input) => {
            let run = M::execute(ctx, input);
            Box::pin(async move {
                match run.await {
                    Ok(value) => Ok(Erased::new(value)),
                    Err(error) => Err(Failure::Error(Erased::new(error))),
                }
            })
        }
        Err(e) => {
            let message = format!("the input of mutation `{}` does not decode: {e}", M::KEY);
            Box::pin(async move { Err(Failure::Broken(message)) })
        }
    }
}

fn dispatch_erased<M: MutationDef>(
    ctx: Ctx,
    input: &[u8],
) -> Result<BoxFuture<EncodedResult>, WireError> {
    let input = M::Input::decode_exact(input)?;
    let mutation = ctx.mutate::<M>(input);
    Ok(Box::pin(async move {
        match mutation.await {
            Ok(value) => Ok(value.encode_to_vec()),
            Err(error) => Err(error.encode_to_vec()),
        }
    }))
}

/// The table of mutation `M`.
pub(crate) fn mutation_vtable<M: MutationDef>() -> &'static MutationVTable {
    &MutationHolder::<M>::VTABLE
}

/// One `#[keel::query]`, submitted through `inventory` by the macro so the client can find the
/// query by id (a platform constructs a handle knowing only the id).
///
/// Generated code contains
/// `inventory::submit! { keel::query::QueryRegistration::of::<TodosQuery>() }`; write it by hand
/// only for a [`QueryDef`] the macro did not generate.
pub struct QueryRegistration {
    vtable: &'static QueryVTable,
}

impl QueryRegistration {
    /// The registration of query `Q`.
    pub const fn of<Q: QueryDef>() -> QueryRegistration {
        QueryRegistration {
            vtable: &QueryHolder::<Q>::VTABLE,
        }
    }
}

inventory::collect!(QueryRegistration);

/// One `#[keel::mutation]`, submitted through `inventory` by the macro so a platform call and
/// the offline queue's replay can find the mutation by id.
pub struct MutationRegistration {
    vtable: &'static MutationVTable,
}

impl MutationRegistration {
    /// The registration of mutation `M`.
    pub const fn of<M: MutationDef>() -> MutationRegistration {
        MutationRegistration {
            vtable: &MutationHolder::<M>::VTABLE,
        }
    }
}

inventory::collect!(MutationRegistration);

/// The registered query with id `id`.
pub(crate) fn registered_query(id: u32) -> Option<&'static QueryVTable> {
    static TABLE: OnceLock<HashMap<u32, &'static QueryVTable>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            let mut table = HashMap::new();
            for registration in inventory::iter::<QueryRegistration> {
                table
                    .entry(registration.vtable.id)
                    .or_insert(registration.vtable);
            }
            table
        })
        .get(&id)
        .copied()
}

/// The registered mutation with id `id`.
pub(crate) fn registered_mutation(id: u32) -> Option<&'static MutationVTable> {
    static TABLE: OnceLock<HashMap<u32, &'static MutationVTable>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            let mut table = HashMap::new();
            for registration in inventory::iter::<MutationRegistration> {
                table
                    .entry(registration.vtable.id)
                    .or_insert(registration.vtable);
            }
            table
        })
        .get(&id)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erased_keeps_the_bytes_and_the_typed_value_together() {
        let e = Erased::new(vec![1_u8, 2, 3]);
        assert_eq!(&*e.bytes, &vec![1_u8, 2, 3].encode_to_vec()[..]);
        assert_eq!(e.typed::<Vec<u8>>(), Some(vec![1, 2, 3]));
        assert_eq!(
            e.typed::<String>(),
            None,
            "the wrong type is None, not a panic"
        );
        let d = Erased::decode::<Vec<u8>>(&e.bytes).unwrap();
        assert_eq!(d.typed::<Vec<u8>>(), Some(vec![1, 2, 3]));
        assert!(Erased::decode::<Vec<u8>>(&[9]).is_err());
    }

    #[test]
    fn only_a_function_failure_is_worth_retrying() {
        assert!(Failure::Error(Erased::new(1_u8)).retryable());
        assert!(!Failure::Broken("x".into()).retryable());
    }
}
