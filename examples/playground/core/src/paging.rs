//! Paging and polling (ADR-043): a list the host pages through, an infinite feed and a polled value.
//!
//! * [`Library`] is a store with two `Lazy<Item>` lists. `books` owns [`LIBRARY_LEN`] rows, and no
//!   platform ever receives them whole: it learns the length and the version and asks the core for a
//!   page when it needs one. `evens` is a read-only view of the even rows of `source` (a small list the
//!   platform does receive), paged through the derived index.
//! * [`feed`] is an infinite query over the rows of [`BigList`](crate::biglist): pages of [`PAGE`]
//!   items, a cursor, a keyed list on the platform that grows by the page.
//! * [`ticker`] is a query that polls: a counter the core bumps on every fetch, refetched every second
//!   while somebody watches it and the app is active.
//!
//! Nothing here reads a clock or a random source (R12): rows are numbered, "latency" is
//! `Ctx::sleep`, and the poll interval is the query's own.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use undra::prelude::*;
use undra::query::Page;

use crate::biglist::{Item, LIST_LEN, ListError};

/// How many rows `books` has in a fresh [`Library`].
pub const LIBRARY_LEN: u32 = 10_000;

/// How many rows one page of the [`feed`] has.
pub const PAGE: u32 = 50;

/// A library of books the host pages through.
#[undra::store(restore = "Self::assemble")]
pub struct Library {
    next_id: AtomicU32,
    /// The rows; the host never receives them whole.
    #[undra(key = "id")]
    books: Lazy<Item>,
    /// A small list the host does receive, and the source of `evens`.
    #[undra(key = "id")]
    source: Signal<Vec<Item>>,
    /// The rows of `source` whose id is even: a read-only lazy view of a derived list.
    #[undra(key = "id")]
    evens: Lazy<Item>,
}

#[undra::api(store)]
impl Library {
    /// A library with [`LIBRARY_LEN`] books and a source of 20 rows.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(
            ctx,
            Lazy::from_vec((1..=LIBRARY_LEN).map(Item::numbered).collect()),
            Signal::new((1..=20).map(Item::numbered).collect()),
            Lazy::new(),
        )
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot: the view is
    // derived data, so the empty list a snapshot holds for it is ignored and it is built again.
    fn assemble(
        _ctx: Ctx,
        books: Lazy<Item>,
        source: Signal<Vec<Item>>,
        _view: Lazy<Item>,
    ) -> Self {
        let evens = Lazy::over(
            &source
                .derive()
                .filter(|item: &Item| item.id % 2 == 0)
                .build(),
        );
        let next = books
            .with_range(.., |rows| {
                rows.iter().map(|item| item.id).max().unwrap_or(0)
            })
            .max(source.with(|rows| rows.iter().map(|item| item.id).max().unwrap_or(0)));
        Self {
            next_id: AtomicU32::new(next.saturating_add(1)),
            books,
            source,
            evens,
        }
    }

    /// How many books there are.
    pub fn count(&self) -> u32 {
        self.books.len() as u32
    }

    /// Appends `count` books and, in the same transaction, as many rows to `source`. Each is one
    /// recorded push, and the host is told one new length and version.
    pub fn add_rows(&self, count: u32) {
        txn(|| {
            for _ in 0..count {
                let item = Item::numbered(self.next_id.fetch_add(1, Ordering::Relaxed));
                self.books.push(item.clone());
                self.source.push(item);
            }
        });
    }

    /// Changes the label of the book at `index` and bumps its version.
    pub fn rename(&self, index: u32, label: String) -> Result<(), ListError> {
        let len = self.books.len();
        if index as usize >= len {
            return Err(ListError::OutOfRange {
                index,
                len: len as u32,
            });
        }
        self.books.update_at(index as usize, |item| {
            item.label = label;
            item.version += 1;
        });
        Ok(())
    }

    /// Removes the book at `index`.
    pub fn remove_at(&self, index: u32) -> Result<(), ListError> {
        let len = self.books.len();
        if index as usize >= len {
            return Err(ListError::OutOfRange {
                index,
                len: len as u32,
            });
        }
        self.books.remove(index as usize);
        Ok(())
    }

    /// Replaces the books with `count` fresh ones numbered from 1.
    pub fn reset(&self, count: u32) {
        self.books
            .replace((1..=count).map(Item::numbered).collect());
    }

    /// Removes `count` rows from the start of `source` (the lazy view follows).
    pub fn drop_source(&self, count: u32) {
        txn(|| {
            for _ in 0..count {
                if self.source.with(|rows| rows.is_empty()) {
                    break;
                }
                self.source.remove(0);
            }
        });
    }
}

