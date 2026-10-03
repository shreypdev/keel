//! Generic functions, objects and stores (ADR-058): one piece of code over the rows of the to-do
//! list and of the notes, and a Swift, Kotlin or TypeScript engineer sees ordinary functions and
//! classes.
//!
//! Callers are on the platforms, so the core lists the types it is generic over where it declares
//! them, and the schema holds one ordinary definition per listed type:
//!
//! * [`newest`] and [`draft`] are functions over any [`Row`], listed for [`Todo`] and [`Note`]:
//!   `newest(rows: todos)` in Swift, `newest(notes)` in Kotlin, `newest("Todo", todos)` in
//!   TypeScript. `draft` has the row type only in its return, so a platform names it (`draft(Todo.self)`).
//! * [`Selection<T>`] is a store, the rows the user ticked, and `TodoSelection` and `NoteSelection`
//!   are two stores of their own, with their own type ids, signals and handles.
//! * [`Recent<T>`] is a plain object, the rows opened last, instantiated for to-dos as
//!   [`RecentTodos`] and handed out by [`recent_todos`].
//!
//! The functions and the two templates are plain Rust: `T` is a type parameter with a bound, the
//! bodies call `T`'s methods, and the core's own tests run them like any generic code.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use undra::prelude::*;

use crate::notes::Note;
use crate::todos::{Todo, counter_of, id_of};

/// Where the serials of drafts start: far above the identities the stores hand out, which count
/// up from one, so a drafted row never has the identity of a stored one.
const DRAFT_BASE: u64 = 1 << 40;

/// The offset of the next draft above [`DRAFT_BASE`] (a counter: the core reads no clock and no
/// random source). It counts the drafts of this run, and a restored selection raises it above the
/// drafts it holds.
static DRAFTS: AtomicU64 = AtomicU64::new(0);

/// What the generic code of this module needs of a row. Plain Rust: the schema never sees it,
/// only the instantiations the core lists.
pub trait Row: SignalValue {
    /// A row that is not stored anywhere yet, with the identity `serial` and the text `title`.
    fn drafted(serial: u64, title: String) -> Self;
    /// The counter inside the row's identity: rows with the same serial are the same row, and the
    /// larger one is the newer.
    fn serial(&self) -> u64;
}

impl Row for Todo {
    fn drafted(serial: u64, title: String) -> Self {
        Todo {
            id: id_of(serial),
            title,
            done: false,
        }
    }

    fn serial(&self) -> u64 {
        counter_of(self.id)
    }
}

impl Row for Note {
    fn drafted(serial: u64, title: String) -> Self {
        Note {
            id: i64::try_from(serial).unwrap_or(i64::MAX),
            title,
            done: false,
        }
    }

    fn serial(&self) -> u64 {
        u64::try_from(self.id).unwrap_or(0)
    }
}

/// The newest of `rows`, the one whose identity counts highest, or nothing for an empty list.
#[undra::api(generic(T = [Todo, Note]))]
pub fn newest<T: Row>(rows: Vec<T>) -> Option<T> {
    rows.into_iter().max_by_key(Row::serial)
}

/// A new row of the type the caller names, titled `title`, with an identity of its own that no
/// stored row has.
///
/// No argument tells which type is wanted, so the caller names it: the type itself in Swift and
/// Kotlin, its name in TypeScript.
#[undra::api(generic(T = [Todo, Note]))]
pub fn draft<T: Row>(title: String) -> T {
    T::drafted(DRAFT_BASE + DRAFTS.fetch_add(1, Ordering::Relaxed), title)
}

/// Makes the next draft's serial larger than `serial`, when `serial` is a draft's.
fn draws_past(serial: u64) {
    if let Some(drawn) = serial.checked_sub(DRAFT_BASE) {
        DRAFTS.fetch_max(drawn.saturating_add(1), Ordering::Relaxed);
    }
}

/// The rows the user has ticked, in the order they were ticked: a store over any [`Row`].
///
/// The struct and the impl block below are the template; it registers nothing until an alias
/// instantiates it (`TodoSelection`, `NoteSelection`).
#[undra::store(generic, restore = "Self::assemble")]
pub struct Selection<T> {
    /// The ticked rows, keyed by their `id` so a tick is one keyed `Insert` and an untick one `Remove`.
    #[undra(key = "id")]
    rows: Signal<Vec<T>>,
    /// How many rows are ticked, kept as rows come and go.
    count: Computed<u32>,
}

#[undra::api(store, generic)]
impl<T: Row> Selection<T> {
    /// Nothing ticked.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Vec::new()))
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot.
    fn assemble(_ctx: Ctx, rows: Signal<Vec<T>>) -> Self {
        // A restored selection may hold drafts of an earlier run of the core, whose counter went
        // further than this run's: drafts continue above every one of them, as `Todos` continues
        // its identities above the snapshot's, so a new draft never unticks an old one.
        rows.with(|rows| {
            for row in rows {
                draws_past(row.serial());
            }
        });
        let count = Computed::new(&rows, |rows: &Vec<T>| {
            u32::try_from(rows.len()).unwrap_or(u32::MAX)
        });
        Selection { rows, count }
    }

