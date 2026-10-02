//! [`QueryHandle`]: the observable a query is watched through (SPEC 9).
//!
//! A handle is a store with five signals (seven for an infinite query, see
//! [`InfiniteHandle`](crate::InfiniteHandle)), numbered exactly as `undra-bindgen` generates them
//! for every `<Name>QueryHandle` class in Swift, Kotlin and TypeScript:
//!
//! | id | signal | type |
//! |---|---|---|
//! | 0 | `data` | `Option<Q::Output>` |
//! | 1 | `status` | [`QueryStatus`] |
//! | 2 | `error` | `Option<Q::Error>` |
//! | 3 | `fetching` | `bool` |
//! | 4 | `updated_at` | `Option<Timestamp>` |
//!
//! Creating a handle registers an observer of the cache entry and starts a fetch if the entry is
//! stale or missing; dropping it (or, for a platform, releasing its object handle) removes the
//! observer. When the last observer goes, the in-flight fetch is cancelled and the entry is
//! scheduled for garbage collection.
//!
//! # Why not `#[undra::store]`?
//!
//! The store macro generates a struct per store type at compile time and registers it in the
//! schema. A query handle is one *generic* type, `QueryHandle<Q>`, instantiated once per
//! query, whose schema description bindgen already synthesizes from `QueryMeta`; registering it
//! as an object as well would put every handle in the schema twice. So the handle is written by
//! hand against the same contract the macro's output meets: a [`StoreCell`] with its signals
//! attached in declaration order, an object-table entry the runtime observes and releases, and
//! dispatch for its constructor and its methods (see [`crate::dispatch`]).

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use std::any::Any;
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::Mutex;
use undra_runtime::{AnyObject, Ctx, UndraObjectDyn, WeakCtx};
use undra_signals::{Signal, SignalsError, StoreCell};
use undra_wire::Timestamp;

use crate::defs::QueryDef;
use crate::erased::{Erased, query_vtable};
use crate::key::{Invalidate, QueryKey};
use crate::shared::{Shared, Sink, View};
use crate::status::QueryStatus;

/// The last view a handle showed, so it only writes what changed and never goes backwards.
#[derive(Default)]
struct Applied {
    seq: Option<u64>,
    data_ver: Option<u64>,
    error_ver: Option<u64>,
}

/// The part of a handle the cache publishes into.
pub(crate) struct HandleInner<Q: QueryDef> {
    data: Signal<Option<Q::Output>>,
    status: Signal<QueryStatus>,
    error: Signal<Option<Q::Error>>,
    fetching: Signal<bool>,
    updated_at: Signal<Option<Timestamp>>,
    applied: Mutex<Applied>,
}

impl<Q: QueryDef> HandleInner<Q> {
    fn new() -> HandleInner<Q> {
        HandleInner {
            data: Signal::new(None),
            status: Signal::new(QueryStatus::Idle),
            error: Signal::new(None),
            fetching: Signal::new(false),
            updated_at: Signal::new(None),
            applied: Mutex::new(Applied::default()),
        }
    }
}

impl<Q: QueryDef> Sink for HandleInner<Q> {
    fn apply(&self, view: &View) {
        let mut applied = self.applied.lock();
        if applied.seq.is_some_and(|seen| view.seq <= seen) {
            return;
        }
        applied.seq = Some(view.seq);
        if applied.data_ver != Some(view.data_ver) {
            applied.data_ver = Some(view.data_ver);
            // (`flat_data` is the entry's value; for an infinite query observed as an ordinary
            // one, the list flattened.)
            self.data.set(
                view.flat_data()
                    .as_ref()
                    .and_then(Erased::typed::<Q::Output>),
            );
        }
        if applied.error_ver != Some(view.error_ver) {
            applied.error_ver = Some(view.error_ver);
            self.error
                .set(view.error.as_ref().and_then(Erased::typed::<Q::Error>));
        }
        if self.status.get() != view.status {
            self.status.set(view.status);
        }
        if self.fetching.get() != view.fetching {
            self.fetching.set(view.fetching);
        }
        let updated_at = view.updated_at.map(Timestamp);
        if self.updated_at.get() != updated_at {
            self.updated_at.set(updated_at);
        }
    }
}