/// What the feed and the ticker share in one runtime.
#[derive(Default)]
struct PagingState {
    /// Bumped by [`touch_feed`]: rows with an even id show it in their label, so a refetch has a
    /// change to find.
    revision: AtomicU32,
    /// How many times [`ticker`] fetched.
    ticks: AtomicU32,
    /// Whether the next fetch of [`ticker`] fails.
    failing: Mutex<bool>,
}

fn state(ctx: &Ctx) -> &PagingState {
    ctx.runtime().extension::<PagingState>()
}

/// The rows of the feed that start after `cursor` (`"<last id>@<revision>"`, none for the first
/// page): [`PAGE`] rows of the big list's rows, only the even ones when `even_only`. The cursor
/// carries the revision, so a refetch re-chains cursors the way a server's would change.
#[undra::query(key = "feed/{even_only}", infinite, item_key = "id", stale = "1m")]
pub async fn feed(
    ctx: &Ctx,
    even_only: bool,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<Page<Item, String>, ListError> {
    // A little latency, through the Timer port, so a fetch is observable while it runs.
    ctx.sleep(std::time::Duration::from_millis(20)).await;
    let revision = state(ctx).revision.load(Ordering::Relaxed);
    let after: u32 = cursor
        .as_deref()
        .and_then(|c| c.split('@').next())
        .and_then(|id| id.parse().ok())
        .unwrap_or(0);
    let mut items = Vec::with_capacity(PAGE as usize);
    let mut id = after;
    while items.len() < PAGE as usize && id < LIST_LEN {
        id += 1;
        if even_only && id % 2 == 1 {
            continue;
        }
        let mut item = Item::numbered(id);
        if id % 2 == 0 {
            item.version = revision;
        }
        items.push(item);
    }
    let next = (id < LIST_LEN).then(|| format!("{id}@{revision}"));
    Ok(Page { items, next })
}

/// Makes the feed's even rows show revision `revision` the next time they are fetched.
#[undra::api]
pub fn touch_feed(ctx: &Ctx, revision: u32) {
    state(ctx).revision.store(revision, Ordering::Relaxed);
}

/// Why the ticker could not tick.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TickError {
    /// [`set_ticker_failing`] made it fail.
    #[error("the ticker is failing on purpose")]
    Failing,
}

/// A counter that goes up by one on every fetch. It polls: while somebody watches it and the app is
/// active it is fetched again a second after the last fetch ended.
#[undra::query(key = "ticker", interval = "1s", retry = 0)]
pub async fn ticker(ctx: &Ctx) -> Result<u32, TickError> {
    let state = state(ctx);
    let ticks = state.ticks.fetch_add(1, Ordering::Relaxed) + 1;
    if *state
        .failing
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return Err(TickError::Failing);
    }
    Ok(ticks)
}

/// How many times the ticker fetched, for a test that wants the number the core sees.
#[undra::api]
pub fn ticker_fetches(ctx: &Ctx) -> u32 {
    state(ctx).ticks.load(Ordering::Relaxed)
}

/// Makes the ticker fail (or succeed again) from its next fetch on.
#[undra::api]
pub fn set_ticker_failing(ctx: &Ctx, failing: bool) {
    *state(ctx)
        .failing
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = failing;
}
