//! Infinite queries (ADR-043 decision 2): a list loaded one page at a time, shown to the
//! platforms as one keyed list that grows.
//!
//! # The model
//!
//! An entry of an infinite query holds its **pages**: for each, the rows it returned and the
//! cursor of the page after it. What a handle shows (`data`) is their concatenation, held in a
//! keyed `Signal<Vec<T>>`; `has_next_page` is whether the last page has a next cursor.
//!
//! * **`fetch_next_page`** (a next cursor, no fetch in flight) fetches the page at that cursor
//!   and appends its rows with the recorded `push`: the change-set of every observer is a keyed
//!   patch of the new rows, O(page) however long the list is (ADR-027). A failure keeps the data,
//!   shows the error and clears `fetching_next_page`; it retries like any fetch.
//! * **A refetch** (stale, invalidated, foreground, online, a poll) fetches the loaded pages again
//!   **in order from the first**, each with the cursor the previous response returned (cursors can
//!   change), up to the number of pages loaded or `refetch_pages` if that is smaller. Only when
//!   every page arrived are they put in place of the old ones, with a raw write of the list: the
//!   commit diffs by key and sends only what changed (a full value when more than half did). A
//!   failure in the middle keeps the old pages. The data stays visible while it refetches.
//! * **The first fetch** is page one.
//! * **Edits** (`CacheView::update_items`, `QueryClient::set`) replace the flattened list. The
//!   pages are laid out again with the page sizes they had (the last page takes what is left), so
//!   cursors and the page count are unchanged; the next fetch of the entry puts the server's
//!   answer in their place.
//!
//! # Keys
//!
//! `item_key` identifies a row, and the list is keyed on it: two rows of one entry must not share
//! a key. A server that pages by offset can repeat a row when its list shifts between two requests;
//! page by a cursor that does not move (the id the last page ended at) or drop the repeats in the
//! query function. The recorded append does not look at keys (ADR-027), so a repeat is not noticed
//! when a page is appended; a host that applies the patch shows it twice.
//!
//! # Persistence
//!
//! A `persist` entry stores its first `persist_pages` pages (default 1) and the cursor after the
//! last of them: the value `{ next: Option<C>, pages: Vec<Vec<T>> }` (the closure of that record
//! is the entry's fingerprint, ADR-037), migrated by field name like any entry. A restored entry
//! knows only the cursor after its last page, which is all `fetch_next_page` and a later write
//! need.
//!
//! # Linking
//!
//! The engine here is reached from the generic code only through [`PagedVTable`], which only an
//! infinite query submits, so a core with ordinary queries only does not link it (ADR-052).

use core::any::Any;
use core::marker::PhantomData;
use core::time::Duration;
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::Mutex;
use undra_meta::{ParamDef, QueryKind, Schema, TypeClosure, TypeRef};
use undra_runtime::executor::TaskId;
use undra_runtime::log::ERROR;
use undra_runtime::persist::{self, RegisteredHooks};
use undra_runtime::{Ctx, WeakCtx};
use undra_signals::{Signal, SignalsError, StoreCell};
use undra_wire::{Decode, Encode, Reader, Timestamp, Writer};

use crate::defs::{BoxFuture, InfiniteQueryDef, Page};
use crate::erased::{Erased, Failure, OpenFn, QueryVTable, query_vtable};
use crate::handle::{HandleOps, Link, Settled};
use crate::key::QueryKey;
use crate::poll::Base;
use crate::retry::with_retries;
use crate::shared::{Fx, Inflight, Shared, Sink, View};
use crate::status::QueryStatus;

// -------------------------------------------------------------------------------------------
// The entry side
// -------------------------------------------------------------------------------------------

/// One loaded page: its rows (a `Vec<T>`, type-erased) and the cursor its response returned.
#[derive(Clone)]
pub(crate) struct PageRec {
    pub(crate) ops: &'static PagedVTable,
    /// The rows, a `Vec<T>`.
    pub(crate) items: Arc<dyn Any + Send + Sync>,
    pub(crate) len: usize,
    /// The cursor of the next page (`C`), if there is one.
    pub(crate) next: Option<Erased>,
}

impl PageRec {
    /// The encoding of the rows (a `Vec<T>`). Made when asked for and not kept: a refetch compares
    /// pages, persistence stores a few, and nothing else needs the bytes, so a long list does not
    /// hold its encoding next to its rows.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        (self.ops.encode_items)(&*self.items)
    }

    /// A page of `items` (a `Vec<T>` of `len` rows) followed by `next`.
    fn of(
        ops: &'static PagedVTable,
        items: Arc<dyn Any + Send + Sync>,
        len: usize,
        next: Option<Erased>,
    ) -> PageRec {
        PageRec {
            ops,
            items,
            len,
            next,
        }
    }
}