/// A live view of one query with one set of parameters (the signals, and why it is not a
/// `#[undra::store]`, are described at the top of this module).
///
/// ```
/// use undra_query::{CtxQuery, QueryDef, QueryHandle, QueryStatus};
/// # use undra_runtime::Ctx;
/// # use undra_query::BoxFuture;
/// # struct Count;
/// # impl QueryDef for Count {
/// #     const ID: u32 = 1; const KEY: &'static str = "count"; const STALE_MS: Option<u64> = None;
/// #     const PERSIST: bool = false; const RETRY: u32 = 0;
/// #     type Params = (); type Output = u32; type Error = String;
/// #     fn fetch(_: Ctx, _: ()) -> BoxFuture<Result<u32, String>> { Box::pin(async { Ok(3) }) }
/// # }
/// # let t = undra_runtime::testing::TestRuntime::new();
/// let count: QueryHandle<Count> = t.ctx().query().observe::<Count>(());
/// assert_eq!(count.status().get(), QueryStatus::Fetching); // fetching has started
/// assert_eq!(count.data().get(), None);
/// t.run_pending();                                         // the fetch runs
/// assert_eq!(count.data().get(), Some(3));
/// assert_eq!(count.status().get(), QueryStatus::Success);
/// ```
pub struct QueryHandle<Q: QueryDef> {
    inner: Arc<HandleInner<Q>>,
    link: Link,
    cell: OnceLock<Arc<StoreCell>>,
}

/// What a handle holds of the cache: its observer, and the context to reach it through.
pub(crate) struct Link {
    /// Weak (ADR-034): a handle lives in the object table, which the runtime owns, so a strong
    /// `Ctx` here would keep a dropped runtime alive for as long as the platform holds it.
    pub(crate) ctx: WeakCtx,
    pub(crate) shared: Arc<Shared>,
    pub(crate) key: QueryKey,
    sink_id: u64,
}

impl Link {
    pub(crate) fn new(ctx: &Ctx, shared: &Arc<Shared>, key: QueryKey, sink_id: u64) -> Link {
        Link {
            ctx: ctx.downgrade(),
            shared: shared.clone(),
            key,
            sink_id,
        }
    }

    pub(crate) fn refetch(&self) {
        if let Ok(ctx) = self.ctx.upgrade() {
            self.shared.refetch(&ctx, &self.key);
        }
    }

    pub(crate) fn invalidate(&self) {
        let Ok(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.shared.invalidate(
            &ctx,
            &[Invalidate::Exact {
                query_id: self.key.query_id,
                params: self.key.params.to_vec(),
            }],
        );
    }

    pub(crate) fn set_poll_interval(&self, interval: Option<Duration>) {
        if let Ok(ctx) = self.ctx.upgrade() {
            self.shared
                .set_poll_interval(&ctx, &self.key, self.sink_id, interval);
        }
    }

    pub(crate) fn settled(&self) -> Settled {
        Settled {
            shared: self.shared.clone(),
            key: self.key.clone(),
        }
    }

    /// What a snapshot keeps of the handle (ADR-059): the parameters it observes with and the
    /// polling interval it set for itself, if any. See [`crate::revive`].
    pub(crate) fn recreation(&self) -> Vec<u8> {
        let poll_ms = self
            .shared
            .state
            .lock()
            .entries
            .get(&self.key)
            .and_then(|entry| entry.polling.override_of(self.sink_id));
        crate::revive::encode_record(&self.key.params, poll_ms)
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        // A runtime that is shutting down or gone drops its cache with it.
        if let Ok(ctx) = self.ctx.upgrade() {
            self.shared.release(&ctx, &self.key, self.sink_id);
        }
    }
}

impl<Q: QueryDef> QueryHandle<Q> {
    /// Observes query `Q` with `params`: registers an observer and fetches if the entry is
    /// stale or missing.
    pub(crate) fn open(ctx: &Ctx, shared: &Arc<Shared>, params: &Q::Params) -> QueryHandle<Q> {
        use undra_wire::Encode;
        let bytes: Arc<[u8]> = Arc::from(params.encode_to_vec());
        let inner = Arc::new(HandleInner::<Q>::new());
        let sink_id = shared.new_sink_id();
        let sink: Weak<dyn Sink> = Arc::downgrade(&inner) as Weak<dyn Sink>;
        let (key, view) = shared.observe(ctx, query_vtable::<Q>(), bytes, Some((sink_id, sink)));
        // Showing the view the observer joined at: a publication that raced ahead of this is
        // newer and wins (the handle never goes back).
        ctx.txn(|| inner.apply(&view));
        QueryHandle {
            inner,
            link: Link::new(ctx, shared, key, sink_id),
            cell: OnceLock::new(),
        }
    }

