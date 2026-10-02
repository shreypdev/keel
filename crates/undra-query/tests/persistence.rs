//! Persistence (SPEC 9, format 2 of ADR-037): entries of `persist` queries are written to the `Kv`
//! port 250 ms after each successful fetch under `undra.query.cache2.<query_id>.<fnv1a64(params)>`
//! with the fingerprint of their type, read back when the runtime starts, and kept across a schema
//! change that does not touch their type. Migration, failures and the queue: `tests/storage.rs`.

mod common;

use common::*;
use undra_meta::ids::fnv1a64;
use undra_ports::fakes::{Fakes, StoreOp};
use undra_ports::{HttpError, HttpResponse};
use undra_query::{CACHE_KEY_PREFIX, QueryStatus, cache_key};
use undra_wire::{Bytes, Encode, Reader};

/// A persisted entry as stored: `(schema hash, updated_at, data bytes)`; the format and the
/// fingerprint are checked here.
fn stored(h: &Harness, key: &str) -> Option<(u64, i64, Vec<u8>)> {
    let bytes = h.fakes.kv.value(key)?;
    let mut r = Reader::new(&bytes);
    assert_eq!(r.read_u16().unwrap(), 2, "format 2");
    let hash = r.read_u64().unwrap();
    let fingerprint = r.read_u64().unwrap();
    let entry = (
        hash,
        r.read_i64().unwrap(),
        r.read_bytes().unwrap().to_vec(),
    );
    r.finish().unwrap();
    assert_eq!(fingerprint, fingerprint_of(h, key), "today's fingerprint");
    Some(entry)
}

/// The current fingerprint of the query the cache key belongs to.
fn fingerprint_of(h: &Harness, key: &str) -> u64 {
    let id = u32::from_str_radix(&key["undra.query.cache2.".len()..][..8], 16).unwrap();
    h.t.runtime()
        .schema()
        .query_closure(id)
        .unwrap()
        .fingerprint()
}

/// The fingerprint of `settings`' cached value (a `String`) in any build of this test core.
fn settings_fingerprint() -> u64 {
    undra_meta::Schema::new("t")
        .closure(&undra_meta::TypeRef::String)
        .fingerprint()
}

/// A format-2 entry of `settings` with today's fingerprint.
fn encoded(hash: u64, updated_at: i64, data: &impl Encode) -> Vec<u8> {
    let mut w = undra_wire::Writer::new();
    w.write_u16(2);
    w.write_u64(hash);
    w.write_u64(settings_fingerprint());
    w.write_i64(updated_at);
    w.write_bytes(&data.encode_to_vec());
    w.into_vec()
}

/// A format-1 entry (before ADR-037): `schema_hash, updated_at, data`.
fn encoded_v1(hash: u64, updated_at: i64, data: &impl Encode) -> Vec<u8> {
    let mut w = undra_wire::Writer::new();
    w.write_u64(hash);
    w.write_i64(updated_at);
    w.write_bytes(&data.encode_to_vec());
    w.into_vec()
}

fn settings_key_v1() -> String {
    format!(
        "undra.query.cache.{:08x}.{:016x}",
        SettingsQuery::QUERY_ID,
        fnv1a64(&().encode_to_vec())
    )
}

fn settings_key() -> String {
    cache_key(SettingsQuery::QUERY_ID, &().encode_to_vec())
}

fn todos_key(page: u32) -> String {
    cache_key(TodosQuery::QUERY_ID, &(page,).encode_to_vec())
}

fn serve_settings(h: &Harness, value: &str) {
    h.fakes.http.reset();
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&value.to_owned()));
}

/// The number of times `key` was written to the store.
fn writes(h: &Harness, key: &str) -> usize {
    h.fakes
        .kv
        .ops()
        .iter()
        .filter(|op| matches!(op, StoreOp::Set(k) if k == key))
        .count()
}

// ----- writing ---------------------------------------------------------------------------------

