//! [`QueryClient`] and [`CtxQuery`]: how core code reaches the query cache.

use core::time::Duration;
use std::sync::Arc;

use undra_runtime::Ctx;
use undra_wire::{Encode, Uuid};

use crate::defs::{InfiniteQueryDef, MutationDef, QueryDef};
use crate::erased::{Erased, query_vtable};
use crate::handle::QueryHandle;
use crate::key::{Invalidate, QueryKey};
use crate::mutation::MutationBuilder;
use crate::paged::InfiniteHandle;
use crate::queue::{DeadLetter, RetryError};
use crate::shared::{Shared, shared_of};

/// The query cache of one runtime, bound to a [`Ctx`]. Get one with
/// [`ctx.query()`](CtxQuery::query); it is cheap to make and to clone, and every client of a
/// runtime shares the same cache.
///
/// ```
/// # use undra_query::{BoxFuture, CtxQuery, QueryDef};
/// # use undra_runtime::Ctx;
/// # struct Count;
/// # impl QueryDef for Count {
/// #     const ID: u32 = 1; const KEY: &'static str = "count"; const STALE_MS: Option<u64> = None;
/// #     const PERSIST: bool = false; const RETRY: u32 = 0;
/// #     type Params = (); type Output = u32; type Error = String;
/// #     fn fetch(_: Ctx, _: ()) -> BoxFuture<Result<u32, String>> { Box::pin(async { Ok(3) }) }
/// # }
/// # let t = undra_runtime::testing::TestRuntime::new();
/// let query = t.ctx().query();
/// query.set::<Count>((), 41);              // seed the cache
/// assert_eq!(query.get::<Count>(()), Some(41));
/// query.invalidate("count");               // marks it stale (nothing observes it)
/// ```
#[derive(Clone)]
pub struct QueryClient {
    ctx: Ctx,
    shared: Arc<Shared>,
}

impl QueryClient {
    pub(crate) fn bound(ctx: Ctx, shared: Arc<Shared>) -> QueryClient {
        QueryClient { ctx, shared }
    }

    /// Observes query `Q` with `params`: registers an observer and fetches if the cached data is
    /// stale or missing. The returned handle keeps observing until dropped.
    pub fn observe<Q: QueryDef>(&self, params: Q::Params) -> QueryHandle<Q> {
        QueryHandle::open(&self.ctx, &self.shared, &params)
    }

    /// Observes the infinite query `Q` with `params` (ADR-043): registers an observer, fetches the
    /// first page if the entry is stale or missing, and shows the rows loaded so far as one keyed
    /// list. The returned handle keeps observing until dropped. See [`InfiniteHandle`].
    pub fn infinite<Q: InfiniteQueryDef>(&self, params: Q::Params) -> InfiniteHandle<Q> {
        InfiniteHandle::open(&self.ctx, &self.shared, &params)
    }

    /// The cached data of `Q` with `params`, if any. Does not fetch and does not observe. For an
    /// infinite query the data is the list of every row loaded.
    pub fn get<Q: QueryDef>(&self, params: Q::Params) -> Option<Q::Output> {
        let key = QueryKey::new(Q::ID, Arc::from(params.encode_to_vec()));
        self.shared.read(&key)?.typed::<Q::Output>()
    }

    /// Writes `value` as the data of `Q` with `params`, as if a fetch had returned it (it is
    /// persisted if the query is). Observers see it at once; a fetch in flight is cancelled.
    pub fn set<Q: QueryDef>(&self, params: Q::Params, value: Q::Output) {
        let now = self.shared.now(&self.ctx);
        self.shared.write(
            &self.ctx,
            query_vtable::<Q>(),
            Arc::from(params.encode_to_vec()),
            Erased::new(value),
            now,
            None,
        );
    }

    /// Marks the matching entries stale and refetches the observed ones (SPEC 9's
    /// `invalidate(prefix)`). Takes a key prefix (`"todos"`) or an [`Invalidate`].
    pub fn invalidate(&self, target: impl Into<Invalidate>) {
        self.shared.invalidate(&self.ctx, &[target.into()]);
    }

    /// Reads the persisted entries, the offline queue and its dead letters from the `Kv` port,
    /// migrating what an older build wrote (ADR-037), and replays the queue if the client is
    /// online. The runtime does this once at start-up (and reads a queue that could not be read
    /// again on `Active`, on a background run and after a backoff); call it yourself to read the
    /// store again, after restoring a backup say, or in a test that installed its fakes late.
    pub async fn hydrate(&self) {
        self.shared.hydrate(&self.ctx.downgrade()).await;
    }

