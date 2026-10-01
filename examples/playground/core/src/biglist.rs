//! The big list: 10,000 keyed items and one-item operations on them.
//!
//! This store is the proof that lists cost O(change), not O(list) (blueprint section 14): the
//! list is a keyed signal, so inserting, updating, moving or removing one item crosses the
//! boundary as a keyed patch of a single operation, however long the list is. Only observing the
//! store ships the whole list, once.

use std::sync::atomic::{AtomicU32, Ordering};

use undra::prelude::*;

/// How many items a new list has.
pub const LIST_LEN: u32 = 10_000;

/// One row of a list, identified by `id`.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Identity of the row; lists are patched by it, never by position.
    pub id: u32,
    /// The text of the row.
    pub label: String,
    /// How many times the row was updated.
    pub version: u32,
}

impl Item {
    /// The `id`th row of a fresh list.
    pub(crate) fn numbered(id: u32) -> Item {
        Item {
            id,
            label: format!("Item {id}"),
            version: 0,
        }
    }
}

/// Why an operation on a list was refused.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListError {
    /// A position is not in the list.
    #[error("position {index} is outside a list of {len} items")]
    OutOfRange {
        /// The position that was asked for.
        index: u32,
        /// How many items the list had.
        len: u32,
    },
}

/// A list of [`LIST_LEN`] items with operations that change one item at a time.
#[undra::store(restore = "Self::assemble")]
pub struct BigList {
    next_id: AtomicU32,
    #[undra(key = "id")]
    items: Signal<Vec<Item>>,
    count: Computed<u32>,
}

#[undra::api(store)]
impl BigList {
    /// A list of [`LIST_LEN`] items numbered from 1.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(
            ctx,
            Signal::new((1..=LIST_LEN).map(Item::numbered).collect()),
        )
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot.
    fn assemble(_ctx: Ctx, items: Signal<Vec<Item>>) -> Self {
        let count = Computed::new(&items, |items| items.len() as u32);
        let next = items.with(|list| list.iter().map(|item| item.id).max().unwrap_or(0));
        Self {
            next_id: AtomicU32::new(next.saturating_add(1)),
            items,
            count,
        }
    }

    /// Inserts a new row with `label` so that it ends at `index` (`index == len` appends), and
    /// returns its identity. One keyed `Insert`.
    pub fn insert_at(&self, index: u32, label: String) -> Result<u32, ListError> {
        self.items.with(|list| {
            // One past the last row is fine (it appends); the error reports the real length.
            if index as usize > list.len() {
                return Err(ListError::OutOfRange {
                    index,
                    len: list.len() as u32,
                });
            }
            Ok(())
        })?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // The recorded list ops keep the change-set O(change) (ADR-027).
        self.items.insert(
            index as usize,
            Item {
                id,
                label,
                version: 0,
            },
        );
        Ok(id)
    }

    /// Changes the label of the row at `index` and bumps its version. One keyed `Update`.
    pub fn update_at(&self, index: u32, label: String) -> Result<(), ListError> {
        self.items.with(|list| check(index, list.len()))?;
        self.items.update_at(index as usize, |item| {
            item.label = label;
            item.version += 1;
        });
        Ok(())
    }

    /// Moves the row at `from` so that it ends at `to`. One keyed `Move`.
    pub fn move_item(&self, from: u32, to: u32) -> Result<(), ListError> {
        self.items.with(|list| {
            check(from, list.len())?;
            check(to, list.len())
        })?;
        self.items.move_item(from as usize, to as usize);
        Ok(())
    }

    /// Removes the row at `index`. One keyed `Remove`.
    pub fn remove_at(&self, index: u32) -> Result<(), ListError> {
        self.items.with(|list| check(index, list.len()))?;
        self.items.remove(index as usize);
        Ok(())
    }

    /// Replaces the list with a fresh one of [`LIST_LEN`] items. The platform receives the keyed
    /// patch that turns the old list into the new one (a full value if they have little in
    /// common).
    pub fn reset(&self) {
        self.next_id.store(LIST_LEN + 1, Ordering::Relaxed);
        self.items.set((1..=LIST_LEN).map(Item::numbered).collect());
    }
}