    /// The latest successful result, if any. Signal 0.
    pub fn data(&self) -> &Signal<Option<Q::Output>> {
        &self.inner.data
    }

    /// Where the query is in its fetch lifecycle. Signal 1.
    pub fn status(&self) -> &Signal<QueryStatus> {
        &self.inner.status
    }

    /// The error of the latest failed fetch, cleared by the next success. Signal 2.
    pub fn error(&self) -> &Signal<Option<Q::Error>> {
        &self.inner.error
    }

    /// Whether a fetch is in flight, also while refetching stale data. Signal 3.
    pub fn fetching(&self) -> &Signal<bool> {
        &self.inner.fetching
    }

    /// When `data` was last confirmed by a fetch or a write. Signal 4.
    pub fn updated_at(&self) -> &Signal<Option<Timestamp>> {
        &self.inner.updated_at
    }

    /// Fetches again now, even if the data is fresh. A fetch already in flight is joined.
    ///
    /// A no-op once the runtime has been shut down or dropped.
    pub fn refetch(&self) {
        self.link.refetch();
    }

    /// Marks this entry stale; it refetches now, because this handle observes it.
    ///
    /// A no-op once the runtime has been shut down or dropped.
    pub fn invalidate(&self) {
        self.link.invalidate();
    }

    /// Polls this query every `interval` while this handle observes it (ADR-043): the entry
    /// refetches `interval` after its previous fetch **ended**, as long as it is observed, the
    /// client is online and the app is `Active` (or the query declares `poll_in_background`).
    /// `None` takes this handle's override away again, back to the query's own `interval`.
    ///
    /// The entry polls at the **smallest** interval among its observers, so a screen can poll
    /// faster only while it is visible (set it when it appears, clear it when it goes), and a
    /// query without an `interval` can still be polled by one observer. An interval below one
    /// second is raised to one second ([`MIN_POLL_INTERVAL_MS`](crate::MIN_POLL_INTERVAL_MS):
    /// use a stream for real-time data), one above a week lowered to a week. Calling it again
    /// replaces this handle's interval; the poll keeps counting from the end of the last fetch.
    ///
    /// A no-op once the runtime has been shut down or dropped, and after the handle is released.
    ///
    /// ```
    /// use core::time::Duration;
    /// use undra_query::{BoxFuture, CtxQuery, QueryDef};
    /// # use undra_runtime::Ctx;
    /// # struct Count;
    /// # impl QueryDef for Count {
    /// #     const ID: u32 = 1; const KEY: &'static str = "count"; const STALE_MS: Option<u64> = None;
    /// #     const PERSIST: bool = false; const RETRY: u32 = 0;
    /// #     type Params = (); type Output = u32; type Error = String;
    /// #     fn fetch(_: Ctx, _: ()) -> BoxFuture<Result<u32, String>> { Box::pin(async { Ok(3) }) }
    /// # }
    /// # let t = undra_runtime::testing::TestRuntime::new();
    /// let count = t.ctx().query().observe::<Count>(());
    /// count.set_poll_interval(Some(Duration::from_secs(5))); // while this screen is visible
    /// // ... and when it goes away:
    /// count.set_poll_interval(None);
    /// ```
    pub fn set_poll_interval(&self, interval: Option<Duration>) {
        self.link.set_poll_interval(interval);
    }

