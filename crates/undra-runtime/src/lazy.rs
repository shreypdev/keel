//! Lazy lists: lists a store exposes by handle so platforms page through them instead of receiving
//! them whole (SPEC 2.1 `Lazy<T>`, 3.3 target 3, 3.5 ops 0 and 2; ADR-043 decision 3).
//!
//! The runtime serves `LazyPage { handle, offset, limit }` calls itself, without a generated
//! dispatcher, from any [`LazySource`] (`undra_signals`) registered in its object table:
//!
//! * a store's `Lazy<T>` signal (and a [`Lazy::over`](undra_signals::Lazy::over) view): the runtime
//!   registers one page server per lazy signal when the store enters the table, as a transient entry
//!   that lives and dies with the store, and tells the store's cell the handle (so `observe` can
//!   send it in `LazyValue`);
//! * a [`LazyList`] of pre-encoded items, for a core that builds a list by hand
//!   ([`Runtime::insert_lazy_list`](crate::Runtime::insert_lazy_list)).
//!
//! The reply of a page call is `version u64, total u32, count u32`, then `count` items, each encoded
//! as the item type: the `version` and `total` the page was read at, atomically with the items.
//! Every argument comes from the host and is treated as hostile: an `offset` past the end gives
//! `count = 0`, a `limit` above [`MAX_PAGE_ITEMS`] is cut to it (so no call makes the core encode
//! more than that many items), and a handle that is stale, foreign or not a lazy list is a typed
//! bad request.
//!
//! ```
//! use undra_runtime::LazyList;
//!
//! let list = LazyList::new();
//! for n in [10_i32, 20, 30] {
//!     list.push_encoded(&n);
//! }
//! // version, total, count, then the items: a host pages with (offset, limit).
//! let reply = list.page(1, 5);
//! assert_eq!(&reply[..8], &3_u64.to_le_bytes()); // the list was changed three times
//! assert_eq!(&reply[8..16], &[3, 0, 0, 0, 2, 0, 0, 0]); // total 3, count 2
//! assert_eq!(&reply[16..], &[20, 0, 0, 0, 30, 0, 0, 0]);
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;
use undra_signals::{LazySource, page_window};
use undra_wire::payload::LazyPage;
use undra_wire::{Encode, Writer};

use crate::object::{AnyObject, UndraObject, erased};

/// The most items one page call returns: a `limit` above it is cut to it. A host asks for a window
/// of what it shows (tens of rows); the cap only keeps a hostile or buggy call from making the core
/// encode a whole list into one reply.
pub const MAX_PAGE_ITEMS: u32 = 4096;

/// The type id and name of a registered page server (and of a [`LazyList`]): what the object table
/// reports for a lazy handle.
pub(crate) const PAGE_SERVER_NAME: &str = "LazyList";

/// A page server in the object table: the type-erased list a `LazyPage` call is answered from.
pub(crate) struct PageServer {
    pub(crate) source: Arc<dyn LazySource>,
}

/// The object-table entry of a page server for `source`.
pub(crate) fn page_server(source: Arc<dyn LazySource>) -> Arc<dyn AnyObject> {
    erased(
        Arc::new(PageServer { source }),
        <LazyList as UndraObject>::TYPE_ID,
        PAGE_SERVER_NAME,
        None,
    )
}

/// Answers a page call: `version u64, total u32, count u32`, then the items (`limit` is cut to
/// [`MAX_PAGE_ITEMS`]; the source clamps the window to the list).
pub(crate) fn page_reply(source: &dyn LazySource, offset: u32, limit: u32) -> Vec<u8> {
    let limit = limit.min(MAX_PAGE_ITEMS);
    // The header is patched in once the source says which version and total the items were read at;
    // room for a typical window (a few dozen rows) so that filling it does not reallocate.
    let mut w = Writer::with_capacity(16 + 48 * limit.min(64) as usize);
    w.write_raw(&[0; 16]);
    let header = source.encode_page(offset, limit, &mut w);
    let mut reply = w.into_vec();
    let mut head = Writer::with_capacity(16);
    header.encode(&mut head);
    reply[..16].copy_from_slice(head.as_slice());
    reply
}

/// The shared state of a [`LazyList`].
#[derive(Debug, Default)]
pub struct LazyListInner {
    items: RwLock<Vec<Vec<u8>>>,
    version: AtomicU64,
}

/// A list of pre-encoded items that platforms page through by handle: the simplest
/// [`LazySource`], for a core that builds a list by hand (a typed, reactive list is a store's
/// `Lazy<T>`).
///
/// Cloning gives another view of the same list: a core keeps one clone to mutate and hands another
/// to [`Runtime::insert_lazy_list`](crate::Runtime::insert_lazy_list). Mutations bump
/// [`version`](LazyList::version); the owning store is responsible for telling the host (a
/// `LazyInvalidated` change-set entry) so it re-pages.
#[derive(Clone, Debug, Default)]
pub struct LazyList {
    inner: Arc<LazyListInner>,
}

impl UndraObject for LazyList {
    const TYPE_ID: u32 = undra_meta::ids::type_id("LazyList");
    const NAME: &'static str = PAGE_SERVER_NAME;
}

impl LazyList {
    /// Creates an empty list.
    pub fn new() -> LazyList {
        LazyList::default()
    }

    /// Creates a list from already-encoded items.
    pub fn from_items(items: Vec<Vec<u8>>) -> LazyList {
        let list = LazyList::default();
        *list.inner.items.write() = items;
        list
    }

    fn bump(&self) {
        self.inner.version.fetch_add(1, Ordering::AcqRel);
    }