/// `Ok` when `index < len`.
fn check(index: u32, len: usize) -> Result<(), ListError> {
    if (index as usize) < len {
        Ok(())
    } else {
        Err(ListError::OutOfRange {
            index,
            len: len as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use undra::meta::ids;
    use undra::runtime::testing::TestRuntime;
    use undra::signals::ALL_SIGNALS;
    use undra::wire::payload::{CallTarget, ChangeOp, ChangeSet, ReplyStatus};
    use undra::wire::{Decode, Encode, KeyedPatch, PatchOp, Reader};

    use super::*;

    const ITEMS: u32 = 0;
    const COUNT: u32 = 1;

    struct App {
        t: TestRuntime,
        store: Handle,
    }

    impl App {
        /// A list that is being observed; the initial change-set was consumed.
        fn new() -> App {
            let t = TestRuntime::new();
            let reply = t.call_sync(
                CallTarget::Constructor {
                    type_id: ids::type_id("BigList"),
                    method_id: ids::method_id("BigList", "new"),
                },
                1,
                &[],
            );
            assert_eq!(reply.status, ReplyStatus::Ok);
            let store = Handle::decode_exact(&reply.body).unwrap();
            t.take_change_sets();
            t.runtime().observe(store.0, ALL_SIGNALS, true);
            let initial = t.host().take_decoded_change_sets();
            assert_eq!(initial.len(), 1);
            let items = Vec::<Item>::decode_exact(&initial[0].entries[0].value).unwrap();
            assert_eq!(items.len(), LIST_LEN as usize);
            assert_eq!(items[0], Item::numbered(1));
            assert_eq!(items[9_999], Item::numbered(10_000));
            App { t, store }
        }

        fn call(&self, method: &str, args: &[u8]) -> (ReplyStatus, Vec<u8>) {
            let reply = self.t.call_sync(
                CallTarget::Method {
                    handle: self.store,
                    method_id: ids::method_id("BigList", method),
                },
                2,
                args,
            );
            (reply.status, reply.body)
        }

        /// Calls a method that must succeed and returns the single change-set it caused.
        fn change(&self, method: &str, args: &[u8]) -> ChangeSet {
            let (status, body) = self.call(method, args);
            assert_eq!(status, ReplyStatus::Ok, "BigList.{method}: {body:?}");
            let mut sets = self.t.host().take_decoded_change_sets();
            assert_eq!(sets.len(), 1, "one call, one change-set: {sets:?}");
            sets.remove(0)
        }

        /// The keyed patch the change-set carries for `items`.
        fn patch(cs: &ChangeSet) -> KeyedPatch<Item> {
            let entry = cs
                .entries
                .iter()
                .find(|e| e.signal_id == ITEMS)
                .expect("items");
            assert_eq!(
                entry.op,
                ChangeOp::KeyedPatch,
                "expected a patch, not the whole list"
            );
            let mut r = Reader::new(&entry.value);
            let patch = KeyedPatch::<Item>::decode(&mut r).unwrap();
            r.finish().unwrap();
            patch
        }
    }

    fn args2(a: u32, b: u32) -> Vec<u8> {
        let mut bytes = a.encode_to_vec();
        bytes.extend(b.encode_to_vec());
        bytes
    }

    #[test]
    fn an_insert_is_one_patch_operation_on_ten_thousand_items() {
        let app = App::new();
        let mut args = 5_000u32.encode_to_vec();
        args.extend("fresh".to_owned().encode_to_vec());
        let cs = app.change("insert_at", &args);
        let patch = App::patch(&cs);
        assert_eq!(patch.ops.len(), 1);
        assert!(matches!(
            &patch.ops[0],
            PatchOp::Insert { index: 5_000, item } if item.label == "fresh" && item.id == 10_001
        ));
        // The encoded entry is a few dozen bytes, not the 200 KB list.
        let items_entry = cs.entries.iter().find(|e| e.signal_id == ITEMS).unwrap();
        assert!(
            items_entry.value.len() < 64,
            "{} bytes",
            items_entry.value.len()
        );
        // The computed length rode along in the same change-set.
        let count = cs.entries.iter().find(|e| e.signal_id == COUNT).unwrap();
        assert_eq!(count.value, 10_001u32.to_le_bytes());
    }

    #[test]
    fn an_update_is_one_update_operation() {
        let app = App::new();
        let mut args = 42u32.encode_to_vec();
        args.extend("renamed".to_owned().encode_to_vec());
        let patch = App::patch(&app.change("update_at", &args));
        assert_eq!(patch.ops.len(), 1);
        assert!(matches!(
            &patch.ops[0],
            PatchOp::Update { index: 42, item } if item.label == "renamed" && item.version == 1 && item.id == 43
        ));
    }

    #[test]
    fn a_move_is_one_move_operation() {
        let app = App::new();
        let patch = App::patch(&app.change("move_item", &args2(10, 9_000)));
        assert_eq!(
            patch.ops,
            [PatchOp::Move {
                from: 10,
                to: 9_000
            }]
        );
    }

    #[test]
    fn a_remove_is_one_remove_operation() {
        let app = App::new();
        let patch = App::patch(&app.change("remove_at", &0u32.encode_to_vec()));
        assert_eq!(patch.ops, [PatchOp::Remove { index: 0 }]);
    }

    #[test]
    fn positions_outside_the_list_are_typed_errors() {
        let app = App::new();
        let (status, body) = app.call("remove_at", &10_000u32.encode_to_vec());
        assert_eq!(status, ReplyStatus::Error);
        assert_eq!(
            ListError::decode_exact(&body).unwrap(),
            ListError::OutOfRange {
                index: 10_000,
                len: 10_000
            }
        );
        let (status, _) = app.call("move_item", &args2(0, 10_000));
        assert_eq!(status, ReplyStatus::Error);
        // Appending is an insert at `len`; one past that is an error that reports the real length.
        let mut args = 10_001u32.encode_to_vec();
        args.extend("x".to_owned().encode_to_vec());
        let (status, body) = app.call("insert_at", &args);
        assert_eq!(status, ReplyStatus::Error);
        assert_eq!(
            ListError::decode_exact(&body).unwrap(),
            ListError::OutOfRange {
                index: 10_001,
                len: 10_000
            }
        );
        // None of the refused calls changed anything...
        assert!(app.t.host().take_decoded_change_sets().is_empty());
        // ...and an insert at `len` appends.
        let mut append = 10_000u32.encode_to_vec();
        append.extend("x".to_owned().encode_to_vec());
        assert_eq!(app.call("insert_at", &append).0, ReplyStatus::Ok);
    }

    #[test]
    fn a_reset_is_the_patch_between_the_old_list_and_the_fresh_one() {
        let app = App::new();
        app.change("remove_at", &0u32.encode_to_vec());
        let patch = App::patch(&app.change("reset", &[]));
        assert_eq!(
            patch.ops,
            [PatchOp::Insert {
                index: 0,
                item: Item::numbered(1)
            }]
        );
    }

    #[test]
    fn identities_are_never_reused() {
        let app = App::new();
        app.change("remove_at", &9_999u32.encode_to_vec());
        let mut args = 0u32.encode_to_vec();
        args.extend("a".to_owned().encode_to_vec());
        let (status, body) = app.call("insert_at", &args);
        assert_eq!(status, ReplyStatus::Ok);
        assert_eq!(
            u32::decode_exact(&body).unwrap(),
            10_001,
            "10_000 was removed but stays spent"
        );
    }
}