    /// Ticks `row`, or unticks it if it is ticked.
    pub fn toggle(&self, row: T) {
        let at = self
            .rows
            .with(|rows| rows.iter().position(|r| r.serial() == row.serial()));
        match at {
            Some(at) => {
                self.rows.remove(at);
            }
            None => self.rows.push(row),
        }
    }

    /// Ticks every row of `rows` that is not ticked yet, in one transaction.
    pub fn select_all(&self, rows: Vec<T>) {
        txn(|| {
            for row in rows {
                let ticked = self
                    .rows
                    .with(|rows| rows.iter().any(|r| r.serial() == row.serial()));
                if !ticked {
                    self.rows.push(row);
                }
            }
        });
    }

    /// Unticks everything.
    pub fn clear(&self) {
        self.rows.set(Vec::new());
    }
}

/// The ticked to-dos.
#[undra::api]
pub type TodoSelection = Selection<Todo>;

/// The ticked notes.
#[undra::api]
pub type NoteSelection = Selection<Note>;

/// The rows opened last, newest first: a plain object over any [`Row`], with no signals.
pub struct Recent<T> {
    limit: usize,
    rows: Mutex<VecDeque<T>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[undra::api(generic)]
impl<T: Row> Recent<T> {
    /// A list that keeps the last `limit` rows opened.
    pub fn new(limit: u32) -> Self {
        Recent {
            limit: limit as usize,
            rows: Mutex::new(VecDeque::new()),
        }
    }

    /// Records that `row` was opened: it moves to the front, and the oldest falls off the end.
    pub fn open(&self, row: T) {
        let mut rows = lock(&self.rows);
        rows.retain(|r| r.serial() != row.serial());
        rows.push_front(row);
        rows.truncate(self.limit);
    }

    /// The rows opened last, newest first.
    pub fn rows(&self) -> Vec<T> {
        lock(&self.rows).iter().cloned().collect()
    }

    /// The row opened last, if any was.
    pub fn latest(&self) -> Option<T> {
        lock(&self.rows).front().cloned()
    }
}

/// The to-dos opened last.
#[undra::api]
pub type RecentTodos = Recent<Todo>;

/// A list of recently opened to-dos that already holds `rows`, the last of them opened last,
/// and keeps `limit` of them.
///
/// An object is returned from a function like any value: the platform gets one wrapper for it.
#[undra::api]
pub fn recent_todos(rows: Vec<Todo>, limit: u32) -> Arc<RecentTodos> {
    let recent = Recent::new(limit);
    for row in rows {
        recent.open(row);
    }
    Arc::new(recent)
}

#[cfg(test)]
mod tests {
    use undra::meta::{collect_schema, ids};
    use undra::runtime::testing::TestRuntime;
    use undra::signals::ALL_SIGNALS;
    use undra::wire::payload::{CallTarget, ChangeOp, ReplyStatus};
    use undra::wire::{Decode, Encode, Handle, KeyedPatch, PatchOp, Reader};

    use super::*;

    fn todo(serial: u64) -> Todo {
        Todo::drafted(serial, format!("todo {serial}"))
    }

    fn note(serial: u64) -> Note {
        Note::drafted(serial, format!("note {serial}"))
    }

    #[test]
    fn the_newest_row_has_the_largest_serial() {
        assert_eq!(newest(vec![todo(2), todo(7), todo(3)]), Some(todo(7)));
        assert_eq!(newest(vec![note(5), note(1)]), Some(note(5)));
        assert_eq!(newest(Vec::<Todo>::new()), None);
    }

    #[test]
    fn a_draft_has_a_fresh_identity_above_every_stored_one() {
        let a: Todo = draft("first".to_owned());
        let b: Todo = draft("second".to_owned());
        assert_ne!(a.id, b.id);
        assert!(a.serial() >= DRAFT_BASE && b.serial() > a.serial());
        assert_eq!((a.title.as_str(), a.done), ("first", false));
        let n: Note = draft("a note".to_owned());
        assert!(n.serial() >= DRAFT_BASE);
    }

    #[test]
    fn recent_keeps_the_last_rows_newest_first_without_duplicates() {
        let recent = Recent::new(2);
        for serial in [1, 2, 1, 3] {
            recent.open(todo(serial));
        }
        assert_eq!(recent.rows(), vec![todo(3), todo(1)]);
        assert_eq!(recent.latest(), Some(todo(3)));
        let held = recent_todos(vec![todo(4), todo(5)], 5);
        assert_eq!(held.rows(), vec![todo(5), todo(4)]);
    }

