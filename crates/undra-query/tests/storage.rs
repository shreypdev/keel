//! Persisted state across app updates and storage failures (ADR-037, ADR-049): cache entries and
//! queued mutations an older build wrote are migrated by name or reported, nothing queued is lost,
//! the persisted cache is bounded, a failed write is retried, and a queue that cannot be read is
//! never overwritten.

mod common;

use common::*;
use undra::prelude::{DynRecord, DynValue, MigrateError};
use undra::wire::Writer;
use undra_meta::{FieldDef, Schema, TypeClosure, TypeRef};
use undra_ports::fakes::{FailOn, Fakes, Matcher};
use undra_ports::{AppState, HttpError, NetKind, StorageError};
use undra_query::{CtxQuery, QUEUE_KEY, RetryError, cache_key, types_key};
use undra_wire::{Encode, Uuid};

fn field(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        name: name.into(),
        ty,
        default: false,
        docs: String::new(),
    }
}

/// This test core's schema (what a `Harness` runtime collected).
fn schema() -> Schema {
    Harness::new().t.runtime().schema().clone()
}

fn page_key(page: u32) -> String {
    cache_key(TodosQuery::QUERY_ID, &(page,).encode_to_vec())
}

/// A format-2 cache entry written by "another build" with `fingerprint`.
fn entry(fingerprint: u64, updated_at: i64, data: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u16(2);
    w.write_u64(0xdead_beef);
    w.write_u64(fingerprint);
    w.write_i64(updated_at);
    w.write_bytes(data);
    w.into_vec()
}

/// Stores `closure` the way a build stores it.
fn store_closure(fakes: &Fakes, closure: &TypeClosure) {
    fakes.kv.insert(
        types_key(closure.fingerprint()),
        closure.canonical_json().into_bytes(),
    );
}

/// The `query` section of the runtime's stats.
fn query_stats(h: &Harness) -> serde_json::Value {
    let stats: serde_json::Value = serde_json::from_str(&h.t.runtime().stats_json()).unwrap();
    stats["query"].clone()
}

fn logged(h: &Harness, needle: &str) -> bool {
    h.t.host()
        .take_logs()
        .iter()
        .any(|l| l.message.contains(needle))
}

// ----- the cache across an update --------------------------------------------------------------

/// The build before this one: `Todo` had a `done: bool` this build dropped and `Page.total` was
/// a `u32` (a widening).
fn older_todos() -> Schema {
    let mut old = schema();
    let todo = old.records.iter_mut().find(|r| r.name == "Todo").unwrap();
    todo.fields.push(field("done", TypeRef::Bool));
    let page = old.records.iter_mut().find(|r| r.name == "Page").unwrap();
    page.fields[1].ty = TypeRef::U32;
    old
}

#[test]
fn a_cache_entry_from_an_older_build_migrates_structurally_and_is_rewritten_once() {
    let old = older_todos();
    let closure = old.query_closure(TodosQuery::QUERY_ID).unwrap();
    let current = schema().query_closure(TodosQuery::QUERY_ID).unwrap();
    assert_ne!(closure.fingerprint(), current.fingerprint());
    // `Page { items: [Todo { id, title, done }], total: u32 }`, as the old build encoded it.
    let old_page = (vec![(Uuid([1; 16]), "milk".to_owned(), true)], 1_u32).encode_to_vec();
    let fakes = Fakes::new();
    store_closure(&fakes, &closure);
    fakes
        .kv
        .insert(page_key(0), entry(closure.fingerprint(), 5, &old_page));

    let h = Harness::with_fakes(fakes);
    h.settle();
    let handle = h.query().observe::<TodosQuery>((0,));
    assert_eq!(handle.data().get(), Some(page(vec![todo(1, "milk")])));
    // Rewritten in today's form, so the migration runs once.
    let raw = h.fakes.kv.value(&page_key(0)).unwrap();
    assert_eq!(&raw[10..18], &current.fingerprint().to_le_bytes());
    assert_eq!(query_stats(&h)["persist"]["migrated"], 1);
    // The old closure is no longer referenced: deleted at the next hydration.
    let second = Harness::with_fakes(h.fakes.clone());
    second.settle();
    assert!(
        !second
            .fakes
            .kv
            .contains_key(&types_key(closure.fingerprint()))
    );
    assert!(
        second
            .fakes
            .kv
            .contains_key(&types_key(current.fingerprint()))
    );
}