/// The rows a publication appended to the list, for the handles that were showing the list as it
/// was just before.
#[derive(Clone)]
pub(crate) struct Appended {
    /// The list version the rows were appended to.
    pub(crate) from: u64,
    /// The rows, a `Vec<T>`.
    pub(crate) items: Arc<dyn Any + Send + Sync>,
}

/// What a persisted entry decodes to: its pages, the last one knowing the next cursor.
pub(crate) struct Restored(pub(crate) Vec<PageRec>);

/// The list state of an entry of an infinite query.
pub(crate) struct PagedState {
    /// The loaded pages. Shared with the publications and the optimistic snapshots that show
    /// them: a change copies the (short) vector of pages only while one of those is alive.
    pub(crate) pages: Arc<Vec<PageRec>>,
    /// Changes whenever the flattened list does.
    pub(crate) list_ver: u64,
    /// The rows the latest change appended, if it was an append.
    pub(crate) appended: Option<Appended>,
}

impl PagedState {
    pub(crate) fn new() -> PagedState {
        PagedState {
            pages: Arc::new(Vec::new()),
            list_ver: 0,
            appended: None,
        }
    }

    /// The first page has been loaded.
    pub(crate) fn has_data(&self) -> bool {
        !self.pages.is_empty()
    }

    /// The cursor of the page after the last loaded one.
    pub(crate) fn next_cursor(&self) -> Option<Erased> {
        self.pages.last().and_then(|page| page.next.clone())
    }

    /// Puts `pages` in place of the loaded ones.
    pub(crate) fn replace(&mut self, pages: Arc<Vec<PageRec>>) {
        self.pages = pages;
        self.list_ver += 1;
        self.appended = None;
    }

    /// Appends one page; the publication tells the handles which rows it added.
    fn append(&mut self, page: PageRec) {
        self.appended = Some(Appended {
            from: self.list_ver,
            items: page.items.clone(),
        });
        self.list_ver += 1;
        Arc::make_mut(&mut self.pages).push(page);
    }

    /// The encoded form of the first `persist_pages` pages (what a persisted entry stores).
    pub(crate) fn persisted(&self) -> Option<Vec<u8>> {
        let first = self.pages.first()?;
        let ops = first.ops;
        Some((ops.encode)(&self.pages, ops.persist_pages as usize))
    }
}

/// What a handle is shown of the list: a version, the pages (to lay the list out again) and the
/// rows the last change appended (to append them).
#[derive(Clone)]
pub(crate) struct ListView {
    pub(crate) ver: u64,
    pub(crate) pages: Arc<Vec<PageRec>>,
    pub(crate) appended: Option<Appended>,
    pub(crate) has_next: bool,
    pub(crate) fetching_next: bool,
}

/// What a page fetch of an infinite entry ended with.
pub(crate) enum PagedOutcome {
    /// Every loaded page was fetched again.
    Refreshed(Vec<PageRec>),
    /// The next page.
    Appended(PageRec),
    /// A fetch failed after its retries.
    Failed(Failure),
}

/// Whether two page lists are the same rows and cursors.
fn same_pages(old: &[PageRec], new: &[PageRec]) -> bool {
    old.len() == new.len()
        && old.iter().zip(new).all(|(a, b)| {
            a.len == b.len
                && (Arc::ptr_eq(&a.items, &b.items) || a.bytes() == b.bytes())
                && match (&a.next, &b.next) {
                    (None, None) => true,
                    (Some(x), Some(y)) => x.bytes == y.bytes,
                    _ => false,
                }
        })
}

// -------------------------------------------------------------------------------------------
// The type-erased half
// -------------------------------------------------------------------------------------------

/// The closure of an infinite query's persisted entry: `{ next: Option<C>, pages: Vec<Vec<T>> }`
/// (ADR-043 decision 2.5), its fields migrated by name like a mutation's parameters. `None` for a
/// query that is not infinite.
fn closure_of(schema: &Schema, query_id: u32) -> Option<TypeClosure> {
    let query = schema
        .queries
        .iter()
        .find(|q| q.query_id == query_id && q.kind == QueryKind::Query)?;
    let infinite = query.infinite.as_ref()?;
    let rows = match &query.returns {
        TypeRef::Result(ok, _) => (**ok).clone(),
        other => other.clone(),
    };
    Some(schema.closure_of_params(&[
        ParamDef {
            name: "next".to_owned(),
            ty: TypeRef::option(infinite.cursor.clone()),
        },
        ParamDef {
            name: "pages".to_owned(),
            ty: TypeRef::vec(rows),
        },
    ]))
}