#[test]
fn the_key_is_the_query_id_and_the_hash_of_the_parameters() {
    let params = (3_u32,).encode_to_vec();
    assert_eq!(
        cache_key(TodosQuery::QUERY_ID, &params),
        format!(
            "undra.query.cache2.{:08x}.{:016x}",
            TodosQuery::QUERY_ID,
            fnv1a64(&params)
        )
    );
    assert!(cache_key(1, &[]).starts_with(CACHE_KEY_PREFIX));
}

#[test]
fn a_successful_fetch_is_written_250_ms_later() {
    let h = Harness::new();
    h.settle();
    serve_settings(&h, "dark");
    let handle = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    let fetched_at = handle.updated_at().get().unwrap().0;
    assert_eq!(handle.data().get(), Some("dark".to_owned()));

    h.advance_ms(249);
    assert_eq!(
        stored(&h, &settings_key()),
        None,
        "not yet: the write is debounced"
    );
    h.advance_ms(1);
    let (hash, updated_at, data) = stored(&h, &settings_key()).expect("written at 250 ms");
    assert_eq!(hash, h.t.runtime().schema_hash());
    assert_eq!(updated_at, fetched_at);
    assert_eq!(data, "dark".to_owned().encode_to_vec());
}

#[test]
fn fetches_inside_the_window_share_one_write_of_the_latest_value() {
    let h = Harness::new();
    h.settle();
    serve_settings(&h, "one");
    let handle = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    h.advance_ms(100);
    serve_settings(&h, "two");
    handle.refetch();
    h.t.run_pending();
    h.advance_ms(100);
    serve_settings(&h, "three");
    handle.refetch();
    h.t.run_pending();
    assert_eq!(writes(&h, &settings_key()), 0);

    h.advance_ms(250);
    assert_eq!(
        writes(&h, &settings_key()),
        1,
        "one write for three fetches"
    );
    let (_, _, data) = stored(&h, &settings_key()).unwrap();
    assert_eq!(data, "three".to_owned().encode_to_vec());

    // A later fetch starts a new window.
    serve_settings(&h, "four");
    handle.refetch();
    h.t.run_pending();
    h.advance_ms(250);
    assert_eq!(writes(&h, &settings_key()), 2);
    assert_eq!(
        stored(&h, &settings_key()).unwrap().2,
        "four".to_owned().encode_to_vec()
    );
}

#[test]
fn queries_without_persist_and_failed_fetches_write_nothing() {
    let h = Harness::new();
    h.settle();
    h.fakes
        .http
        .respond(format!("{API}/hello/x"), ok(&"hi".to_owned()));
    let _greeting = h.query().observe::<GreetingQuery>(("x".to_owned(),));
    h.fakes
        .http
        .fail(format!("{API}/settings"), HttpError::Network("down".into()));
    let _settings = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    h.advance_ms(10_000);
    assert!(
        h.fakes.kv.keys().is_empty(),
        "nothing persisted: {:?}",
        h.fakes.kv.keys()
    );
}

#[test]
fn writing_the_cache_directly_persists_a_persisted_query() {
    let h = Harness::new();
    h.settle();
    h.query().set::<SettingsQuery>((), "light".to_owned());
    h.advance_ms(250);
    assert_eq!(
        stored(&h, &settings_key()).unwrap().2,
        "light".to_owned().encode_to_vec()
    );
}

#[test]
fn an_optimistic_write_is_not_persisted() {
    let h = Harness::new();
    h.settle();
    h.serve_page(0, vec![todo(1, "milk")]);
    let _page = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    h.advance_ms(250);
    let before = stored(&h, &todos_key(0)).unwrap();

    h.fakes.http.respond(
        undra_ports::fakes::Matcher::post(format!("{API}/todos")),
        HttpResponse::new(500, b"no".to_vec()),
    );
    let ctx = h.ctx();
    let _ = h.t.run_until(async move {
        undra_query::CtxQuery::mutate::<AddTodoMutation>(&ctx, ("x".to_owned(),))
            .optimistic(|cache| {
                cache.update::<TodosQuery>((0,), |page| page.items.push(todo(2, "x")));
            })
            .await
    });
    h.advance_ms(1_000);
    assert_eq!(
        stored(&h, &todos_key(0)),
        Some(before),
        "the optimistic value never reached storage"
    );
}