#[test]
fn a_cache_entry_that_does_not_migrate_is_dropped_and_reported() {
    // The old `Todo` had no `title`: this build's `title` has no default.
    let mut old = schema();
    let todo = old.records.iter_mut().find(|r| r.name == "Todo").unwrap();
    todo.fields.retain(|f| f.name != "title");
    let closure = old.query_closure(TodosQuery::QUERY_ID).unwrap();
    let old_page = (vec![Uuid([1; 16])], 1_u64).encode_to_vec();
    let fakes = Fakes::new();
    store_closure(&fakes, &closure);
    fakes
        .kv
        .insert(page_key(0), entry(closure.fingerprint(), 5, &old_page));

    let h = Harness::with_fakes(fakes);
    h.settle();
    assert!(
        !h.fakes.kv.contains_key(&page_key(0)),
        "deleted: it can be fetched again"
    );
    assert_eq!(query_stats(&h)["persist"]["dropped"], 1);
    assert!(logged(&h, "dropped the persisted cache entry"));
    let handle = h.query().observe::<TodosQuery>((0,));
    assert_eq!(handle.data().get(), None);
}

#[test]
fn a_cache_entry_whose_description_is_missing_is_dropped() {
    let fakes = Fakes::new();
    fakes.kv.insert(page_key(0), entry(42, 5, &[1, 2, 3]));
    let h = Harness::with_fakes(fakes);
    h.settle();
    assert!(!h.fakes.kv.contains_key(&page_key(0)));
    assert_eq!(query_stats(&h)["persist"]["dropped"], 1);
}

#[test]
fn the_persisted_cache_is_bounded_least_recently_updated_first() {
    let h = Harness::new();
    h.settle();
    h.query().set_max_persisted_entries(2);
    for n in 0..3 {
        h.serve_page(n, vec![todo(1, "x")]);
        let _handle = h.query().observe::<TodosQuery>((n,));
        h.t.run_pending();
        h.advance_ms(300);
    }
    assert!(!h.fakes.kv.contains_key(&page_key(0)), "the oldest went");
    assert!(h.fakes.kv.contains_key(&page_key(1)));
    assert!(h.fakes.kv.contains_key(&page_key(2)));
    assert_eq!(query_stats(&h)["persisted_entries"], 2);
}

// ----- storage failures (ADR-049) --------------------------------------------------------------

#[test]
fn a_full_store_keeps_the_entry_in_memory_and_writes_it_once_a_write_succeeds() {
    let h = Harness::new();
    h.settle();
    h.fakes.kv.fail(FailOn::Set, StorageError::Full);
    h.serve_page(0, vec![todo(1, "milk")]);
    let handle = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    h.advance_ms(300);
    assert_eq!(
        handle.data().get(),
        Some(page(vec![todo(1, "milk")])),
        "still shown"
    );
    assert!(!h.fakes.kv.contains_key(&page_key(0)));
    let failed = query_stats(&h)["persist"]["write_failed"].as_u64().unwrap();
    assert!(failed >= 1);
    let logs = h.t.host().take_logs();
    let warnings = logs
        .iter()
        .filter(|l| l.message.contains("could not") && l.message.contains("storage is full"))
        .count();
    assert_eq!(warnings, 1, "one WARN per operation and reason: {logs:?}");

    // Another entry while full: it waits (one probe of the failed entry is tried instead).
    h.serve_page(1, vec![todo(2, "eggs")]);
    let _second = h.query().observe::<TodosQuery>((1,));
    h.t.run_pending();
    h.advance_ms(300);
    assert!(!h.fakes.kv.contains_key(&page_key(1)));

    // The store has room again: the next write persists both.
    h.fakes.kv.heal();
    handle.refetch();
    h.t.run_pending();
    h.advance_ms(300);
    assert!(h.fakes.kv.contains_key(&page_key(0)));
    assert!(h.fakes.kv.contains_key(&page_key(1)));
    assert!(
        h.t.runtime().stats_json().contains("\"panics\":0"),
        "no panic"
    );
}