/// An entry stored with the closure `old`, as bytes of the closure `new`: field by field.
fn migrate_entry(data: &[u8], old: &TypeClosure, new: &TypeClosure) -> Result<Vec<u8>, String> {
    let record = persist::decode_params(data, "pages", old).map_err(|e| e.to_string())?;
    persist::migrate_params(&record, old, new, &RegisteredHooks).map_err(|e| e.to_string())
}

/// Migrates an entry's bytes from the closure they were written with to the current one.
type MigrateFn = fn(&[u8], &TypeClosure, &TypeClosure) -> Result<Vec<u8>, String>;

/// Fetches one page: the encoded parameters and the cursor (`None`: the first page).
type FetchPageFn = fn(Ctx, &[u8], Option<&Erased>) -> BoxFuture<Result<PageRec, Failure>>;

/// The type-erased half of one infinite query: what the engine does with its pages without
/// knowing `T` or `C`. Built with [`paged_vtable`]; the fields are the engine's business.
pub struct PagedVTable {
    pub(crate) refetch_pages: Option<u32>,
    pub(crate) persist_pages: u32,
    /// Fetches one page: the encoded parameters and the cursor (`None`: the first page).
    pub(crate) fetch_page: FetchPageFn,
    /// The flattened list (a `Vec<T>`).
    pub(crate) flatten: fn(&[PageRec]) -> Erased,
    /// The pages laid out again for a new flattened list, with the old page sizes and cursors.
    pub(crate) rebuild: fn(&[PageRec], &Erased) -> Option<Vec<PageRec>>,
    /// The persisted form of the first `n` pages.
    pub(crate) encode: fn(&[PageRec], usize) -> Vec<u8>,
    /// The encoding of one page's rows (a `Vec<T>`).
    pub(crate) encode_items: fn(&(dyn Any + Send + Sync)) -> Vec<u8>,
    /// Reads a persisted entry: an `Erased` holding a [`Restored`].
    pub(crate) decode: fn(&[u8]) -> Result<Erased, undra_wire::WireError>,
    /// Observes the query for a platform: an infinite handle.
    pub(crate) open: OpenFn,
    /// The closure that identifies a persisted entry of the query (see [`closure_of`]).
    pub(crate) closure: fn(&Schema, u32) -> Option<TypeClosure>,
    /// Migrates a persisted entry written with another closure (see [`migrate_entry`]).
    pub(crate) migrate: MigrateFn,
    /// Starts the task that fetches the loaded pages again.
    pub(crate) spawn_refresh:
        fn(&Arc<Shared>, &Ctx, &QueryKey, &'static QueryVTable, u64, usize) -> TaskId,
    /// Starts the task that fetches the page at a cursor.
    pub(crate) spawn_next:
        fn(&Arc<Shared>, &Ctx, &QueryKey, &'static QueryVTable, u64, Erased) -> TaskId,
}

struct PagedHolder<Q>(PhantomData<fn() -> Q>);

impl<Q: InfiniteQueryDef> PagedHolder<Q> {
    const VTABLE: PagedVTable = PagedVTable {
        refetch_pages: Q::REFETCH_PAGES,
        persist_pages: if Q::PERSIST_PAGES == 0 {
            1
        } else {
            Q::PERSIST_PAGES
        },
        fetch_page: fetch_page_erased::<Q>,
        flatten: flatten_erased::<Q>,
        rebuild: rebuild_erased::<Q>,
        encode: encode_erased::<Q>,
        encode_items: encode_items_erased::<Q>,
        decode: decode_erased::<Q>,
        open: open_erased::<Q>,
        closure: closure_of,
        migrate: migrate_entry,
        spawn_refresh,
        spawn_next,
    };
}

/// The paging table of infinite query `Q`. Generated code names it: `QueryDef::PAGED` of an
/// infinite query is `Some(paged_vtable::<Self>())`.
#[doc(hidden)]
pub const fn paged_vtable<Q: InfiniteQueryDef>() -> &'static PagedVTable {
    &PagedHolder::<Q>::VTABLE
}

fn page_rec<Q: InfiniteQueryDef>(page: Page<Q::Item, Q::Cursor>) -> PageRec {
    let len = page.items.len();
    PageRec::of(
        &PagedHolder::<Q>::VTABLE,
        Arc::new(page.items),
        len,
        page.next.map(Erased::new),
    )
}

/// The rows of a page of `Q`.
fn rows_of<Q: InfiniteQueryDef>(page: &PageRec) -> &[Q::Item] {
    page.items
        .downcast_ref::<Vec<Q::Item>>()
        .map_or(&[], Vec::as_slice)
}

fn broken<T: 'static>(message: String) -> BoxFuture<Result<T, Failure>> {
    Box::pin(async move { Err(Failure::Broken(message)) })
}