// ----- reading ---------------------------------------------------------------------------------

/// A first run that fetched `settings` and let it be persisted; returns its fakes, ready for a
/// second run.
fn first_run(value: &str) -> Fakes {
    let h = Harness::new();
    h.settle();
    serve_settings(&h, value);
    let handle = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    h.advance_ms(250);
    assert!(stored(&h, &settings_key()).is_some());
    drop(handle);
    let fakes = h.fakes.clone();
    drop(h);
    fakes
}

#[test]
fn a_persisted_entry_is_shown_at_once_by_the_next_run_without_fetching_while_fresh() {
    let fakes = first_run("dark");
    let h = Harness::with_fakes(fakes);
    h.fakes.http.reset();
    h.settle(); // hydration reads the store

    let handle = h.query().observe::<SettingsQuery>(());
    assert_eq!(handle.data().get(), Some("dark".to_owned()));
    assert_eq!(handle.status().get(), QueryStatus::Success);
    assert!(
        !handle.fetching().get(),
        "one hour is the window: still fresh"
    );
    h.t.run_pending();
    assert_eq!(h.http_calls(), 0);
}

#[test]
fn a_stale_persisted_entry_is_shown_while_it_refetches() {
    let fakes = first_run("dark");
    let h = Harness::with_fakes(fakes);
    h.settle();
    h.advance_ms(3_600_000); // an hour later: stale

    serve_settings(&h, "light");
    let handle = h.query().observe::<SettingsQuery>(());
    assert_eq!(
        handle.data().get(),
        Some("dark".to_owned()),
        "the old value shows meanwhile"
    );
    assert_eq!(handle.status().get(), QueryStatus::Success);
    assert!(handle.fetching().get());
    h.t.run_pending();
    assert_eq!(handle.data().get(), Some("light".to_owned()));
}

#[test]
fn the_age_of_a_persisted_entry_comes_from_when_it_was_fetched() {
    let fakes = first_run("dark");
    let stored_at = stored_at(&fakes);
    let h = Harness::with_fakes(fakes);
    h.settle();
    let handle = h.query().observe::<SettingsQuery>(());
    assert_eq!(handle.updated_at().get().map(|t| t.0), Some(stored_at));
}

fn stored_at(fakes: &Fakes) -> i64 {
    let bytes = fakes.kv.value(&settings_key()).unwrap();
    // format u16, schema hash u64, fingerprint u64, then updated_at.
    Reader::new(&bytes[18..26]).read_i64().unwrap()
}

/// ADR-037: an entry written by a build with another schema hash but the same type of value is
/// kept (an unrelated change, a new method, no longer wipes the cache).
#[test]
fn an_entry_written_under_another_schema_hash_with_the_same_type_is_kept() {
    let fakes = Fakes::new();
    fakes
        .kv
        .insert(settings_key(), encoded(0xdead_beef, 5, &"kept".to_owned()));
    let h = Harness::with_fakes(fakes);
    h.settle();
    let handle = h.query().observe::<SettingsQuery>(());
    assert_eq!(handle.data().get(), Some("kept".to_owned()));
}

#[test]
fn an_entry_of_format_1_from_another_schema_is_deleted_reported_and_ignored() {
    let fakes = Fakes::new();
    fakes.kv.insert(
        settings_key_v1(),
        encoded_v1(0xdead_beef, 5, &"stale".to_owned()),
    );
    // An unrelated key in the store is none of the cache's business.
    fakes.kv.insert("app.own.key", vec![1]);
    let h = Harness::with_fakes(fakes);
    h.settle();

    assert_eq!(
        h.fakes.kv.value(&settings_key_v1()),
        None,
        "dropped from storage"
    );
    assert!(h.fakes.kv.contains_key("app.own.key"));
    assert!(
        h.t.host()
            .take_logs()
            .iter()
            .any(|l| l.message.contains("dropped the persisted cache entry")
                && l.message.contains("format 1")),
        "reported"
    );
    serve_settings(&h, "fresh");
    let handle = h.query().observe::<SettingsQuery>(());
    assert_eq!(
        handle.data().get(),
        None,
        "nothing to show from the old schema"
    );
    assert_eq!(handle.status().get(), QueryStatus::Fetching);
    h.t.run_pending();
    assert_eq!(handle.data().get(), Some("fresh".to_owned()));
}

