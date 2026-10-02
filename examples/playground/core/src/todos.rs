//! The to-do list: a store with a keyed list, a filter, a derived list and a count.
//!
//! This is the screen every playground app shows first. `todos` is keyed by `id` and written with
//! the recorded list operations (`push`, `update_at`, `remove`), so the core ships an insert, a
//! remove or an update as a one-op keyed patch (SPEC 3.8). `visible` is a [`DerivedList`] of it
//! (ADR-039): kept current from those same operations, it reaches the UI as keyed patches too, one
//! op per changed row however long the list is, and a change the filter hides sends nothing.
//! `remaining` is the length of another view, kept as rows come and go with no scan of the list.

use std::sync::atomic::{AtomicU64, Ordering};

use undra::prelude::*;

/// One item of the to-do list.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    /// Identity of the item; the list is updated by key, so the UI diffs by it.
    pub id: Uuid,
    /// What has to be done.
    pub title: String,
    /// Whether it is finished.
    pub done: bool,
}

/// Which items the list shows.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Every item.
    All,
    /// Items that are not finished.
    Active,
    /// Items that are finished.
    Done,
}

impl Filter {
    /// Whether `todo` passes this filter.
    fn matches(self, todo: &Todo) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => !todo.done,
            Filter::Done => todo.done,
        }
    }
}

/// Why an item could not be added.
#[undra::error]
#[derive(Clone, Debug, PartialEq)]
pub enum TodoError {
    /// The title is empty once spaces are trimmed.
    #[error("the title cannot be empty")]
    EmptyTitle,
}

/// The to-do list: what the UI observes (`todos`, `filter`, `visible`, `remaining`) and calls.
#[undra::store(restore = "Self::assemble")]
pub struct Todos {
    next: AtomicU64,
    #[undra(key = "id")]
    todos: Signal<Vec<Todo>>,
    filter: Signal<Filter>,
    #[undra(key = "id")]
    visible: DerivedList<Todo>,
    remaining: Computed<u32>,
}

#[undra::api(store)]
impl Todos {
    /// An empty list showing every item.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(vec![]), Signal::new(Filter::All))
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot.
    fn assemble(_ctx: Ctx, todos: Signal<Vec<Todo>>, filter: Signal<Filter>) -> Self {
        // Kept current from the list's recorded operations: one toggle is one op at 10 or 100,000
        // rows. A filter change walks the list once and sends what entered and left (or the
        // whole view).
        let visible = todos
            .derive()
            .filter_with(&filter, |filter, todo| filter.matches(todo))
            .build();
        // Only the length of a view is needed here: kept as rows come and go, no scan per change.
        let remaining = todos.derive().filter(|todo| !todo.done).count();
        // Identities come from a counter (the core reads no clock and no random source): after a
        // restore it continues above the largest identity the snapshot holds.
        let next = todos.with(|list| {
            list.iter()
                .map(|todo| counter_of(todo.id))
                .max()
                .unwrap_or(0)
        });
        Self {
            next: AtomicU64::new(next + 1),
            todos,
            filter,
            visible,
            remaining,
        }
    }

    /// Adds an item at the end of the list.
    pub async fn add(&self, title: String) -> Result<Todo, TodoError> {
        let title = title.trim().to_owned();
        if title.is_empty() {
            return Err(TodoError::EmptyTitle);
        }
        let todo = Todo {
            id: id_of(self.next.fetch_add(1, Ordering::Relaxed)),
            title,
            done: false,
        };
        // A recorded operation: one `Insert` for `todos`, and one for `visible` if it shows it.
        self.todos.push(todo.clone());
        Ok(todo)
    }

    /// Flips the `done` flag of the item with `id`; unknown ids are ignored.
    pub fn toggle(&self, id: Uuid) {
        if let Some(at) = self.position(id) {
            // `todos`: one `Update`; `visible`: an `Update`, an `Insert` or a `Remove`, or nothing.
            self.todos.update_at(at, |todo| todo.done = !todo.done);
        }
    }