#[test]
fn a_failed_read_of_a_cache_entry_starts_it_empty_and_keeps_it() {
    let first = Harness::new();
    first.settle();
    first.serve_page(0, vec![todo(1, "milk")]);
    let _handle = first.query().observe::<TodosQuery>((0,));
    first.t.run_pending();
    first.advance_ms(300);
    let fakes = first.fakes.clone();
    drop(first);
    fakes
        .kv
        .fail_times(FailOn::Get, StorageError::Io("EIO".into()), 1);
    let h = Harness::with_fakes(fakes);
    h.settle();
    assert!(h.fakes.kv.contains_key(&page_key(0)), "not deleted");
    assert_eq!(query_stats(&h)["persist"]["read_failed"], 1);
}

// ----- the queue across an update and storage failures -----------------------------------------

/// The input of `add_todo` as an older build queued it: `(title: String, urgent: bool)`.
fn older_add_todo() -> TypeClosure {
    let mut old = schema();
    let def = old
        .queries
        .iter_mut()
        .find(|q| q.query_id == AddTodoMutation::MUTATION_ID)
        .unwrap();
    def.params.push(undra_meta::ParamDef {
        name: "urgent".into(),
        ty: TypeRef::Bool,
    });
    old.mutation_closure(AddTodoMutation::MUTATION_ID).unwrap()
}

/// A format-2 queue of one `add_todo` with `fingerprint` and `params`.
fn queue_of(fingerprint: u64, params: &[u8], key: u8) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u16(2);
    w.write_u64(0xdead_beef);
    w.write_len(1);
    w.write_u32(AddTodoMutation::MUTATION_ID);
    w.write_u64(fingerprint);
    w.write_bytes(params);
    Uuid([key; 16]).encode(&mut w);
    w.into_vec()
}

fn post_todos() -> Matcher {
    Matcher::post(format!("{API}/todos"))
}

#[test]
fn a_queued_mutation_from_an_older_build_migrates_by_parameter_name_and_replays() {
    let closure = older_add_todo();
    let fakes = Fakes::new();
    store_closure(&fakes, &closure);
    fakes.kv.insert(
        QUEUE_KEY,
        queue_of(
            closure.fingerprint(),
            &("eggs".to_owned(), true).encode_to_vec(),
            7,
        ),
    );
    fakes.http.respond(post_todos(), ok(&todo(1, "eggs")));
    let h = Harness::with_fakes(fakes);
    h.settle();
    let posts: Vec<_> = h
        .fakes
        .http
        .calls()
        .into_iter()
        .filter(|r| r.method == undra_ports::HttpMethod::Post)
        .collect();
    assert_eq!(posts.len(), 1, "replayed");
    assert_eq!(posts[0].body.as_ref().unwrap().0, b"eggs");
    assert!(h.query().dead_letters().is_empty());
}