#[test]
fn garbage_in_the_store_is_dropped_without_a_panic() {
    let fakes = Fakes::new();
    fakes.kv.insert(settings_key(), vec![1, 2, 3]);
    fakes
        .kv
        .insert(format!("{CACHE_KEY_PREFIX}not-a-key"), vec![1]);
    let h = Harness::with_fakes(fakes);
    h.settle();
    assert_eq!(h.fakes.kv.value(&settings_key()), None);
    assert!(
        h.fakes
            .kv
            .contains_key(&format!("{CACHE_KEY_PREFIX}not-a-key")),
        "keys that are not ours are left alone"
    );
}

#[test]
fn an_entry_whose_bytes_do_not_decode_is_ignored_when_its_query_is_observed() {
    // The right schema hash but data that is not a `String`: a corrupt or hand-edited entry.
    let h0 = Harness::new();
    let schema_hash = h0.t.runtime().schema_hash();
    let fakes = Fakes::new();
    fakes
        .kv
        .insert(settings_key(), encoded(schema_hash, 5, &7_u8));
    let h = Harness::with_fakes(fakes);
    h.settle();
    serve_settings(&h, "fresh");
    let handle = h.query().observe::<SettingsQuery>(());
    assert_eq!(handle.data().get(), None);
    h.t.run_pending();
    assert_eq!(handle.data().get(), Some("fresh".to_owned()));
}

/// ADR-037 decision 7: an entry of a query this build no longer defines can never be shown or
/// fetched again; it is deleted and reported, so the persisted cache stays bounded.
#[test]
fn an_entry_of_a_query_nobody_defines_is_dropped_and_reported() {
    let h0 = Harness::new();
    let schema_hash = h0.t.runtime().schema_hash();
    let fakes = Fakes::new();
    let key = cache_key(0x0bad_cafe, &[]);
    fakes.kv.insert(key.clone(), encoded(schema_hash, 5, &1_u8));
    let h = Harness::with_fakes(fakes);
    h.settle();
    assert!(!h.fakes.kv.contains_key(&key));
    assert!(
        h.t.host()
            .take_logs()
            .iter()
            .any(|l| l.message.contains("this build has no such query"))
    );
}

#[test]
fn an_entry_of_format_1_from_this_very_build_is_rewritten_in_format_2() {
    let h0 = Harness::new();
    let schema_hash = h0.t.runtime().schema_hash();
    let fakes = Fakes::new();
    fakes.kv.insert(
        settings_key_v1(),
        encoded_v1(schema_hash, 5, &"old".to_owned()),
    );
    let h = Harness::with_fakes(fakes);
    h.settle();
    assert!(
        !h.fakes.kv.contains_key(&settings_key_v1()),
        "the old key is gone"
    );
    let (hash, updated_at, data) = stored(&h, &settings_key()).expect("rewritten");
    assert_eq!((hash, updated_at), (schema_hash, 5));
    assert_eq!(data, "old".to_owned().encode_to_vec());
    let handle = h.query().observe::<SettingsQuery>(());
    assert_eq!(handle.data().get(), Some("old".to_owned()));
}