    /// Removes the item with `id`; unknown ids are ignored.
    pub fn remove(&self, id: Uuid) {
        if let Some(at) = self.position(id) {
            self.todos.remove(at);
        }
    }

    /// Chooses which items `visible` holds.
    pub fn set_filter(&self, filter: Filter) {
        self.filter.set(filter);
    }

    /// Removes every finished item.
    pub fn clear_done(&self) {
        let done: Vec<usize> = self
            .todos
            .with(|list| (0..list.len()).rev().filter(|&at| list[at].done).collect());
        txn(|| {
            // Highest index first, so the indices still to remove do not move; one change-set.
            for at in done {
                self.todos.remove(at);
            }
        });
    }

    /// Appends `count` items titled `Item 1`, `Item 2`, .. with every fourth one finished, in one
    /// transaction (a demo and test helper: the big-list screen of the docs, contract scenario S19).
    /// A bulk load is a raw write, so `todos` and `visible` are sent as full values this once; the
    /// writes after it are patches again.
    pub fn fill(&self, count: u32) {
        let first = self.next.fetch_add(u64::from(count), Ordering::Relaxed);
        let items: Vec<Todo> = (0..u64::from(count))
            .map(|n| Todo {
                id: id_of(first + n),
                title: format!("Item {}", n + 1),
                done: n % 4 == 3,
            })
            .collect();
        self.todos.update(|list| list.extend(items));
    }

    /// Where the item with `id` is. O(n) in app code: finding a row by key is not the core's job
    /// (ADR-039 section 9).
    fn position(&self, id: Uuid) -> Option<usize> {
        self.todos
            .with(|list| list.iter().position(|todo| todo.id == id))
    }
}

/// The identity of the `n`th item: `n` in the first eight bytes, big-endian.
pub(crate) fn id_of(n: u64) -> Uuid {
    let mut id = [0; 16];
    id[..8].copy_from_slice(&n.to_be_bytes());
    Uuid(id)
}

/// The counter value inside an identity made by [`id_of`].
pub(crate) fn counter_of(id: Uuid) -> u64 {
    let mut n = [0; 8];
    n.copy_from_slice(&id.0[..8]);
    u64::from_be_bytes(n)
}

#[cfg(test)]
mod tests {
    use undra::meta::ids;
    use undra::runtime::testing::TestRuntime;
    use undra::signals::ALL_SIGNALS;
    use undra::wire::payload::{CallTarget, ChangeOp, ChangeSet, ReplyStatus};
    use undra::wire::{Decode, Encode, KeyedPatch, PatchOp, Reader};

    use super::*;

    const TODOS: u32 = 0;
    const FILTER: u32 = 1;
    const VISIBLE: u32 = 2;
    const REMAINING: u32 = 3;

    /// A runtime and the call ids a host would allocate.
    struct App {
        t: TestRuntime,
        next_call: std::cell::Cell<u32>,
    }

    impl App {
        fn new() -> App {
            App {
                t: TestRuntime::new(),
                next_call: std::cell::Cell::new(1),
            }
        }

        fn call_id(&self) -> u32 {
            let id = self.next_call.get();
            self.next_call.set(id + 1);
            id
        }

        fn store(&self) -> Handle {
            let reply = self.t.call_sync(
                CallTarget::Constructor {
                    type_id: ids::type_id("Todos"),
                    method_id: ids::method_id("Todos", "new"),
                },
                self.call_id(),
                &[],
            );
            assert_eq!(reply.status, ReplyStatus::Ok);
            Handle::decode_exact(&reply.body).expect("a handle")
        }

        fn sync(&self, store: Handle, method: &str, args: &[u8]) {
            let reply = self.t.call_sync(
                CallTarget::Method {
                    handle: store,
                    method_id: ids::method_id("Todos", method),
                },
                self.call_id(),
                args,
            );
            assert_eq!(reply.status, ReplyStatus::Ok, "Todos.{method}: {reply:?}");
        }