fn fetch_page_erased<Q: InfiniteQueryDef>(
    ctx: Ctx,
    params: &[u8],
    cursor: Option<&Erased>,
) -> BoxFuture<Result<PageRec, Failure>> {
    let params = match Q::Params::decode_exact(params) {
        Ok(params) => params,
        Err(e) => {
            return broken(format!(
                "the parameters of query `{}` do not decode: {e}",
                Q::KEY
            ));
        }
    };
    let cursor = match cursor {
        None => None,
        Some(erased) => match erased.typed::<Q::Cursor>() {
            Some(cursor) => Some(cursor),
            None => return broken(format!("a cursor of query `{}` has the wrong type", Q::KEY)),
        },
    };
    let fetch = Q::fetch_page(ctx, params, cursor);
    Box::pin(async move {
        match fetch.await {
            Ok(page) => Ok(page_rec::<Q>(page)),
            Err(error) => Err(Failure::Error(Erased::new(error))),
        }
    })
}

/// The encoding of the flattened list (a `Vec<T>`): the pages' own encodings one after the other
/// under one count, so no row is encoded twice.
pub(crate) fn flat_bytes(pages: &[PageRec]) -> Vec<u8> {
    let total: usize = pages.iter().map(|p| p.len).sum();
    let mut w = Writer::new();
    w.write_len(u32::try_from(total).unwrap_or(u32::MAX));
    for page in pages {
        w.write_raw(page.bytes().get(4..).unwrap_or(&[]));
    }
    w.into_vec()
}

fn flatten_erased<Q: InfiniteQueryDef>(pages: &[PageRec]) -> Erased {
    let mut rows: Vec<Q::Item> = Vec::with_capacity(pages.iter().map(|p| p.len).sum());
    for page in pages {
        rows.extend_from_slice(rows_of::<Q>(page));
    }
    Erased {
        bytes: Arc::from(flat_bytes(pages)),
        value: Arc::new(rows),
    }
}

fn rebuild_erased<Q: InfiniteQueryDef>(old: &[PageRec], flat: &Erased) -> Option<Vec<PageRec>> {
    let flat = flat.value.downcast_ref::<Vec<Q::Item>>()?;
    let Some((last, init)) = old.split_last() else {
        return Some(vec![PageRec::of(
            &PagedHolder::<Q>::VTABLE,
            Arc::new(flat.clone()),
            flat.len(),
            None,
        )]);
    };
    let mut rest = flat.as_slice();
    let mut pages = Vec::with_capacity(old.len());
    for page in init {
        let (head, tail) = rest.split_at(page.len.min(rest.len()));
        pages.push(PageRec::of(
            page.ops,
            Arc::new(head.to_vec()),
            head.len(),
            page.next.clone(),
        ));
        rest = tail;
    }
    pages.push(PageRec::of(
        last.ops,
        Arc::new(rest.to_vec()),
        rest.len(),
        last.next.clone(),
    ));
    Some(pages)
}

fn encode_erased<Q: InfiniteQueryDef>(pages: &[PageRec], n: usize) -> Vec<u8> {
    let n = n.min(pages.len());
    let next: Option<Q::Cursor> = n
        .checked_sub(1)
        .and_then(|last| pages.get(last))
        .and_then(|page| page.next.as_ref())
        .and_then(Erased::typed::<Q::Cursor>);
    let mut w = Writer::new();
    next.encode(&mut w);
    w.write_len(u32::try_from(n).unwrap_or(u32::MAX));
    for page in &pages[..n] {
        w.write_raw(&page.bytes());
    }
    w.into_vec()
}

fn encode_items_erased<Q: InfiniteQueryDef>(items: &(dyn Any + Send + Sync)) -> Vec<u8> {
    items
        .downcast_ref::<Vec<Q::Item>>()
        .map(Encode::encode_to_vec)
        .unwrap_or_default()
}

fn decode_erased<Q: InfiniteQueryDef>(bytes: &[u8]) -> Result<Erased, undra_wire::WireError> {
    let mut r = Reader::new(bytes);
    let next = Option::<Q::Cursor>::decode(&mut r)?;
    let pages = Vec::<Vec<Q::Item>>::decode(&mut r)?;
    r.finish()?;
    let count = pages.len();
    let recs: Vec<PageRec> = pages
        .into_iter()
        .enumerate()
        .map(|(at, items)| {
            let len = items.len();
            let cursor = if at + 1 == count { next.clone() } else { None };
            PageRec::of(
                &PagedHolder::<Q>::VTABLE,
                Arc::new(items),
                len,
                cursor.map(Erased::new),
            )
        })
        .collect();
    Ok(Erased {
        bytes: Arc::from(bytes),
        value: Arc::new(Restored(recs)),
    })
}

