//! Mutations: `ctx.mutate(M, input)` with optimistic cache updates, rollback, invalidation,
//! retry and the offline queue (SPEC 5.3 and 9).
//!
//! ```ignore
//! let added = ctx
//!     .mutate::<AddTodoMutation>((title,))
//!     .optimistic(|cache| {
//!         cache.update::<TodosQuery>((0,), |page| page.items.push(placeholder));
//!     })
//!     .invalidates(["todos"])
//!     .await?;
//! ```
//!
//! # What happens, in order
//!
//! 1. The `optimistic` closure runs inside one transaction with a [`CacheView`]. Whatever it
//!    changes is visible to observers immediately, in one change-set per store, and is
//!    remembered so it can be undone.
//! 2. The mutation runs, up to `1 + RETRY` times with the standard backoff between attempts.
//! 3. On success the optimistic writes stay, and the mutation's own key (its `key = ".."`, rendered
//!    with the input) together with the `invalidates` targets are marked stale; the observed
//!    ones refetch.
//! 4. On failure every entry the closure touched is restored to exactly what it was, in one
//!    transaction, and the error is returned. The exception is the offline queue: an idempotent
//!    mutation that failed for lack of a network while the client is offline is queued and
//!    keeps waiting (see the [`queue`](crate::idempotency_key) notes), its optimistic writes
//!    staying visible; the awaited result is that of the replay.
//! 5. Dropping the future (a cancelled call) rolls the optimistic writes back too.

use core::future::IntoFuture;
use std::collections::HashSet;
use std::sync::Arc;

use keel_runtime::Ctx;
use keel_wire::Encode;

use crate::defs::{BoxFuture, MutationDef, QueryDef};
use crate::erased::{Erased, Failure, mutation_vtable, query_vtable};
use crate::key::{Invalidate, QueryKey};
use crate::queue::{is_network_error, scoped};
use crate::retry::{new_uuid, with_retries};
use crate::shared::{EntrySnapshot, Shared};

/// Typed access to the query cache inside an optimistic update (SPEC 9). Every change is applied
/// inside the mutation's transaction and recorded so a failed mutation can undo it.
///
/// The parameters are those of the query (`Q::Params`, a tuple), the value its output. A `set`
/// or `update` marks the entry as freshly updated and cancels a fetch of it that is still
/// running, whose answer would otherwise overwrite the write with older data.
pub struct CacheView<'a> {
    ctx: &'a Ctx,
    shared: &'a Arc<Shared>,
    undo: &'a mut UndoLog,
    now: i64,
}

impl CacheView<'_> {
    /// The cached data of query `Q` with these parameters, if any.
    pub fn get<Q: QueryDef>(&self, params: Q::Params) -> Option<Q::Output> {
        let key = QueryKey::new(Q::ID, Arc::from(params.encode_to_vec()));
        self.shared.read(&key)?.typed::<Q::Output>()
    }

    /// Replaces the data of query `Q` with these parameters, creating the entry if needed.
    pub fn set<Q: QueryDef>(&mut self, params: Q::Params, value: Q::Output) {
        let params: Arc<[u8]> = Arc::from(params.encode_to_vec());
        self.shared.write(
            self.ctx,
            query_vtable::<Q>(),
            params,
            Erased::new(value),
            self.now,
            Some(&mut *self.undo),
        );
    }

    /// Changes the cached data of query `Q` with these parameters in place. Returns `false`
    /// (and does nothing) if there is no data to change.
    ///
    /// ```ignore
    /// cache.update::<TodosQuery>((0,), |page| page.items.push(todo));
    /// ```
    pub fn update<Q: QueryDef>(
        &mut self,
        params: Q::Params,
        f: impl FnOnce(&mut Q::Output),
    ) -> bool {
        let bytes: Arc<[u8]> = Arc::from(params.encode_to_vec());
        let key = QueryKey::new(Q::ID, bytes.clone());
        let Some(mut value) = self.shared.read(&key).and_then(|e| e.typed::<Q::Output>()) else {
            return false;
        };
        f(&mut value);
        self.shared.write(
            self.ctx,
            query_vtable::<Q>(),
            bytes,
            Erased::new(value),
            self.now,
            Some(&mut *self.undo),
        );
        true
    }
}

/// What an optimistic update changed, in the order it changed it.
#[derive(Default)]
pub(crate) struct UndoLog {
    seen: HashSet<QueryKey>,
    log: Vec<(QueryKey, Option<EntrySnapshot>)>,
}

impl UndoLog {
    /// Remembers what `key` looked like before its first change (`None`: it did not exist).
    pub(crate) fn record(&mut self, key: &QueryKey, before: Option<EntrySnapshot>) {
        if self.seen.insert(key.clone()) {
            self.log.push((key.clone(), before));
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.log.is_empty()
    }

    pub(crate) fn into_entries(self) -> Vec<(QueryKey, Option<EntrySnapshot>)> {
        self.log
    }
}

/// Undoes an optimistic update unless it is disarmed, including when the mutation's future is
/// dropped before it finishes.
struct Rollback {
    shared: Arc<Shared>,
    ctx: Ctx,
    undo: Option<UndoLog>,
}

impl Rollback {
    /// The optimistic writes are final.
    fn disarm(&mut self) {
        self.undo = None;
    }