        /// Runs `Todos.add` to completion and returns the reply status and body.
        fn add(&self, store: Handle, title: &str) -> (ReplyStatus, Vec<u8>) {
            let id = self.call_id();
            let target = CallTarget::Method {
                handle: store,
                method_id: ids::method_id("Todos", "add"),
            };
            assert_eq!(
                self.t.call(target, id, &title.to_owned().encode_to_vec()),
                0
            );
            self.t.run_pending();
            let mut replies = self.t.take_replies();
            assert_eq!(replies.len(), 1, "{replies:?}");
            let reply = replies.remove(0);
            (reply.status, reply.body)
        }

        fn observe(&self, store: Handle) -> ChangeSet {
            self.t.take_change_sets();
            self.t.runtime().observe(store.0, ALL_SIGNALS, true);
            let mut sets = self.t.host().take_decoded_change_sets();
            assert_eq!(sets.len(), 1, "{sets:?}");
            sets.remove(0)
        }

        fn change_sets(&self) -> Vec<ChangeSet> {
            self.t.host().take_decoded_change_sets()
        }
    }

    fn value<T: Decode>(cs: &ChangeSet, signal: u32) -> T {
        let entry = cs
            .entries
            .iter()
            .find(|e| e.signal_id == signal)
            .unwrap_or_else(|| panic!("no entry for signal {signal} in {cs:?}"));
        T::decode_exact(&entry.value).expect("a value")
    }

    fn entry_op(cs: &ChangeSet, signal: u32) -> ChangeOp {
        cs.entries
            .iter()
            .find(|e| e.signal_id == signal)
            .map(|e| e.op)
            .unwrap_or_else(|| panic!("no entry for signal {signal}"))
    }

    fn signal_ids(cs: &ChangeSet) -> Vec<u32> {
        cs.entries.iter().map(|e| e.signal_id).collect()
    }

    /// The ops of `signal`'s keyed-patch entry.
    fn ops(cs: &ChangeSet, signal: u32) -> Vec<PatchOp<Todo>> {
        let entry = cs
            .entries
            .iter()
            .find(|e| e.signal_id == signal)
            .unwrap_or_else(|| panic!("no entry for signal {signal} in {cs:?}"));
        assert_eq!(entry.op, ChangeOp::KeyedPatch, "{cs:?}");
        let mut r = Reader::new(&entry.value);
        let patch = KeyedPatch::<Todo>::decode(&mut r).unwrap();
        r.finish().unwrap();
        patch.ops
    }

    /// What a platform holds of `visible`: full values replace it, patches apply to it.
    #[derive(Default)]
    struct Mirror(Vec<Todo>);

    impl Mirror {
        fn apply(&mut self, sets: &[ChangeSet]) {
            for cs in sets {
                for e in cs.entries.iter().filter(|e| e.signal_id == VISIBLE) {
                    match e.op {
                        ChangeOp::Full => self.0 = Vec::<Todo>::decode_exact(&e.value).unwrap(),
                        ChangeOp::KeyedPatch => {
                            let mut r = Reader::new(&e.value);
                            let patch = KeyedPatch::<Todo>::decode(&mut r).unwrap();
                            patch.apply(&mut self.0).unwrap();
                        }
                        other => panic!("{other:?}"),
                    }
                }
            }
        }
    }

    fn todo(n: u64, title: &str, done: bool) -> Todo {
        Todo {
            id: id_of(n),
            title: title.into(),
            done,
        }
    }

    #[test]
    fn observing_sends_every_signal_once() {
        let app = App::new();
        let store = app.store();
        let initial = app.observe(store);
        assert_eq!(signal_ids(&initial), [TODOS, FILTER, VISIBLE, REMAINING]);
        assert_eq!(value::<Vec<Todo>>(&initial, TODOS), []);
        assert_eq!(value::<Filter>(&initial, FILTER), Filter::All);
        assert_eq!(value::<Vec<Todo>>(&initial, VISIBLE), []);
        assert_eq!(value::<u32>(&initial, REMAINING), 0);
    }