#[test]
fn a_queued_mutation_that_does_not_migrate_is_dead_lettered_never_lost() {
    // The older build queued `add_todo(title: u32)`: a type change, not structural.
    let mut old = schema();
    let def = old
        .queries
        .iter_mut()
        .find(|q| q.query_id == AddTodoMutation::MUTATION_ID)
        .unwrap();
    def.params[0].ty = TypeRef::U32;
    let closure = old.mutation_closure(AddTodoMutation::MUTATION_ID).unwrap();
    let fakes = Fakes::new();
    store_closure(&fakes, &closure);
    fakes.kv.insert(
        QUEUE_KEY,
        queue_of(closure.fingerprint(), &(5_u32,).encode_to_vec(), 7),
    );
    let h = Harness::with_fakes(fakes);
    h.settle();
    assert_eq!(h.http_calls(), 0, "not replayed with mis-decoded arguments");
    let dead = h.query().dead_letters();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].mutation, "add_todo");
    assert_eq!(
        dead[0].params.get("title"),
        Some(&undra::runtime::persist::DynValue::Int(5))
    );
    assert!(
        dead[0].reason.contains("does not migrate"),
        "{}",
        dead[0].reason
    );
    assert_eq!(query_stats(&h)["persist"]["dead_lettered"], 1);
    assert!(logged(
        &h,
        "moved a queued `add_todo` to the dead-letter queue"
    ));
    assert!(h.fakes.kv.contains_key(undra_query::DEAD_LETTER_KEY));
    // Its closure is still referenced, so it is kept for a later retry.
    let again = Harness::with_fakes(h.fakes.clone());
    again.settle();
    assert!(
        again
            .fakes
            .kv
            .contains_key(&types_key(closure.fingerprint()))
    );
    assert_eq!(again.query().dead_letters().len(), 1, "persisted");

    // Retrying without a hook leaves it where it is; discarding drops it for good.
    assert!(matches!(
        again.query().retry_dead_letter(Uuid([7; 16])),
        Err(RetryError::Incompatible(_))
    ));
    assert_eq!(
        again.query().retry_dead_letter(Uuid([9; 16])),
        Err(RetryError::NotFound)
    );
    assert!(again.query().discard_dead_letter(Uuid([7; 16])));
    again.t.run_pending();
    assert!(again.query().dead_letters().is_empty());
    assert!(!again.fakes.kv.contains_key(undra_query::DEAD_LETTER_KEY));
}

#[test]
fn a_stored_queue_that_does_not_decode_is_dead_lettered_with_its_bytes() {
    let fakes = Fakes::new();
    fakes.kv.insert(QUEUE_KEY, vec![2, 0, 9, 9]);
    let h = Harness::with_fakes(fakes);
    h.settle();
    let dead = h.query().dead_letters();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].raw, [2, 0, 9, 9], "the bytes are kept intact");
    assert!(dead[0].reason.contains("does not decode"));
}

/// ADR-049 decision 1.4: an app launched before the device's first unlock reads `Locked`. The
/// client does not replay and never writes the queue key until a read succeeds; new offline
/// mutations wait in memory; the read is tried again on `Active`.
#[test]
fn an_unreadable_queue_is_never_overwritten_and_replays_once_readable() {
    // A queue an earlier run left behind.
    let fakes = Fakes::new();
    let current = schema()
        .mutation_closure(AddTodoMutation::MUTATION_ID)
        .unwrap()
        .fingerprint();
    let stored = queue_of(current, &("first".to_owned(),).encode_to_vec(), 1);
    fakes.kv.insert(QUEUE_KEY, stored.clone());
    fakes.kv.fail(FailOn::Get, StorageError::Locked);
    let h = Harness::with_fakes(fakes);
    h.fakes.connectivity.go_offline();
    h.settle();
    assert_eq!(query_stats(&h)["queue"], "unreadable");

    // Offline, a new idempotent mutation waits in memory; the stored queue is untouched.
    h.fakes
        .http
        .fail(post_todos(), HttpError::Network("down".into()));
    let (second, _) = spawn(
        &h,
        h.ctx().mutate::<AddTodoMutation>(("second".to_owned(),)),
    );
    h.t.run_pending();
    h.advance_ms(5_000);
    assert_eq!(
        h.fakes.kv.value(QUEUE_KEY),
        Some(stored),
        "never overwritten unread"
    );
    assert_eq!(h.query().pending_mutations(), 1);

    // Unlocked and active: read, merged (the stored one first), written, replayed when online.
    h.fakes.kv.heal();
    h.fakes.lifecycle.set(AppState::Active);
    h.t.run_pending();
    assert_eq!(query_stats(&h)["queue"], "hydrated");
    assert_eq!(h.query().pending_mutations(), 2);
    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(1, "x")));
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();
    let bodies: Vec<Vec<u8>> = h
        .fakes
        .http
        .calls()
        .into_iter()
        .filter(|r| r.method == undra_ports::HttpMethod::Post)
        .map(|r| r.body.unwrap().0)
        .collect();
    assert_eq!(bodies, [b"first".to_vec(), b"second".to_vec()]);
    assert!(take(&second).unwrap().is_ok());
}