    /// How long an entry nobody observes stays cached (default 5 minutes). Applies to entries
    /// that become unobserved from now on.
    pub fn set_gc_time(&self, gc_time: Duration) {
        let ms = u64::try_from(gc_time.as_millis()).unwrap_or(u64::MAX);
        self.shared
            .gc_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the client believes the device can reach the network. It starts `true` and
    /// follows the platform's `Connectivity` events.
    pub fn is_online(&self) -> bool {
        self.shared.is_online()
    }

    /// How many mutations wait in the offline queue.
    pub fn pending_mutations(&self) -> usize {
        self.shared.queue_len()
    }

    /// How many entries the cache holds, observed or not.
    pub fn cached_entries(&self) -> usize {
        self.shared.state.lock().entries.len()
    }

    /// How many cache entries are kept in the `Kv` store at most (default
    /// [`DEFAULT_MAX_PERSISTED_ENTRIES`](crate::DEFAULT_MAX_PERSISTED_ENTRIES)); the least
    /// recently updated are deleted beyond it, when a new entry is written and at hydration
    /// (ADR-037 decision 8).
    pub fn set_max_persisted_entries(&self, max: usize) {
        self.shared.state.lock().storage.max_entries = max;
    }

    /// The queued mutations that could not be migrated to this build (ADR-037 decision 7): their
    /// mutation, idempotency key, the reason and their input by parameter name. They are never
    /// deleted by the client; show them, export them, [retry](Self::retry_dead_letter) or
    /// [discard](Self::discard_dead_letter) them.
    pub fn dead_letters(&self) -> Vec<DeadLetter> {
        self.shared.dead_letters(&self.ctx)
    }

    /// Runs the migration of the dead letter with `key` again (after an update that added a
    /// `#[undra::migrate]` hook, say). On success it goes to the end of the offline queue and
    /// replays when the client is online.
    ///
    /// # Errors
    ///
    /// [`RetryError::NotFound`] if no dead letter has that key, [`RetryError::Incompatible`] if it
    /// still does not migrate (it stays a dead letter, with the new reason).
    pub fn retry_dead_letter(&self, key: Uuid) -> Result<(), RetryError> {
        self.shared.retry_dead_letter(&self.ctx, key)
    }

    /// Deletes the dead letter with `key` for good. `false` if there was none.
    pub fn discard_dead_letter(&self, key: Uuid) -> bool {
        self.shared.discard_dead_letter(&self.ctx, key)
    }

    /// The persistence counters and whether the stored queue has been read (what `stats_json`
    /// reports as `query.persist` and `query.queue`).
    pub fn persist_stats(&self) -> PersistStats {
        self.shared.persist_stats()
    }
}

/// What the client's persistence did so far ([`QueryClient::persist_stats`]; ADR-037, ADR-049).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PersistStats {
    /// Writes to the `Kv` store that failed (each is tried again later).
    pub write_failed: u64,
    /// Reads from the `Kv` store that failed.
    pub read_failed: u64,
    /// Persisted cache entries deleted because they could not be migrated (they can be fetched
    /// again).
    pub dropped: u64,
    /// Cache entries and queued mutations an older build wrote that were migrated.
    pub migrated: u64,
    /// Queued mutations moved to the dead-letter queue.
    pub dead_lettered: u64,
    /// Whether the stored offline queue has been read. `false` before start-up finished and
    /// while a read fails (the queue is then neither replayed nor written).
    pub queue_readable: bool,
}

/// `ctx.query()` and `ctx.mutate(..)` on [`Ctx`] (SPEC 5.3).
///
/// Import it (`undra::prelude::*` does) to call them; like the port accessors of `undra-ports`
/// they are an extension trait because `Ctx` lives in `undra-runtime`, below this crate.
pub trait CtxQuery {
    /// The query cache of this runtime.
    ///
    /// SPEC 5.3 writes `-> &QueryClient`; this returns a small owned client bound to the `Ctx`,
    /// because the cache lives in the runtime while a client also needs the `Ctx` to spawn
    /// fetches, and a runtime that owned its own `Ctx` would never be freed.
    fn query(&self) -> QueryClient;

    /// Prepares mutation `M` with `input` (the tuple of its parameters): add an
    /// [`optimistic`](MutationBuilder::optimistic) update and
    /// [`invalidates`](MutationBuilder::invalidates) targets, then `.await` it.
    fn mutate<M: MutationDef>(&self, input: M::Input) -> MutationBuilder<M>;
}

impl CtxQuery for Ctx {
    fn query(&self) -> QueryClient {
        let shared = shared_of(self.runtime());
        shared.start(self);
        QueryClient::bound(self.clone(), shared)
    }

    fn mutate<M: MutationDef>(&self, input: M::Input) -> MutationBuilder<M> {
        let shared = shared_of(self.runtime());
        shared.start(self);
        MutationBuilder::new(self.clone(), shared, input)
    }
}