    #[test]
    fn adding_is_one_insert_in_the_list_and_in_the_view() {
        let app = App::new();
        let store = app.store();
        app.observe(store);
        let (status, body) = app.add(store, "  Milk ");
        assert_eq!(status, ReplyStatus::Ok);
        let todo = Todo::decode_exact(&body).unwrap();
        assert_eq!((todo.title.as_str(), todo.done), ("Milk", false));

        let sets = app.change_sets();
        assert_eq!(sets.len(), 1, "one write, one change-set: {sets:?}");
        assert_eq!(signal_ids(&sets[0]), [TODOS, VISIBLE, REMAINING]);
        assert_eq!(value::<u32>(&sets[0], REMAINING), 1);
        let insert = vec![PatchOp::Insert {
            index: 0,
            item: todo,
        }];
        assert_eq!(ops(&sets[0], TODOS), insert);
        assert_eq!(
            ops(&sets[0], VISIBLE),
            insert,
            "the view is patched, not sent"
        );
    }

    #[test]
    fn an_empty_title_is_a_typed_error() {
        let app = App::new();
        let store = app.store();
        app.observe(store);
        let (status, body) = app.add(store, "   ");
        assert_eq!(status, ReplyStatus::Error);
        assert_eq!(
            TodoError::decode_exact(&body).unwrap(),
            TodoError::EmptyTitle
        );
        assert!(app.change_sets().is_empty(), "a failed add changes nothing");
    }