fn open_erased<Q: InfiniteQueryDef>(
    client: &crate::QueryClient,
    args: &[u8],
) -> Result<Box<dyn HandleOps>, undra_wire::WireError> {
    let params = Q::Params::decode_exact(args)?;
    Ok(Box::new(client.infinite::<Q>(params)))
}

// -------------------------------------------------------------------------------------------
// The engine: tasks and their results
// -------------------------------------------------------------------------------------------

fn spawn_refresh(
    shared: &Arc<Shared>,
    ctx: &Ctx,
    key: &QueryKey,
    vt: &'static QueryVTable,
    serial: u64,
    loaded: usize,
) -> TaskId {
    ctx.spawn(run_refresh(
        shared.clone(),
        ctx.downgrade(),
        key.clone(),
        vt,
        serial,
        loaded,
    ))
}

fn spawn_next(
    shared: &Arc<Shared>,
    ctx: &Ctx,
    key: &QueryKey,
    vt: &'static QueryVTable,
    serial: u64,
    cursor: Erased,
) -> TaskId {
    ctx.spawn(run_next_page(
        shared.clone(),
        ctx.downgrade(),
        key.clone(),
        vt,
        serial,
        cursor,
    ))
}

/// Fetches one page with the query's retries, holding only the weak context between attempts.
async fn fetch_with_retries(
    weak: &WeakCtx,
    key: &QueryKey,
    vt: &'static QueryVTable,
    paged: &'static PagedVTable,
    cursor: Option<Erased>,
) -> Result<PageRec, Failure> {
    with_retries(weak, vt.retry, Failure::retryable, || {
        match weak.upgrade() {
            Ok(ctx) => (paged.fetch_page)(ctx, &key.params, cursor.as_ref()),
            Err(gone) => broken(gone.to_string()),
        }
    })
    .await
}

/// The pages of a refetch: `loaded` of them (at most `refetch_pages`, at least the first), each
/// asked for with the cursor the previous one returned.
async fn refresh_pages(
    weak: &WeakCtx,
    key: &QueryKey,
    vt: &'static QueryVTable,
    paged: &'static PagedVTable,
    loaded: usize,
) -> PagedOutcome {
    let wanted = paged
        .refetch_pages
        .map_or(loaded, |n| loaded.min(n as usize))
        .max(1);
    let mut pages: Vec<PageRec> = Vec::new();
    let mut cursor: Option<Erased> = None;
    for _ in 0..wanted {
        match fetch_with_retries(weak, key, vt, paged, cursor.take()).await {
            Ok(page) => {
                cursor = page.next.clone();
                pages.push(page);
                if cursor.is_none() {
                    break;
                }
            }
            Err(failure) => return PagedOutcome::Failed(failure),
        }
    }
    PagedOutcome::Refreshed(pages)
}

async fn run_refresh(
    shared: Arc<Shared>,
    weak: WeakCtx,
    key: QueryKey,
    vt: &'static QueryVTable,
    serial: u64,
    loaded: usize,
) {
    let mut guard = crate::shared::FetchGuard::new(&shared, &weak, &key, serial);
    let outcome = match vt.paged {
        Some(paged) => refresh_pages(&weak, &key, vt, paged, loaded).await,
        None => PagedOutcome::Failed(Failure::Broken("not an infinite query".to_owned())),
    };
    guard.disarm();
    if let Ok(ctx) = weak.upgrade() {
        shared.complete_pages(&ctx, &key, serial, outcome);
    }
}

async fn run_next_page(
    shared: Arc<Shared>,
    weak: WeakCtx,
    key: QueryKey,
    vt: &'static QueryVTable,
    serial: u64,
    cursor: Erased,
) {
    let mut guard = crate::shared::FetchGuard::new(&shared, &weak, &key, serial);
    let outcome = match vt.paged {
        Some(paged) => match fetch_with_retries(&weak, &key, vt, paged, Some(cursor)).await {
            Ok(page) => PagedOutcome::Appended(page),
            Err(failure) => PagedOutcome::Failed(failure),
        },
        None => PagedOutcome::Failed(Failure::Broken("not an infinite query".to_owned())),
    };
    guard.disarm();
    if let Ok(ctx) = weak.upgrade() {
        shared.complete_pages(&ctx, &key, serial, outcome);
    }
}

