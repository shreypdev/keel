//! The traits `#[keel::query]` and `#[keel::mutation]` implement (SPEC 9).

use core::future::Future;
use core::pin::Pin;

use keel_wire::{Decode, Encode};

use crate::runtime::Ctx;

/// A boxed, sendable future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// The bounds every value crossing the query cache must satisfy.
pub trait CacheValue: Encode + Decode + Clone + Send + Sync + 'static {}
impl<T: Encode + Decode + Clone + Send + Sync + 'static> CacheValue for T {}

/// A `#[keel::query]` function.
pub trait QueryDef: 'static {
    /// `fnv1a32("query.<fn name>")`.
    const ID: u32;
    /// The cache key template, `{param}` placeholders included.
    const KEY: &'static str;
    /// The staleness window in milliseconds.
    const STALE_MS: Option<u64>;
    /// Whether results are persisted.
    const PERSIST: bool;
    /// Retry attempts.
    const RETRY: u32;
    /// The parameters (excluding `Ctx`) as a tuple.
    type Params: CacheValue;
    /// The success value.
    type Output: CacheValue;
    /// The error value.
    type Error: CacheValue;
    /// Runs the function.
    fn fetch(ctx: Ctx, params: Self::Params) -> BoxFuture<Result<Self::Output, Self::Error>>;
}

/// A `#[keel::mutation]` function.
pub trait MutationDef: 'static {
    /// `fnv1a32("mutation.<fn name>")`.
    const ID: u32;
    /// The invalidation key template.
    const KEY: &'static str;
    /// Retry attempts.
    const RETRY: u32;
    /// Whether the mutation is safe to replay.
    const IDEMPOTENT: bool;
    /// The parameters (excluding `Ctx`) as a tuple.
    type Input: CacheValue;
    /// The success value.
    type Output: CacheValue;
    /// The error value.
    type Error: CacheValue;
    /// Runs the function.
    fn execute(ctx: Ctx, input: Self::Input) -> BoxFuture<Result<Self::Output, Self::Error>>;
}