    /// The registered schema describes each instantiation on its own.
    #[test]
    fn the_schema_has_one_definition_per_listed_type() {
        let schema = collect_schema("playground-core");
        let function = |name: &str| schema.functions.iter().find(|f| f.name == name);
        for name in ["newest<Todo>", "newest<Note>", "draft<Todo>", "draft<Note>"] {
            let f = function(name).unwrap_or_else(|| panic!("no function {name}"));
            assert_eq!(f.method_id, ids::function_id(name));
        }
        assert!(function("newest<Draft>").is_none());
        // Inferred from the rows for `newest`, a token for `draft`.
        let inferred = |name: &str| {
            function(name)
                .and_then(|f| f.generic.as_ref())
                .map(|g| g.args[0].inferred)
        };
        assert_eq!(inferred("newest<Todo>"), Some(true));
        assert_eq!(inferred("draft<Todo>"), Some(false));
        for object in ["TodoSelection", "NoteSelection", "RecentTodos"] {
            assert!(schema.objects.iter().any(|o| o.name == object), "{object}");
        }
        assert_ne!(
            schema.store_fingerprint(ids::type_id("TodoSelection")),
            schema.store_fingerprint(ids::type_id("NoteSelection"))
        );
        schema.validate().unwrap();
    }

    /// A draft after a restore never has the identity of a drafted row the restored selection
    /// holds (review L7): the snapshot may come from an earlier run of the core, whose draft
    /// counter went further than this run's, and `toggle` tells rows apart by identity.
    #[test]
    fn a_draft_after_a_restore_is_above_every_drafted_row_the_selection_holds() {
        let t = TestRuntime::new();
        let reply = t.call_sync(
            CallTarget::Constructor {
                type_id: ids::type_id("TodoSelection"),
                method_id: ids::method_id("TodoSelection", "new"),
            },
            1,
            &[],
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        let selection = Handle::decode_exact(&reply.body).unwrap();
        // A row drafted far ahead of this run's counter, as by an earlier run of the core.
        let now: Todo = draft("probe".to_owned());
        let ahead = Todo::drafted(now.serial() + 1_000_000, "from an earlier run".to_owned());
        let reply = t.call_sync(
            CallTarget::Method {
                handle: selection,
                method_id: ids::method_id("TodoSelection", "toggle"),
            },
            2,
            &ahead.encode_to_vec(),
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        let snapshot = t.runtime().snapshot();
        t.runtime()
            .restore(&snapshot)
            .expect("the snapshot restores");
        let reply = t.call_sync(
            CallTarget::Function {
                method_id: ids::function_id("draft<Todo>"),
            },
            3,
            &"next".to_owned().encode_to_vec(),
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        let next = Todo::decode_exact(&reply.body).unwrap();
        assert!(
            next.serial() > ahead.serial(),
            "the draft {} is not above the restored {}",
            next.serial(),
            ahead.serial()
        );
    }

    /// Ticking a row is one keyed `Insert` for the todo selection and nothing for the note one.
    #[test]
    fn two_selections_are_two_stores_with_one_keyed_patch_each_tick() {
        let t = TestRuntime::new();
        let mut call = 0;
        let mut construct = |name: &str| {
            call += 1;
            let reply = t.call_sync(
                CallTarget::Constructor {
                    type_id: ids::type_id(name),
                    method_id: ids::method_id(name, "new"),
                },
                call,
                &[],
            );
            assert_eq!(reply.status, ReplyStatus::Ok);
            Handle::decode_exact(&reply.body).unwrap()
        };
        let todos = construct("TodoSelection");
        let notes = construct("NoteSelection");
        assert_ne!(todos, notes);
        t.take_change_sets();
        t.runtime().observe(todos.0, ALL_SIGNALS, true);
        t.runtime().observe(notes.0, ALL_SIGNALS, true);
        t.host().take_decoded_change_sets();

        let reply = t.call_sync(
            CallTarget::Method {
                handle: todos,
                method_id: ids::method_id("TodoSelection", "toggle"),
            },
            100,
            &todo(1).encode_to_vec(),
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        let sets = t.host().take_decoded_change_sets();
        assert_eq!(sets.len(), 1, "{sets:?}");
        // One entry, for the todo selection's `rows`: the note selection hears nothing.
        let entries: Vec<_> = sets[0].entries.iter().collect();
        assert_eq!(entries.len(), 2, "rows and count: {entries:?}");
        assert!(entries.iter().all(|e| e.handle == todos), "{entries:?}");
        let rows = entries
            .iter()
            .find(|e| e.signal_id == 0)
            .expect("an entry for rows");
        assert_eq!(rows.op, ChangeOp::KeyedPatch);
        let mut r = Reader::new(&rows.value);
        let patch = KeyedPatch::<Todo>::decode(&mut r).unwrap();
        assert!(matches!(patch.ops.as_slice(), [PatchOp::Insert { .. }]));
    }
}