impl Shared {
    /// Fetches the page after the last loaded one, if the entry has a next cursor and nothing is
    /// being fetched.
    pub(crate) fn fetch_next_page(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey) {
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let Some(entry) = state.entries.get_mut(key) else {
                return;
            };
            if entry.inflight.is_some() || entry.observers == 0 {
                return;
            }
            let Some(paged) = entry.vt.paged else {
                return;
            };
            let Some(cursor) = entry.paged.as_ref().and_then(PagedState::next_cursor) else {
                return;
            };
            let serial = self.new_serial();
            let task = (paged.spawn_next)(self, ctx, key, entry.vt, serial, cursor);
            entry.inflight = Some(Inflight {
                serial,
                task,
                next_page: true,
            });
            self.cancel_poll(entry, &mut fx);
            fx.publish(entry);
        }
        fx.run(ctx);
    }

    /// A page fetch of an infinite entry ended.
    pub(crate) fn complete_pages(
        self: &Arc<Self>,
        ctx: &Ctx,
        key: &QueryKey,
        serial: u64,
        outcome: PagedOutcome,
    ) {
        let now = self.now(ctx);
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let Some(entry) = state.entries.get_mut(key) else {
                return;
            };
            if entry.inflight.as_ref().map(|i| i.serial) != Some(serial) {
                // Superseded or cancelled while the result was on its way.
                return;
            }
            entry.inflight = None;
            // Newer than any optimistic write before it (see `Shared::complete`).
            entry.stamp = self.new_stamp();
            match outcome {
                PagedOutcome::Refreshed(pages) => {
                    if let Some(paged) = entry.paged.as_mut() {
                        if !same_pages(&paged.pages, &pages) {
                            paged.replace(Arc::new(pages));
                        }
                    }
                    self.settle_success(ctx, key, entry, now);
                }
                PagedOutcome::Appended(page) => {
                    let mut persist = false;
                    if let Some(paged) = entry.paged.as_mut() {
                        paged.append(page);
                        persist = entry
                            .vt
                            .paged
                            .is_some_and(|p| paged.pages.len() <= p.persist_pages as usize);
                    }
                    // The head of the list was not confirmed: `updated_at` stays what the last
                    // refetch made it.
                    if entry.error.take().is_some() {
                        entry.error_ver += 1;
                    }
                    entry.failed = false;
                    if persist && entry.vt.persist {
                        self.schedule_persist(ctx, key, entry);
                    }
                }
                PagedOutcome::Failed(Failure::Error(error)) => {
                    entry.error = Some(error);
                    entry.error_ver += 1;
                    entry.failed = false;
                }
                PagedOutcome::Failed(Failure::Broken(message)) => {
                    entry.failed = true;
                    Shared::log(ctx, ERROR, &message);
                }
            }
            fx.publish(entry);
            self.reschedule_poll(ctx, key, entry, &mut fx, now, Base::Now);
        }
        fx.run(ctx);
    }
}

// -------------------------------------------------------------------------------------------
// The handle
// -------------------------------------------------------------------------------------------

/// The last view a handle showed.
#[derive(Default)]
struct Applied {
    seq: Option<u64>,
    list_ver: Option<u64>,
    error_ver: Option<u64>,
}

/// The part of an infinite handle the cache publishes into.
pub(crate) struct InfiniteInner<Q: InfiniteQueryDef> {
    data: Signal<Vec<Q::Item>>,
    status: Signal<QueryStatus>,
    error: Signal<Option<Q::Error>>,
    fetching: Signal<bool>,
    updated_at: Signal<Option<Timestamp>>,
    has_next_page: Signal<bool>,
    fetching_next_page: Signal<bool>,
    applied: Mutex<Applied>,
}

impl<Q: InfiniteQueryDef> InfiniteInner<Q> {
    fn new() -> InfiniteInner<Q> {
        InfiniteInner {
            data: Signal::new(Vec::new()),
            status: Signal::new(QueryStatus::Idle),
            error: Signal::new(None),
            fetching: Signal::new(false),
            updated_at: Signal::new(None),
            has_next_page: Signal::new(false),
            fetching_next_page: Signal::new(false),
            applied: Mutex::new(Applied::default()),
        }
    }

    /// Shows the pages as the list: appends the rows the list just grew by, or lays the whole
    /// list out again.
    fn show_list(&self, applied: &mut Applied, list: &crate::paged::ListView) {
        if applied.list_ver == Some(list.ver) {
            return;
        }
        let appended = match (&list.appended, applied.list_ver) {
            (Some(a), Some(shown)) if a.from == shown => a.items.downcast_ref::<Vec<Q::Item>>(),
            _ => None,
        };
        match appended {
            Some(rows) => {
                // The recorded `push`: the commit sends these rows and nothing else.
                for row in rows {
                    self.data.push(row.clone());
                }
            }
            None => {
                let mut flat: Vec<Q::Item> = Vec::new();
                for page in list.pages.iter() {
                    flat.extend_from_slice(rows_of::<Q>(page));
                }
                // A raw write: the commit diffs by key and sends what changed.
                if !(flat.is_empty() && self.data.with(Vec::is_empty)) {
                    self.data.set(flat);
                }
            }
        }
        applied.list_ver = Some(list.ver);
    }
}