    #[test]
    fn toggling_one_of_many_items_is_one_op_in_the_list_and_the_view() {
        let app = App::new();
        let store = app.store();
        for title in ["a", "b", "c"] {
            assert_eq!(app.add(store, title).0, ReplyStatus::Ok);
        }
        app.observe(store);
        app.sync(store, "toggle", &id_of(2).encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(sets.len(), 1);
        assert_eq!(entry_op(&sets[0], TODOS), ChangeOp::KeyedPatch);
        let update = vec![PatchOp::Update {
            index: 1,
            item: todo(2, "b", true),
        }];
        assert_eq!(ops(&sets[0], TODOS), update);
        assert_eq!(ops(&sets[0], VISIBLE), update, "under `All` the row stays");
        assert_eq!(value::<u32>(&sets[0], REMAINING), 2);
    }

    #[test]
    fn under_a_filter_a_toggle_is_a_remove_or_an_insert() {
        let app = App::new();
        let store = app.store();
        for title in ["a", "b", "c"] {
            app.add(store, title);
        }
        app.sync(store, "set_filter", &Filter::Active.encode_to_vec());
        app.observe(store);
        app.sync(store, "toggle", &id_of(1).encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(ops(&sets[0], VISIBLE), [PatchOp::Remove { index: 0 }]);
        app.sync(store, "toggle", &id_of(1).encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(
            ops(&sets[0], VISIBLE),
            [PatchOp::Insert {
                index: 0,
                item: todo(1, "a", false)
            }]
        );
    }

    #[test]
    fn the_filter_sends_what_entered_and_left_the_view() {
        let app = App::new();
        let store = app.store();
        for title in ["a", "b"] {
            app.add(store, title);
        }
        app.sync(store, "toggle", &id_of(1).encode_to_vec());
        app.observe(store);
        app.sync(store, "set_filter", &Filter::Done.encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(sets.len(), 1);
        // `todos` did not change: only the filter and the view's membership patch.
        assert_eq!(signal_ids(&sets[0]), [FILTER, VISIBLE]);
        assert_eq!(ops(&sets[0], VISIBLE), [PatchOp::Remove { index: 1 }]);
        app.sync(store, "set_filter", &Filter::Active.encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(
            ops(&sets[0], VISIBLE),
            [
                PatchOp::Remove { index: 0 },
                PatchOp::Insert {
                    index: 0,
                    item: todo(2, "b", false)
                }
            ]
        );
    }

    #[test]
    fn removing_and_clearing_shrink_the_list() {
        let app = App::new();
        let store = app.store();
        for title in ["a", "b", "c", "d"] {
            app.add(store, title);
        }
        app.sync(store, "toggle", &id_of(3).encode_to_vec());
        let mut mirror = Mirror::default();
        mirror.apply(&[app.observe(store)]);
        app.sync(store, "remove", &id_of(1).encode_to_vec());
        app.sync(store, "remove", &id_of(99).encode_to_vec());
        app.sync(store, "clear_done", &[]);
        let sets = app.change_sets();
        mirror.apply(&sets);
        let titles: Vec<&str> = mirror.0.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["b", "d"]);
        assert!(
            sets.iter()
                .flat_map(|cs| &cs.entries)
                .filter(|e| e.signal_id == VISIBLE)
                .all(|e| e.op == ChangeOp::KeyedPatch),
            "only patches after the observe"
        );
    }

    #[test]
    fn a_toggle_in_ten_thousand_rows_is_one_small_patch() {
        let app = App::new();
        let store = app.store();
        app.observe(store);
        app.sync(store, "fill", &10_000_u32.encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(value::<u32>(&sets[0], REMAINING), 7_500);
        let mut mirror = Mirror::default();
        mirror.apply(&sets);
        assert_eq!(mirror.0.len(), 10_000);
        // `Item 5001` (identity 5001, index 5000) is open; under `All` it stays visible.
        app.sync(store, "toggle", &id_of(5_001).encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(sets.len(), 1);
        let entry = sets[0]
            .entries
            .iter()
            .find(|e| e.signal_id == VISIBLE)
            .unwrap();
        assert_eq!(entry.op, ChangeOp::KeyedPatch);
        assert!(
            entry.value.len() < 100,
            "one op, not the view: {} bytes",
            entry.value.len()
        );
        assert_eq!(ops(&sets[0], VISIBLE).len(), 1);
        assert_eq!(value::<u32>(&sets[0], REMAINING), 7_499);
        mirror.apply(&sets);
        assert!(mirror.0[5_000].done);
    }

    #[test]
    fn identities_continue_after_a_restore() {
        let app = App::new();
        let store = app.store();
        for title in ["a", "b", "c"] {
            app.add(store, title);
        }
        app.sync(store, "remove", &id_of(1).encode_to_vec());
        let snapshot = app.t.runtime().snapshot();
        app.t
            .runtime()
            .restore(&snapshot)
            .expect("the snapshot restores");
        // The store was rebuilt from its signals; the next identity is above every survivor,
        // not `len + 1`, which would collide with item 3.
        let (status, body) = app.add(store, "d");
        assert_eq!(status, ReplyStatus::Ok);
        assert_eq!(Todo::decode_exact(&body).unwrap().id, id_of(4));
    }

    #[test]
    fn restore_rebuilds_the_view_and_observing_sends_it_whole() {
        let app = App::new();
        let store = app.store();
        for title in ["a", "b", "c"] {
            app.add(store, title);
        }
        app.sync(store, "toggle", &id_of(2).encode_to_vec());
        app.sync(store, "set_filter", &Filter::Active.encode_to_vec());
        let snapshot = app.t.runtime().snapshot();
        app.t
            .runtime()
            .restore(&snapshot)
            .expect("the snapshot restores");
        let initial = app.observe(store);
        assert_eq!(entry_op(&initial, VISIBLE), ChangeOp::Full);
        assert_eq!(
            value::<Vec<Todo>>(&initial, VISIBLE),
            [todo(1, "a", false), todo(3, "c", false)]
        );
        assert_eq!(value::<u32>(&initial, REMAINING), 2);
        // And patches resume.
        app.sync(store, "toggle", &id_of(3).encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(ops(&sets[0], VISIBLE), [PatchOp::Remove { index: 1 }]);
    }

    #[test]
    fn filters_select_items() {
        let todo = |done| Todo {
            id: id_of(1),
            title: "x".into(),
            done,
        };
        assert!(Filter::All.matches(&todo(true)) && Filter::All.matches(&todo(false)));
        assert!(Filter::Active.matches(&todo(false)) && !Filter::Active.matches(&todo(true)));
        assert!(Filter::Done.matches(&todo(true)) && !Filter::Done.matches(&todo(false)));
    }
}