    /// A future that resolves when no fetch of this entry is in flight (at once if none is).
    pub fn settled(&self) -> Settled {
        self.link.settled()
    }

    /// Binds the five signals to a store cell, in id order: what makes the handle observable
    /// by a platform. Errors if the handle already has one.
    pub(crate) fn make_cell(&self) -> Result<Arc<StoreCell>, SignalsError> {
        if self.cell.get().is_some() {
            return Err(SignalsError::AlreadyAttached);
        }
        let cell = StoreCell::new(Q::ID);
        cell.attach(&self.inner.data, 0)?;
        cell.attach(&self.inner.status, 1)?;
        cell.attach(&self.inner.error, 2)?;
        cell.attach(&self.inner.fetching, 3)?;
        cell.attach(&self.inner.updated_at, 4)?;
        let _ = self.cell.set(cell.clone());
        Ok(cell)
    }
}

impl<Q: QueryDef> core::fmt::Debug for QueryHandle<Q> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("QueryHandle")
            .field("query", &Q::KEY)
            .field("status", &self.inner.status.get())
            .field("fetching", &self.inner.fetching.get())
            .finish_non_exhaustive()
    }
}

/// Resolves when a handle's entry has no fetch in flight. See [`QueryHandle::settled`].
pub struct Settled {
    shared: Arc<Shared>,
    key: QueryKey,
}

impl Future for Settled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.shared.poll_settled(&self.key, cx.waker()) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// What the runtime and a platform can do with a handle without knowing its query.
pub(crate) trait HandleOps: Send + Sync {
    fn refetch(&self);
    fn invalidate(&self);
    fn set_poll_interval(&self, interval: Option<Duration>);
    /// Fetches the next page. `false` if this is not the handle of an infinite query (the call
    /// does not exist on it).
    fn fetch_next_page(&self) -> bool {
        false
    }
    fn make_cell(&self) -> Result<Arc<StoreCell>, SignalsError>;
    /// What a snapshot keeps of the handle (ADR-059): [`Link::recreation`].
    fn recreation(&self) -> Vec<u8>;
}

impl<Q: QueryDef> HandleOps for QueryHandle<Q> {
    fn refetch(&self) {
        QueryHandle::refetch(self);
    }

    fn invalidate(&self) {
        QueryHandle::invalidate(self);
    }

    fn set_poll_interval(&self, interval: Option<Duration>) {
        QueryHandle::set_poll_interval(self, interval);
    }

    fn make_cell(&self) -> Result<Arc<StoreCell>, SignalsError> {
        QueryHandle::make_cell(self)
    }

    fn recreation(&self) -> Vec<u8> {
        self.link.recreation()
    }
}

/// A handle in the object table: the store the platform observes.
pub(crate) struct HandleObjectInner {
    pub(crate) ops: Box<dyn HandleOps>,
    cell: Arc<StoreCell>,
    type_id: u32,
}

/// The object-table entry of a handle. Its type id is the query id (a type per query, as
/// `undra-bindgen` numbers them).
pub(crate) struct HandleObject(Arc<HandleObjectInner>);

impl HandleObject {
    pub(crate) fn new(type_id: u32, ops: Box<dyn HandleOps>, cell: Arc<StoreCell>) -> HandleObject {
        HandleObject(Arc::new(HandleObjectInner { ops, cell, type_id }))
    }
}

impl UndraObjectDyn for HandleObject {
    fn undra_type_id(&self) -> u32 {
        self.0.type_id
    }

