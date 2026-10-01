//! The to-do list: a store with a keyed list, a filter and two computed values.
//!
//! This is the screen every playground app shows first. `todos` is keyed by `id`, so the core
//! ships an insert, a remove or an update as a keyed patch (SPEC 3.8) and the UI diffs by the same
//! key; `visible` and `remaining` are [`Computed`]s, recomputed in the core and pushed in the same
//! change-set as the write that caused them.

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
    visible: Computed<Vec<Todo>>,
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
        let visible = Computed::new((&todos, &filter), |(todos, filter)| {
            todos
                .iter()
                .filter(|todo| filter.matches(todo))
                .cloned()
                .collect()
        });
        let remaining = Computed::new(&todos, |todos| {
            todos.iter().filter(|todo| !todo.done).count() as u32
        });
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
        self.todos.update(|list| list.push(todo.clone()));
        Ok(todo)
    }

    /// Flips the `done` flag of the item with `id`; unknown ids are ignored.
    pub fn toggle(&self, id: Uuid) {
        self.todos.update(|list| {
            if let Some(todo) = list.iter_mut().find(|todo| todo.id == id) {
                todo.done = !todo.done;
            }
        });
    }

    /// Removes the item with `id`; unknown ids are ignored.
    pub fn remove(&self, id: Uuid) {
        self.todos.update(|list| list.retain(|todo| todo.id != id));
    }

    /// Chooses which items `visible` holds.
    pub fn set_filter(&self, filter: Filter) {
        self.filter.set(filter);
    }

    /// Removes every finished item.
    pub fn clear_done(&self) {
        self.todos.update(|list| list.retain(|todo| !todo.done));
    }
}

/// The identity of the `n`th item: `n` in the first eight bytes, big-endian.
fn id_of(n: u64) -> Uuid {
    let mut id = [0; 16];
    id[..8].copy_from_slice(&n.to_be_bytes());
    Uuid(id)
}

/// The counter value inside an identity made by [`id_of`].
fn counter_of(id: Uuid) -> u64 {
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
    use undra::wire::{Decode, Encode, KeyedPatch, Reader};

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

    #[test]
    fn observing_sends_every_signal_once() {
        let app = App::new();
        let store = app.store();
        let initial = app.observe(store);
        assert_eq!(signal_ids(&initial), [TODOS, FILTER, VISIBLE, REMAINING]);
        assert_eq!(value::<Vec<Todo>>(&initial, TODOS), []);
        assert_eq!(value::<Filter>(&initial, FILTER), Filter::All);
        assert_eq!(value::<u32>(&initial, REMAINING), 0);
    }

    #[test]
    fn adding_ships_the_list_the_computeds_and_one_change_set() {
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
        assert_eq!(value::<Vec<Todo>>(&sets[0], VISIBLE), [todo]);
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
    fn toggling_one_of_many_items_is_a_keyed_update_patch() {
        let app = App::new();
        let store = app.store();
        for title in ["a", "b", "c"] {
            assert_eq!(app.add(store, title).0, ReplyStatus::Ok);
        }
        app.observe(store);
        let second = id_of(2);
        app.sync(store, "toggle", &second.encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(sets.len(), 1);
        assert_eq!(entry_op(&sets[0], TODOS), ChangeOp::KeyedPatch);
        let entry = sets[0]
            .entries
            .iter()
            .find(|e| e.signal_id == TODOS)
            .unwrap();
        let mut r = Reader::new(&entry.value);
        let patch = KeyedPatch::<Todo>::decode(&mut r).unwrap();
        assert_eq!(patch.ops.len(), 1, "an O(change) patch: {patch:?}");
        assert_eq!(value::<u32>(&sets[0], REMAINING), 2);
    }

    #[test]
    fn the_filter_selects_what_visible_holds() {
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
        // `todos` did not change: only the filter and what is derived from it.
        assert_eq!(signal_ids(&sets[0]), [FILTER, VISIBLE]);
        let visible = value::<Vec<Todo>>(&sets[0], VISIBLE);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].title, "a");
    }

    #[test]
    fn removing_and_clearing_shrink_the_list() {
        let app = App::new();
        let store = app.store();
        for title in ["a", "b", "c", "d"] {
            app.add(store, title);
        }
        app.sync(store, "toggle", &id_of(3).encode_to_vec());
        app.observe(store);
        app.sync(store, "remove", &id_of(1).encode_to_vec());
        app.sync(store, "remove", &id_of(99).encode_to_vec());
        app.sync(store, "clear_done", &[]);
        let sets = app.change_sets();
        let last = sets.last().expect("a change-set");
        let titles: Vec<String> = value::<Vec<Todo>>(last, VISIBLE)
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["b", "d"]);
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
