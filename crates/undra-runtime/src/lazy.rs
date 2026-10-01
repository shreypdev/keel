//! Lazy lists: a list a store exposes by handle so platforms can page through it instead of
//! receiving it whole (SPEC 2.1 `Lazy<T>`, 3.3 target 3, 3.5 op 2).
//!
//! The runtime owns the object behind a lazy-list handle: it keeps the items **already
//! encoded** (each one a complete value of the list's item type) and answers
//! `LazyPage { handle, offset, limit }` calls itself, without a generated dispatcher.
//!
//! ```
//! use undra_runtime::LazyList;
//!
//! let list = LazyList::new();
//! for n in [10_i32, 20, 30] {
//!     list.push_encoded(&n);
//! }
//! // total, count, then the items: a host pages with (offset, limit).
//! let page = list.page(1, 5);
//! assert_eq!(&page[..8], &[3, 0, 0, 0, 2, 0, 0, 0]);
//! assert_eq!(&page[8..], &[20, 0, 0, 0, 30, 0, 0, 0]);
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;
use undra_wire::{Encode, Writer};

use crate::object::UndraObject;

/// The shared state of a [`LazyList`].
#[derive(Debug, Default)]
pub struct LazyListInner {
    items: RwLock<Vec<Vec<u8>>>,
    version: AtomicU64,
}

/// A list of pre-encoded items that platforms page through by handle.
///
/// Cloning gives another view of the same list: a store keeps one clone to mutate and hands
/// another to [`Runtime::insert_lazy_list`](crate::Runtime::insert_lazy_list). Mutations bump
/// [`version`](LazyList::version); the owning store is responsible for telling the host (a
/// `LazyInvalidated` change-set entry) so it re-pages.
#[derive(Clone, Debug, Default)]
pub struct LazyList {
    inner: Arc<LazyListInner>,
}

impl UndraObject for LazyList {
    const TYPE_ID: u32 = undra_meta::ids::type_id("LazyList");
    const NAME: &'static str = "LazyList";
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

    /// Encodes the page starting at `offset` with at most `limit` items:
    /// `total u32, count u32, items...` (the items exactly as pushed). An `offset` at or past
    /// the end yields `count = 0`.
    pub fn page(&self, offset: u32, limit: u32) -> Vec<u8> {
        let items = self.inner.items.read();
        let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
        let start = (offset as usize).min(items.len());
        let end = start.saturating_add(limit as usize).min(items.len());
        let window = &items[start..end];
        let body: usize = window.iter().map(Vec::len).sum();
        let mut w = Writer::with_capacity(8 + body);
        w.write_u32(total);
        w.write_u32(u32::try_from(window.len()).unwrap_or(u32::MAX));
        for item in window {
            w.write_raw(item);
        }
        w.into_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: u8) -> LazyList {
        LazyList::from_items((0..n).map(|i| vec![i, i]).collect())
    }

    #[test]
    fn pages_are_windows_over_the_items() {
        let l = list(5);
        assert_eq!(l.page(0, 2), [5, 0, 0, 0, 2, 0, 0, 0, 0, 0, 1, 1]);
        assert_eq!(l.page(3, 10), [5, 0, 0, 0, 2, 0, 0, 0, 3, 3, 4, 4]);
        assert_eq!(l.page(4, 1), [5, 0, 0, 0, 1, 0, 0, 0, 4, 4]);
    }

    #[test]
    fn empty_and_out_of_range_pages_report_the_total() {
        assert_eq!(LazyList::new().page(0, 10), [0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(list(3).page(3, 1), [3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(list(3).page(u32::MAX, u32::MAX), [3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(list(3).page(1, 0), [3, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn limit_at_the_extremes_does_not_overflow() {
        let l = list(2);
        assert_eq!(l.page(1, u32::MAX), [2, 0, 0, 0, 1, 0, 0, 0, 1, 1]);
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
        l.clear();
        assert!(view.is_empty());
        assert_eq!(view.version(), 4);
    }

    #[test]
    fn variable_length_items_are_concatenated_verbatim() {
        let l = LazyList::new();
        l.push_encoded("ab");
        l.push_encoded("");
        assert_eq!(
            l.page(0, 2),
            [2, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0, b'a', b'b', 0, 0, 0, 0]
        );
    }
}