    fn undra_type_name(&self) -> &'static str {
        "QueryHandle"
    }

    fn as_store(&self) -> Option<&Arc<StoreCell>> {
        Some(&self.0.cell)
    }

    /// A handle is a view of the cache, not state: a snapshot does not carry it as a store.
    fn transient(&self) -> bool {
        true
    }

    /// What a snapshot keeps instead (ADR-059): the handle's parameters and its observer's own
    /// polling interval, from which a restore re-issues the handle and the runtime builds the
    /// object again when the host first uses it ([`crate::revive`]).
    fn recreation(&self) -> Option<Vec<u8>> {
        Some(self.0.ops.recreation())
    }
}

impl AnyObject for HandleObject {
    fn shared(&self) -> Arc<dyn Any + Send + Sync> {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::BoxFuture;
    use undra_runtime::testing::TestRuntime;

    struct Q;

    impl QueryDef for Q {
        const ID: u32 = 0x1234;
        const KEY: &'static str = "q";
        const STALE_MS: Option<u64> = None;
        const PERSIST: bool = false;
        const RETRY: u32 = 0;
        type Params = ();
        type Output = u32;
        type Error = String;
        fn fetch(_: Ctx, _: ()) -> BoxFuture<Result<u32, String>> {
            Box::pin(async { Ok(1) })
        }
    }

    fn view(seq: u64, data: Option<u32>, data_ver: u64) -> View {
        View {
            seq,
            data: data.map(Erased::new),
            data_ver,
            error: None,
            error_ver: 0,
            status: QueryStatus::Success,
            fetching: false,
            updated_at: Some(5),
            list: None,
        }
    }

    #[test]
    fn a_handle_never_goes_back_to_an_older_view() {
        let inner = HandleInner::<Q>::new();
        inner.apply(&view(5, Some(1), 1));
        assert_eq!(inner.data.get(), Some(1));
        inner.apply(&view(4, Some(2), 2));
        assert_eq!(
            inner.data.get(),
            Some(1),
            "a view older than the one shown is ignored"
        );
        inner.apply(&view(5, Some(3), 3));
        assert_eq!(
            inner.data.get(),
            Some(1),
            "so is a repeat of the sequence number"
        );
        inner.apply(&view(6, Some(4), 4));
        assert_eq!(inner.data.get(), Some(4));
        assert_eq!(inner.updated_at.get(), Some(Timestamp(5)));
    }

    #[test]
    fn the_first_view_is_always_shown_even_at_sequence_zero() {
        let inner = HandleInner::<Q>::new();
        inner.apply(&view(0, Some(9), 0));
        assert_eq!(inner.data.get(), Some(9));
        assert_eq!(inner.status.get(), QueryStatus::Success);
    }

    #[test]
    fn a_handle_gets_one_cell_and_the_object_is_transient_and_typed_by_the_query() {
        let t = TestRuntime::new();
        let handle = crate::CtxQuery::query(&t.ctx()).observe::<Q>(());
        let cell = handle.make_cell().unwrap();
        assert_eq!(cell.signal_count(), 5);
        assert_eq!(StoreCell::type_id(&cell), Q::ID);
        assert_eq!(
            handle.make_cell().unwrap_err(),
            SignalsError::AlreadyAttached
        );

        let object = HandleObject::new(Q::ID, Box::new(handle), cell.clone());
        assert_eq!(object.undra_type_id(), Q::ID);
        assert_eq!(object.undra_type_name(), "QueryHandle");
        assert!(object.transient());
        assert!(Arc::ptr_eq(object.as_store().unwrap(), &cell));
        assert!(object.shared().downcast::<HandleObjectInner>().is_ok());
    }

    #[test]
    fn debug_shows_the_query_and_where_it_is() {
        let t = TestRuntime::new();
        let handle = crate::CtxQuery::query(&t.ctx()).observe::<Q>(());
        let text = format!("{handle:?}");
        assert!(
            text.contains("QueryHandle") && text.contains("\"q\""),
            "{text}"
        );
    }
}