    /// Restores every touched entry now, in one transaction.
    fn rollback_now(&mut self) {
        if let Some(undo) = self.undo.take() {
            if !undo.is_empty() {
                self.shared.rollback(&self.ctx, undo);
            }
        }
    }
}

impl Drop for Rollback {
    fn drop(&mut self) {
        if !self.ctx.runtime().is_shut_down() {
            self.rollback_now();
        }
    }
}

type Optimistic = Box<dyn FnOnce(&mut CacheView<'_>) + Send>;

/// A mutation being prepared: add an [`optimistic`](MutationBuilder::optimistic) update and
/// [`invalidates`](MutationBuilder::invalidates) targets, then `.await` it. Made by
/// [`ctx.mutate`](crate::CtxQuery::mutate).
#[must_use = "a mutation does nothing until it is awaited"]
pub struct MutationBuilder<M: MutationDef> {
    ctx: Ctx,
    shared: Arc<Shared>,
    input: M::Input,
    optimistic: Option<Optimistic>,
    invalidations: Vec<Invalidate>,
}

impl<M: MutationDef> MutationBuilder<M> {
    pub(crate) fn new(ctx: Ctx, shared: Arc<Shared>, input: M::Input) -> MutationBuilder<M> {
        MutationBuilder {
            ctx,
            shared,
            input,
            optimistic: None,
            invalidations: Vec::new(),
        }
    }

    /// Applies `update` to the cache before the mutation runs, so the UI shows the result at
    /// once. It runs inside one transaction; if the mutation fails everything it changed is
    /// restored in another. Several calls run in order.
    pub fn optimistic(
        mut self,
        update: impl FnOnce(&mut CacheView<'_>) + Send + 'static,
    ) -> MutationBuilder<M> {
        let previous = self.optimistic.take();
        self.optimistic = Some(Box::new(move |cache| {
            if let Some(previous) = previous {
                previous(cache);
            }
            update(cache);
        }));
        self
    }

    /// Marks these entries stale when the mutation succeeds (on top of the mutation's own
    /// key); the observed ones refetch. Takes prefixes (`["todos"]`) or [`Invalidate`]s.
    pub fn invalidates<I, T>(mut self, targets: I) -> MutationBuilder<M>
    where
        I: IntoIterator<Item = T>,
        T: Into<Invalidate>,
    {
        self.invalidations
            .extend(targets.into_iter().map(Into::into));
        self
    }
}

impl<M: MutationDef> IntoFuture for MutationBuilder<M> {
    type Output = Result<M::Output, M::Error>;
    type IntoFuture = BoxFuture<Result<M::Output, M::Error>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(run::<M>(self))
    }
}

async fn run<M: MutationDef>(builder: MutationBuilder<M>) -> Result<M::Output, M::Error> {
    let MutationBuilder {
        ctx,
        shared,
        input,
        optimistic,
        invalidations,
    } = builder;

    // 1. The optimistic update, in one transaction.
    let mut undo = UndoLog::default();
    if let Some(update) = optimistic {
        let now = shared.now(&ctx);
        ctx.txn(|| {
            let mut cache = CacheView {
                ctx: &ctx,
                shared: &shared,
                undo: &mut undo,
                now,
            };
            update(&mut cache);
        });
    }
    let mut rollback = Rollback {
        shared: shared.clone(),
        ctx: ctx.clone(),
        undo: Some(undo),
    };

    // 2. Run it, with retries. An idempotent mutation carries one key for all its runs.
    let input_bytes = input.encode_to_vec();
    let key = M::IDEMPOTENT.then(|| new_uuid(&ctx));
    let result = with_retries(
        &ctx,
        M::RETRY,
        |_: &M::Error| true,
        || scoped(key, M::execute(ctx.clone(), input.clone())),
    )
    .await;

    match result {
        // 3. Success: the optimistic writes stay; what the mutation touched is stale.
        Ok(output) => {
            rollback.disarm();
            let mut targets = invalidations;
            let prefix = shared.render_key(&ctx, M::ID, M::KEY, &input_bytes);
            if !prefix.is_empty() {
                targets.push(Invalidate::Prefix(prefix));
            }
            shared.invalidate(&ctx, &targets);
            Ok(output)
        }
        Err(error) => {
            // 4. Offline: an idempotent mutation waits for the network.
            if let Some(key) = key {
                if !shared.is_online() && is_network_error(&ctx, M::ID, &Erased::new(error.clone()))
                {
                    let waiter = shared.enqueue(
                        &ctx,
                        mutation_vtable::<M>(),
                        input_bytes,
                        key,
                        invalidations,
                    );
                    match waiter.wait().await {
                        Ok(value) => {
                            if let Some(output) = value.typed::<M::Output>() {
                                rollback.disarm();
                                return Ok(output);
                            }
                        }
                        Err(Failure::Error(replayed)) => {
                            if let Some(replayed) = replayed.typed::<M::Error>() {
                                rollback.rollback_now();
                                return Err(replayed);
                            }
                        }
                        Err(Failure::Broken(_)) => {}
                    }
                }
            }
            // Failure: put the cache back exactly as it was.
            rollback.rollback_now();
            Err(error)
        }
    }
}