/// ADR-049 decision 1.4 / ADR-037 implementation notes: a queue the store reports `Corrupt` (the key
/// is there, its bytes cannot be read back) is not transient: it becomes a dead letter (with no bytes,
/// since none could be read) and reported, and the queue counts as read, so the client goes on and
/// new offline mutations are persisted again. (Review, 2026-10-02: this path had no test.)
#[test]
fn a_queue_the_store_reports_corrupt_is_dead_lettered_and_counts_as_read() {
    let fakes = Fakes::new();
    fakes.kv.insert(QUEUE_KEY, vec![2, 0, 1, 2, 3]);
    fakes.kv.fail(
        FailOn::Get,
        StorageError::Corrupt("the ciphertext failed authentication".into()),
    );
    let h = Harness::with_fakes(fakes);
    h.fakes.connectivity.go_offline();
    h.settle();
    assert_eq!(
        query_stats(&h)["queue"],
        "hydrated",
        "Corrupt is not retried"
    );
    let dead = h.query().dead_letters();
    let queue = dead
        .iter()
        .find(|d| d.reason.contains("offline queue cannot be read back"))
        .unwrap_or_else(|| panic!("{dead:?}"));
    assert!(queue.raw.is_empty(), "no bytes could be read");
    assert!(
        queue.reason.contains("failed authentication"),
        "{}",
        queue.reason
    );

    // The store works again: a new offline mutation is queued and persisted (the key is not unread).
    h.fakes.kv.heal();
    h.fakes
        .http
        .fail(post_todos(), HttpError::Network("down".into()));
    let _pending = spawn(&h, h.ctx().mutate::<AddTodoMutation>(("later".to_owned(),)));
    h.t.run_pending();
    h.advance_ms(5_000);
    assert_eq!(h.query().pending_mutations(), 1);
    let written = h.fakes.kv.value(QUEUE_KEY).expect("the queue is written");
    assert_ne!(
        written,
        vec![2, 0, 1, 2, 3],
        "rewritten with the new mutation"
    );
}

#[test]
fn a_failed_read_is_tried_again_after_a_backoff() {
    let fakes = Fakes::new();
    fakes
        .kv
        .fail_times(FailOn::Get, StorageError::Io("busy".into()), 2);
    let h = Harness::with_fakes(fakes);
    h.settle();
    assert_eq!(query_stats(&h)["queue"], "unreadable");
    h.advance_ms(30_000);
    h.advance_ms(30_000);
    assert_eq!(query_stats(&h)["queue"], "hydrated");
}

#[test]
fn the_stats_section_reports_the_client() {
    let h = Harness::new();
    h.settle();
    let q = query_stats(&h);
    assert_eq!(q["queue"], "hydrated");
    for counter in [
        "write_failed",
        "read_failed",
        "dropped",
        "migrated",
        "dead_lettered",
    ] {
        assert_eq!(q["persist"][counter], 0, "{counter}");
    }
}

// ----- a mutation hook ---------------------------------------------------------------------------

