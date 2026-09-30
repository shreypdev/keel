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
//!    remembered (per entry: what it looked like before, and the write stamp the closure left)
//!    so it can be undone.
//! 2. The mutation runs, up to `1 + RETRY` times with the standard backoff between attempts.
//! 3. On success the optimistic writes stay, and the mutation's own key (its `key = ".."`, rendered
//!    with the input) together with the `invalidates` targets are marked stale; the observed
//!    ones refetch.
//! 4. On failure the entries the closure wrote are restored, in one transaction, to exactly what
//!    they were before it wrote them, and the error is returned. Only the closure's own writes are
//!    undone: an entry something else has written since (a later optimistic mutation, a fetch
//!    result, a `set`) is left as it is (see [`MutationBuilder::optimistic`]). The exception is
//!    the offline queue (see the crate documentation): an idempotent mutation that failed for
//!    lack of a network while the client is offline is queued and keeps waiting, its optimistic
//!    writes staying visible; the awaited result is that of the replay.
//! 5. Dropping the future (a cancelled call) rolls the optimistic writes back too.
//!
//! # Concurrent optimistic mutations
//!
//! A rollback is the inverse of the failed mutation's own writes, not a restore of a snapshot of
//! the whole cache. Each entry has a **write stamp**: a number nobody has had before, taken anew
//! by every write of the entry (an optimistic write, `QueryClient::set`, a fetch result or
//! error). The entry also remembers, for each optimistic mutation that has written it and not
//! yet settled, the state before that mutation's first write and the stamp its last write left.
//! When a mutation fails:
//!
//! * if the entry still has the stamp the mutation left, nobody wrote it since, and it is
//!   restored (with its old stamp, so an earlier mutation's rollback can still recognise it);
//! * otherwise it is left as it is. The stamp is compared, never the bytes: a fetch that
//!   returned the very value the mutation wrote is still newer than the mutation.
//!
//! When the later write was made by another optimistic mutation, directly on top of the failed
//! one's result, the later mutation takes over the failed one's restore point. So with A then B
//! on one entry: if A fails first, B's value stays (and still contains A's change, which cannot
//! be taken out of a value the core only knows as bytes); if B then fails too the entry goes back
//! to before A; if B succeeds, what B invalidates is refetched, and the entry shows the
//! server's answer.

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
use crate::shared::Shared;

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

/// Which entries an optimistic update wrote, in the order it first wrote them. What each one
/// looked like before, and whether it is still as the update left it, is kept by the entry itself
/// (a [`Layer`](crate::shared::Layer) under the update's `owner` id).
pub(crate) struct UndoLog {
    owner: u64,
    seen: HashSet<QueryKey>,
    keys: Vec<QueryKey>,
}

impl UndoLog {
    /// An empty log for the mutation `owner` (an id from `Shared::new_stamp`).
    pub(crate) fn new(owner: u64) -> UndoLog {
        UndoLog {
            owner,
            seen: HashSet::new(),
            keys: Vec::new(),
        }
    }

    /// The mutation this log belongs to.
    pub(crate) fn owner(&self) -> u64 {
        self.owner
    }

    /// Remembers that `key` was written (only its first write is noted).
    pub(crate) fn touch(&mut self, key: &QueryKey) {
        if self.seen.insert(key.clone()) {
            self.keys.push(key.clone());
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub(crate) fn keys(&self) -> &[QueryKey] {
        &self.keys
    }

    pub(crate) fn into_keys(self) -> Vec<QueryKey> {
        self.keys
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
        if let Some(undo) = self.undo.take() {
            if !undo.is_empty() {
                self.shared.commit(&undo);
            }
        }
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
    /// once. It runs inside one transaction. Several calls run in order.
    ///
    /// # If the mutation fails
    ///
    /// What the update wrote is undone in one more transaction: each entry it wrote goes back
    /// to what it was before the update wrote it, and an entry it created is removed. The
    /// rollback is the inverse of **this update's own writes**, not a restore of the cache as it
    /// was when the mutation started, so a mutation that ran in between is not undone with it.
    /// Every entry has a write stamp that changes with each write of it; an entry is restored
    /// only if it still has the stamp this update left, and is otherwise **left as it is**:
    ///
    /// * A later optimistic mutation's write survives, on another entry and on the same entry:
    ///   a placeholder shown for an add stays until that add settles. On the same entry the
    ///   value that stays was built on top of this update's result, so it may still show this
    ///   update's change until the later mutation settles.
    /// * A fetch result, or `QueryClient::set`, that landed after this update's write is newer
    ///   than it and stays (even when it holds the very bytes the update wrote).
    /// * If the later write belongs to another optimistic mutation that wrote directly on top of
    ///   this one's result, it takes over this update's restore point: if it fails too, the
    ///   entry goes back to before both. If it succeeds, the entries it invalidates are
    ///   refetched, which is the last word on what the entry shows, so a mutation should
    ///   invalidate the entries it writes optimistically.
    ///
    /// Rolling back several entries is one transaction, and entries that are left alone are not
    /// published: a store whose entry stays sees no change-set.
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

    // 1. The optimistic update, in one transaction. The rollback guard exists before the update
    //    runs, so a closure that panics halfway still gets what it did undone.
    let mut rollback = Rollback {
        shared: shared.clone(),
        ctx: ctx.clone(),
        undo: Some(UndoLog::new(shared.new_stamp())),
    };
    if let Some(update) = optimistic {
        let now = shared.now(&ctx);
        ctx.txn(|| {
            if let Some(undo) = rollback.undo.as_mut() {
                let mut cache = CacheView {
                    ctx: &ctx,
                    shared: &shared,
                    undo,
                    now,
                };
                update(&mut cache);
            }
        });
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> QueryKey {
        QueryKey::new(1, Arc::from(vec![n]))
    }

    #[test]
    fn the_undo_log_keeps_each_key_once_in_the_order_of_its_first_write() {
        let mut log = UndoLog::new(7);
        assert!(log.is_empty());
        assert_eq!(log.owner(), 7);
        log.touch(&key(1));
        log.touch(&key(2));
        // A second write of a key is not a second entry.
        log.touch(&key(1));
        assert!(!log.is_empty());
        assert_eq!(log.keys(), [key(1), key(2)]);
        assert_eq!(log.into_keys(), [key(1), key(2)]);
    }
}