impl<Q: InfiniteQueryDef> Sink for InfiniteInner<Q> {
    fn apply(&self, view: &View) {
        let mut applied = self.applied.lock();
        if applied.seq.is_some_and(|seen| view.seq <= seen) {
            return;
        }
        applied.seq = Some(view.seq);
        if let Some(list) = &view.list {
            self.show_list(&mut applied, list);
            if self.has_next_page.get() != list.has_next {
                self.has_next_page.set(list.has_next);
            }
            if self.fetching_next_page.get() != list.fetching_next {
                self.fetching_next_page.set(list.fetching_next);
            }
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

/// A live view of one infinite query with one set of parameters (ADR-043): the rows loaded so far
/// as one keyed list, and `fetch_next_page` to load more. Made by
/// [`ctx.query().infinite::<Q>(params)`](crate::QueryClient::infinite).
///
/// The seven signals are numbered as `undra-bindgen` generates them for the platforms: `data` 0
/// (a `Vec<T>` keyed by `item_key`; empty, not absent, before the first page), `status` 1,
/// `error` 2, `fetching` 3, `updated_at` 4, `has_next_page` 5 and `fetching_next_page` 6.
///
/// ```
/// use undra_query::{BoxFuture, CtxQuery, InfiniteQueryDef, Page, QueryDef};
/// use undra_runtime::Ctx;
/// # struct Numbers;
/// # impl QueryDef for Numbers {
/// #     const ID: u32 = 1; const KEY: &'static str = "numbers"; const STALE_MS: Option<u64> = None;
/// #     const PERSIST: bool = false; const RETRY: u32 = 0;
/// #     const PAGED: Option<&'static undra_query::PagedVTable> = Some(undra_query::paged_vtable::<Self>());
/// #     type Params = (); type Output = Vec<u32>; type Error = String;
/// #     fn fetch(ctx: Ctx, params: ()) -> BoxFuture<Result<Vec<u32>, String>> {
/// #         undra_query::fetch_first_page::<Self>(ctx, params)
/// #     }
/// # }
/// # impl InfiniteQueryDef for Numbers {
/// #     type Item = u32; type Cursor = u32;
/// #     fn fetch_page(_: Ctx, _: (), cursor: Option<u32>) -> BoxFuture<Result<Page<u32, u32>, String>> {
/// #         let from = cursor.unwrap_or(0);
/// #         Box::pin(async move { Ok(Page::new((from..from + 3).collect(), Some(from + 3))) })
/// #     }
/// #     fn item_key(n: &u32) -> u64 { u64::from(*n) }
/// # }
/// # let t = undra_runtime::testing::TestRuntime::new();
/// let numbers = t.ctx().query().infinite::<Numbers>(());
/// assert!(numbers.data().get().is_empty());          // nothing yet, and not `None`
/// t.run_pending();
/// assert_eq!(numbers.data().get(), [0, 1, 2]);
/// assert!(numbers.has_next_page().get());
/// numbers.fetch_next_page();
/// t.run_pending();
/// assert_eq!(numbers.items().get(), [0, 1, 2, 3, 4, 5]); // `items()` is `data()`
/// ```
pub struct InfiniteHandle<Q: InfiniteQueryDef> {
    inner: Arc<InfiniteInner<Q>>,
    link: Link,
    cell: OnceLock<Arc<StoreCell>>,
}

impl<Q: InfiniteQueryDef> InfiniteHandle<Q> {
    /// Observes infinite query `Q` with `params`: registers an observer and fetches the first
    /// page if the entry is stale or missing.
    pub(crate) fn open(ctx: &Ctx, shared: &Arc<Shared>, params: &Q::Params) -> InfiniteHandle<Q> {
        let bytes: Arc<[u8]> = Arc::from(params.encode_to_vec());
        let inner = Arc::new(InfiniteInner::<Q>::new());
        let sink_id = shared.new_sink_id();
        let sink: Weak<dyn Sink> = Arc::downgrade(&inner) as Weak<dyn Sink>;
        let (key, view) = shared.observe(ctx, query_vtable::<Q>(), bytes, Some((sink_id, sink)));
        ctx.txn(|| inner.apply(&view));
        InfiniteHandle {
            inner,
            link: Link::new(ctx, shared, key, sink_id),
            cell: OnceLock::new(),
        }
    }

    /// The rows loaded so far, a keyed list: empty before the first page, never absent. Signal 0.
    pub fn data(&self) -> &Signal<Vec<Q::Item>> {
        &self.inner.data
    }

    /// The same list as [`data`](Self::data), under the name a store reads it by: a valid source
    /// for a derived list, `items().derive().filter(..)` (ADR-039).
    pub fn items(&self) -> &Signal<Vec<Q::Item>> {
        &self.inner.data
    }

    /// Where the query is in its fetch lifecycle (`Fetching` only while nothing is shown yet).
    /// Signal 1.
    pub fn status(&self) -> &Signal<QueryStatus> {
        &self.inner.status
    }

    /// The error of the latest failed fetch (a refetch or a next page), cleared by the next
    /// success. Signal 2.
    pub fn error(&self) -> &Signal<Option<Q::Error>> {
        &self.inner.error
    }

    /// Whether a fetch of any kind is in flight. Signal 3.
    pub fn fetching(&self) -> &Signal<bool> {
        &self.inner.fetching
    }

    /// When the head of the list was last confirmed by a fetch or a write (loading a next page
    /// does not move it). Signal 4.
    pub fn updated_at(&self) -> &Signal<Option<Timestamp>> {
        &self.inner.updated_at
    }

    /// Whether the last loaded page names a page after it. Signal 5.
    pub fn has_next_page(&self) -> &Signal<bool> {
        &self.inner.has_next_page
    }

    /// Whether the fetch in flight is a next page (not a refetch). Signal 6.
    pub fn fetching_next_page(&self) -> &Signal<bool> {
        &self.inner.fetching_next_page
    }

    /// Fetches the next page and appends its rows to `data`. Does nothing without a next page,
    /// while a fetch is in flight, or once the runtime is gone.
    pub fn fetch_next_page(&self) {
        if let Ok(ctx) = self.link.ctx.upgrade() {
            self.link.shared.fetch_next_page(&ctx, &self.link.key);
        }
    }

    /// Fetches the loaded pages again now, even if the data is fresh (at most `refetch_pages` of
    /// them). A fetch already in flight is joined.
    pub fn refetch(&self) {
        self.link.refetch();
    }

    /// Marks this entry stale; it refetches now, because this handle observes it.
    pub fn invalidate(&self) {
        self.link.invalidate();
    }

    /// Polls at `interval` while this handle observes the query (an interval below one second is
    /// raised to one second); `None` clears this handle's override. See
    /// [`QueryHandle::set_poll_interval`](crate::QueryHandle::set_poll_interval).
    pub fn set_poll_interval(&self, interval: Option<Duration>) {
        self.link.set_poll_interval(interval);
    }

    /// A future that resolves when no fetch of this entry is in flight.
    pub fn settled(&self) -> Settled {
        self.link.settled()
    }

    /// Binds the seven signals to a store cell, `data` as a keyed list.
    pub(crate) fn make_cell(&self) -> Result<Arc<StoreCell>, SignalsError> {
        if self.cell.get().is_some() {
            return Err(SignalsError::AlreadyAttached);
        }
        let cell = StoreCell::new(Q::ID);
        cell.attach_keyed(&self.inner.data, 0, Q::item_key)?;
        cell.attach(&self.inner.status, 1)?;
        cell.attach(&self.inner.error, 2)?;
        cell.attach(&self.inner.fetching, 3)?;
        cell.attach(&self.inner.updated_at, 4)?;
        cell.attach(&self.inner.has_next_page, 5)?;
        cell.attach(&self.inner.fetching_next_page, 6)?;
        let _ = self.cell.set(cell.clone());
        Ok(cell)
    }
}

impl<Q: InfiniteQueryDef> core::fmt::Debug for InfiniteHandle<Q> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InfiniteHandle")
            .field("query", &Q::KEY)
            .field("status", &self.inner.status.get())
            .field("rows", &self.inner.data.with(Vec::len))
            .field("has_next_page", &self.inner.has_next_page.get())
            .finish_non_exhaustive()
    }
}

impl<Q: InfiniteQueryDef> HandleOps for InfiniteHandle<Q> {
    fn refetch(&self) {
        InfiniteHandle::refetch(self);
    }

    fn invalidate(&self) {
        InfiniteHandle::invalidate(self);
    }

    fn set_poll_interval(&self, interval: Option<Duration>) {
        InfiniteHandle::set_poll_interval(self, interval);
    }

    fn fetch_next_page(&self) -> bool {
        InfiniteHandle::fetch_next_page(self);
        true
    }

    fn make_cell(&self) -> Result<Arc<StoreCell>, SignalsError> {
        InfiniteHandle::make_cell(self)
    }
}