/// The older `flaky_add` called its parameter `name`; this build calls it `title`. A rename is not
/// structural: this hook converts it (ADR-037 decision 6).
#[undra::migrate(mutation = "flaky_add")]
fn flaky_add_from_name(old: &DynRecord) -> Result<DynRecord, MigrateError> {
    let mut new = old.clone();
    new.rename("name", "title");
    Ok(new)
}

/// `flaky_add(name: String)`, as the older build declared it.
fn older_flaky_add() -> TypeClosure {
    let mut old = schema();
    let def = old
        .queries
        .iter_mut()
        .find(|q| q.query_id == FlakyAddMutation::MUTATION_ID)
        .unwrap();
    def.params[0].name = "name".into();
    old.mutation_closure(FlakyAddMutation::MUTATION_ID).unwrap()
}

fn flaky_item(fingerprint: u64, key: u8) -> (u32, u64, Vec<u8>, Uuid) {
    (
        FlakyAddMutation::MUTATION_ID,
        fingerprint,
        ("milk".to_owned(),).encode_to_vec(),
        Uuid([key; 16]),
    )
}

fn write_item(w: &mut Writer, item: &(u32, u64, Vec<u8>, Uuid)) {
    w.write_u32(item.0);
    w.write_u64(item.1);
    w.write_bytes(&item.2);
    item.3.encode(w);
}

#[test]
fn a_mutation_hook_migrates_what_is_not_structural() {
    let closure = older_flaky_add();
    let fakes = Fakes::new();
    store_closure(&fakes, &closure);
    let mut w = Writer::new();
    w.write_u16(2);
    w.write_u64(0xdead_beef);
    w.write_len(1);
    write_item(&mut w, &flaky_item(closure.fingerprint(), 3));
    fakes.kv.insert(QUEUE_KEY, w.into_vec());
    fakes
        .http
        .respond(Matcher::post(format!("{API}/flaky")), ok(&todo(1, "milk")));
    let h = Harness::with_fakes(fakes);
    h.settle();
    let posts: Vec<_> = h
        .fakes
        .http
        .calls()
        .into_iter()
        .filter(|r| r.url.ends_with("/flaky"))
        .collect();
    assert_eq!(posts.len(), 1, "replayed through the hook");
    assert_eq!(posts[0].body.as_ref().unwrap().0, b"milk");
    assert_eq!(query_stats(&h)["persist"]["migrated"], 1);
}

/// A dead letter an older client could not migrate (it had no hook) is retried by this build,
/// which has one: it goes back to the queue and replays.
#[test]
fn retrying_a_dead_letter_runs_the_hooks_again() {
    let closure = older_flaky_add();
    let fakes = Fakes::new();
    store_closure(&fakes, &closure);
    let mut w = Writer::new();
    w.write_u16(2);
    w.write_u64(0xdead_beef);
    w.write_len(1);
    write_item(&mut w, &flaky_item(closure.fingerprint(), 4));
    w.write_str("its input does not migrate to this build");
    fakes.kv.insert(undra_query::DEAD_LETTER_KEY, w.into_vec());
    fakes
        .http
        .respond(Matcher::post(format!("{API}/flaky")), ok(&todo(1, "milk")));
    let h = Harness::with_fakes(fakes);
    h.settle();
    let dead = h.query().dead_letters();
    assert_eq!(dead.len(), 1);
    assert_eq!(
        dead[0].params.get("name"),
        Some(&DynValue::String("milk".into())),
        "decoded with the description it was written with"
    );
    assert_eq!(h.query().retry_dead_letter(Uuid([4; 16])), Ok(()));
    h.t.run_pending();
    assert!(h.query().dead_letters().is_empty());
    assert_eq!(
        h.fakes
            .http
            .calls()
            .into_iter()
            .filter(|r| r.url.ends_with("/flaky"))
            .count(),
        1
    );
    assert!(!h.fakes.kv.contains_key(undra_query::DEAD_LETTER_KEY));
}