    /// Appends one already-encoded item.
    pub fn push(&self, item: Vec<u8>) {
        self.inner.items.write().push(item);
        self.bump();
    }

    /// Encodes `item` and appends it.
    pub fn push_encoded<T: Encode + ?Sized>(&self, item: &T) {
        let mut w = Writer::new();
        item.encode(&mut w);
        self.push(w.into_vec());
    }

    /// Replaces every item.
    pub fn replace(&self, items: Vec<Vec<u8>>) {
        *self.inner.items.write() = items;
        self.bump();
    }

    /// Removes every item.
    pub fn clear(&self) {
        self.inner.items.write().clear();
        self.bump();
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.inner.items.read().len()
    }

    /// Whether the list has no items.
    pub fn is_empty(&self) -> bool {
        self.inner.items.read().is_empty()
    }

    /// Incremented by every mutation.
    pub fn version(&self) -> u64 {
        self.inner.version.load(Ordering::Acquire)
    }

    /// The reply to a page call for `offset` and `limit`: `version u64, total u32, count u32`, then
    /// the items exactly as pushed (SPEC 3.3). An `offset` at or past the end yields `count = 0`;
    /// `limit` is cut to [`MAX_PAGE_ITEMS`].
    pub fn page(&self, offset: u32, limit: u32) -> Vec<u8> {
        page_reply(self, offset, limit)
    }
}

impl LazySource for LazyList {
    fn stamp(&self) -> (usize, u64) {
        // Under the read lock, as every writer bumps the version under the write lock.
        let items = self.inner.items.read();
        (items.len(), self.inner.version.load(Ordering::Acquire))
    }

    fn encode_page(&self, offset: u32, limit: u32, out: &mut Writer) -> LazyPage {
        let items = self.inner.items.read();
        let window = page_window(items.len(), offset, limit);
        for item in &items[window.clone()] {
            out.write_raw(item);
        }
        LazyPage {
            version: self.inner.version.load(Ordering::Acquire),
            total: u32::try_from(items.len()).unwrap_or(u32::MAX),
            count: u32::try_from(window.len()).unwrap_or(u32::MAX),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: u8) -> LazyList {
        LazyList::from_items((0..n).map(|i| vec![i, i]).collect())
    }

    /// `version`, `total`, `count` of a reply, and its items.
    fn parts(reply: &[u8]) -> ((u64, u32, u32), &[u8]) {
        let version = u64::from_le_bytes(reply[..8].try_into().unwrap());
        let total = u32::from_le_bytes(reply[8..12].try_into().unwrap());
        let count = u32::from_le_bytes(reply[12..16].try_into().unwrap());
        ((version, total, count), &reply[16..])
    }

    #[test]
    fn pages_are_windows_over_the_items() {
        let l = list(5);
        assert_eq!(parts(&l.page(0, 2)), ((0, 5, 2), &[0, 0, 1, 1][..]));
        assert_eq!(parts(&l.page(3, 10)), ((0, 5, 2), &[3, 3, 4, 4][..]));
        assert_eq!(parts(&l.page(4, 1)), ((0, 5, 1), &[4, 4][..]));
    }

    #[test]
    fn empty_and_out_of_range_pages_report_the_total() {
        assert_eq!(parts(&LazyList::new().page(0, 10)), ((0, 0, 0), &[][..]));
        assert_eq!(parts(&list(3).page(3, 1)), ((0, 3, 0), &[][..]));
        assert_eq!(
            parts(&list(3).page(u32::MAX, u32::MAX)),
            ((0, 3, 0), &[][..])
        );
        assert_eq!(parts(&list(3).page(1, 0)), ((0, 3, 0), &[][..]));
    }

    #[test]
    fn limit_at_the_extremes_does_not_overflow_and_is_capped() {
        let l = list(2);
        assert_eq!(parts(&l.page(1, u32::MAX)), ((0, 2, 1), &[1, 1][..]));
        // A list longer than the cap: no call returns more than `MAX_PAGE_ITEMS` items.
        let big = LazyList::from_items((0..MAX_PAGE_ITEMS + 10).map(|_| vec![7]).collect());
        let reply = big.page(0, u32::MAX);
        let ((_, total, count), rows) = parts(&reply);
        assert_eq!((total, count), (MAX_PAGE_ITEMS + 10, MAX_PAGE_ITEMS));
        assert_eq!(rows.len(), MAX_PAGE_ITEMS as usize);
        let reply = big.page(MAX_PAGE_ITEMS, u32::MAX);
        assert_eq!(parts(&reply).0.2, 10);
    }

    #[test]
    fn mutations_bump_the_version_and_clones_share_state() {
        let l = LazyList::new();
        let view = l.clone();
        assert_eq!(view.version(), 0);
        l.push(vec![1]);
        l.push_encoded(&7_u8);
        assert_eq!(view.len(), 2);
        assert_eq!(view.version(), 2);
        l.replace(vec![vec![9]]);
        assert_eq!((view.len(), view.version()), (1, 3));
        assert_eq!(view.stamp(), (1, 3));
        l.clear();
        assert!(view.is_empty());
        assert_eq!(view.version(), 4);
        // The page says which version it was read at.
        assert_eq!(parts(&view.page(0, 1)).0, (4, 0, 0));
    }

    #[test]
    fn variable_length_items_are_concatenated_verbatim() {
        let l = LazyList::new();
        l.push_encoded("ab");
        l.push_encoded("");
        assert_eq!(
            parts(&l.page(0, 2)),
            ((2, 2, 2), &[2, 0, 0, 0, b'a', b'b', 0, 0, 0, 0][..])
        );
    }
}