#[test]
fn hydration_that_arrives_after_an_observer_fills_an_entry_with_nothing_to_show() {
    let h = Harness::new();
    h.settle();
    // The first fetch fails, so the entry exists but has no data.
    h.fakes
        .http
        .fail(format!("{API}/settings"), HttpError::Network("down".into()));
    let handle = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    assert_eq!(handle.data().get(), None);

    // Storage turns out to have a value (a slow store, in real life): hydrating again shows it.
    let schema_hash = h.t.runtime().schema_hash();
    h.fakes.kv.insert(
        settings_key(),
        encoded(schema_hash, 5, &"stored".to_owned()),
    );
    let client = h.query();
    h.t.run_until(client.hydrate());
    assert_eq!(handle.data().get(), Some("stored".to_owned()));
}

#[test]
fn hydrating_twice_is_harmless() {
    let fakes = first_run("dark");
    let h = Harness::with_fakes(fakes);
    h.settle();
    let client = h.query();
    h.t.run_until(client.hydrate());
    let handle = client.observe::<SettingsQuery>(());
    assert_eq!(handle.data().get(), Some("dark".to_owned()));
}

#[test]
fn bytes_in_the_store_use_the_documented_layout() {
    let h = Harness::new();
    h.settle();
    serve_settings(&h, "x");
    let _handle = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    h.advance_ms(250);
    let raw = h.fakes.kv.value(&settings_key()).unwrap();
    let hash = h.t.runtime().schema_hash();
    let mut expected = 2_u16.to_le_bytes().to_vec();
    expected.extend_from_slice(&hash.to_le_bytes());
    expected.extend_from_slice(&settings_fingerprint().to_le_bytes());
    expected.extend_from_slice(&stored(&h, &settings_key()).unwrap().1.to_le_bytes());
    expected.extend_from_slice(&Bytes("x".to_owned().encode_to_vec()).encode_to_vec());
    assert_eq!(
        raw, expected,
        "{{ format u16 = 2, schema_hash u64, fingerprint u64, updated_at i64, data bytes }}, little-endian"
    );
    // The closure is stored once, under its fingerprint, before the entry that needs it.
    let closure = h
        .fakes
        .kv
        .value(&undra_query::types_key(settings_fingerprint()))
        .expect("the type description is stored");
    assert_eq!(
        undra_meta::TypeClosure::from_json(std::str::from_utf8(&closure).unwrap())
            .unwrap()
            .fingerprint(),
        settings_fingerprint()
    );
}

// ----- start-up ordering -----------------------------------------------------------------------

#[test]
fn hydration_waits_for_a_platform_that_registers_its_adapters_late() {
    // A native host starts the core thread before it has registered its port adapters, so the
    // first ask of the `Kv` port can be answered "unavailable". Hydration asks again.
    let t = undra::runtime::testing::TestRuntime::new();
    t.run_init_hooks();
    t.run_pending();

    let fakes = Fakes::new();
    let schema_hash = t.runtime().schema_hash();
    fakes.kv.insert(
        settings_key(),
        encoded(schema_hash, 5, &"stored".to_owned()),
    );
    fakes.install_test(&t);
    t.advance(std::time::Duration::from_millis(100));
    t.run_pending();
    let h = Harness { t, fakes };

    let handle = h.query().observe::<SettingsQuery>(());
    assert_eq!(handle.data().get(), Some("stored".to_owned()));
}

#[test]
fn hydration_gives_up_after_a_few_seconds_without_a_kv_port_and_the_cache_still_works() {
    let t = undra::runtime::testing::TestRuntime::new();
    t.run_init_hooks();
    t.run_pending();
    for _ in 0..60 {
        t.advance(std::time::Duration::from_millis(100));
    }
    let logs = t.host().take_logs();
    assert!(
        logs.iter()
            .any(|l| l.message.contains("Kv port never became available")),
        "{logs:?}"
    );
    // The client is usable: a query that does not persist needs no storage.
    let fakes = Fakes::new();
    fakes.install_test(&t);
    fakes
        .http
        .respond(format!("{API}/hello/x"), ok(&"hi".to_owned()));
    let h = Harness { t, fakes };
    let handle = h.query().observe::<GreetingQuery>(("x".to_owned(),));
    h.t.run_pending();
    assert_eq!(handle.data().get(), Some("hi".to_owned()));
}
