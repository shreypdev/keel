//! Mutations through the Rust API: optimistic updates, rollback, invalidation, retry and the
//! idempotency key.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use keel_ports::fakes::{Matcher, SeededRng};
use keel_ports::{HttpError, HttpMethod, HttpResponse, Rng};
use keel_query::{CtxQuery, Invalidate, QueryStatus, backoff_ms};
use keel_wire::Uuid;

fn network_down() -> HttpError {
    HttpError::Network("down".into())
}

fn post_todos() -> Matcher {
    Matcher::post(format!("{API}/todos"))
}

/// A page-0 handle that has settled with `milk`.
fn observed_page(h: &Harness) -> keel_query::QueryHandle<TodosQuery> {
    h.serve_page(0, vec![todo(1, "milk")]);
    let handle = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    assert_eq!(handle.status().get(), QueryStatus::Success);
    handle
}

fn push_eggs(cache: &mut keel_query::CacheView<'_>) {
    assert!(cache.update::<TodosQuery>((0,), |page| {
        page.items.push(todo(2, "eggs"));
        page.total += 1;
    }));
}

// ----- optimistic updates ----------------------------------------------------------------------

#[test]
fn an_optimistic_update_is_visible_before_the_mutation_finishes_and_stays_on_success() {
    let h = Harness::new();
    let handle = observed_page(&h);
    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(2, "eggs")));

    let (result, _) = spawn(
        &h,
        h.ctx()
            .mutate::<SlowAddMutation>(("eggs".to_owned(), 500))
            .optimistic(push_eggs),
    );
    h.t.run_pending();
    // The mutation is still waiting on its 500 ms, and the UI already shows the new todo.
    assert!(take(&result).is_none());
    assert_eq!(
        handle.data().get(),
        Some(Page {
            items: vec![todo(1, "milk"), todo(2, "eggs")],
            total: 2
        })
    );
    assert_eq!(h.http_calls(), 0, "nothing has reached the server yet");

    // The server has the new page by the time the invalidation refetches it.
    h.serve_page(0, vec![todo(1, "milk"), todo(2, "eggs")]);
    h.advance_ms(500);
    assert_eq!(take(&result), Some(Ok(todo(2, "eggs"))));
    // The mutation's key is `todos`: the observed page refetched.
    assert!(
        h.fakes
            .http
            .calls()
            .iter()
            .any(|r| r.url == format!("{API}/todos?page=0")),
        "the observed page was refetched after the mutation"
    );
    assert_eq!(handle.data().get().unwrap().items.len(), 2);
}

#[test]
fn a_failed_mutation_restores_every_touched_entry_exactly() {
    let h = Harness::new();
    let handle = observed_page(&h);
    let before = (handle.data().get(), handle.updated_at().get());
    h.fakes
        .http
        .respond(post_todos(), HttpResponse::new(422, b"no".to_vec()));
    h.advance_ms(5_000); // the clock moves, so a restore of `updated_at` is not a coincidence

    let ctx = h.ctx();
    let result = h.t.run_until(async move {
        ctx.mutate::<AddTodoMutation>(("eggs".to_owned(),))
            .optimistic(|cache| {
                push_eggs(cache);
                cache.set::<TodosQuery>((7,), page(vec![todo(9, "phantom")]));
            })
            .await
    });
    assert_eq!(result, Err(TodoError::Rejected("no".into())));
    assert_eq!((handle.data().get(), handle.updated_at().get()), before);
    assert_eq!(handle.status().get(), QueryStatus::Success);
    // An entry the optimistic update created is gone again.
    assert_eq!(h.query().get::<TodosQuery>((7,)), None);
    assert_eq!(h.query().cached_entries(), 1);
}

#[test]
fn several_optimistic_updates_compose_in_order_and_roll_back_together() {
    let h = Harness::new();
    let handle = observed_page(&h);
    h.fakes
        .http
        .respond(post_todos(), HttpResponse::new(500, b"no".to_vec()));
    let ctx = h.ctx();
    let result = h.t.run_until(async move {
        ctx.mutate::<AddTodoMutation>(("x".to_owned(),))
            .optimistic(|cache| {
                cache.update::<TodosQuery>((0,), |page| page.items.push(todo(2, "two")));
            })
            .optimistic(|cache| {
                // Sees the first update.
                let seen = cache.get::<TodosQuery>((0,)).unwrap();
                assert_eq!(seen.items.len(), 2);
                cache.update::<TodosQuery>((0,), |page| page.items.push(todo(3, "three")));
            })
            .await
    });
    assert!(result.is_err());
    assert_eq!(handle.data().get().unwrap().items, vec![todo(1, "milk")]);
}

#[test]
fn update_of_a_missing_entry_does_nothing_and_says_so() {
    let h = Harness::new();
    h.fakes.http.respond(post_todos(), ok(&todo(1, "a")));
    let ctx = h.ctx();
    let changed = Arc::new(Mutex::new(None));
    let seen = changed.clone();
    let result = h.t.run_until(async move {
        ctx.mutate::<AddTodoMutation>(("a".to_owned(),))
            .optimistic(move |cache| {
                *seen.lock().unwrap() = Some(cache.update::<TodosQuery>((5,), |_| unreachable!()));
            })
            .await
    });
    assert_eq!(result, Ok(todo(1, "a")));
    assert_eq!(*changed.lock().unwrap(), Some(false));
    assert_eq!(h.query().cached_entries(), 0);
}

#[test]
fn an_optimistic_write_cancels_a_fetch_that_would_overwrite_it() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/slow?ms=500"), ok(&7_u32));
    let handle = h.query().observe::<SlowQuery>((500,));
    h.t.run_pending();
    h.advance_ms(100);
    assert!(handle.fetching().get());

    h.fakes.http.respond(post_todos(), ok(&todo(1, "a")));
    let ctx = h.ctx();
    h.t.run_until(async move {
        ctx.mutate::<AddTodoMutation>(("a".to_owned(),))
            .optimistic(|cache| cache.set::<SlowQuery>((500,), 99))
            .await
    })
    .unwrap();
    // The optimistic value is on screen and the older answer never lands on top of it.
    assert_eq!(handle.data().get(), Some(99));
    assert!(!handle.fetching().get());
    h.advance_ms(1_000);
    assert_eq!(handle.data().get(), Some(99));
}

#[test]
fn dropping_a_mutation_in_flight_rolls_its_optimistic_writes_back() {
    let h = Harness::new();
    let handle = observed_page(&h);
    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(2, "eggs")));
    let (result, task) = spawn(
        &h,
        h.ctx()
            .mutate::<SlowAddMutation>(("eggs".to_owned(), 500))
            .optimistic(push_eggs),
    );
    h.t.run_pending();
    assert_eq!(handle.data().get().unwrap().items.len(), 2);

    h.ctx().cancel_task(task);
    assert_eq!(
        handle.data().get().unwrap().items.len(),
        1,
        "the cancelled mutation was undone"
    );
    h.advance_ms(1_000);
    assert!(take(&result).is_none());
    assert_eq!(h.http_calls(), 0, "the request was never made");
}

#[test]
fn a_panicking_mutation_rolls_back_and_the_runtime_survives() {
    let h = Harness::new();
    let handle = observed_page(&h);
    let (result, _) = spawn(&h, h.ctx().mutate::<BoomMutation>(()).optimistic(push_eggs));
    h.t.run_pending();
    assert!(
        take(&result).is_none(),
        "a panicked mutation never produces a result"
    );
    assert_eq!(handle.data().get().unwrap().items.len(), 1);
    // The runtime still works.
    handle.refetch();
    h.t.run_pending();
    assert_eq!(handle.status().get(), QueryStatus::Success);
}

// ----- invalidation ----------------------------------------------------------------------------

fn urls(h: &Harness) -> Vec<String> {
    let mut urls: Vec<String> = h
        .fakes
        .http
        .take_calls()
        .into_iter()
        .map(|r| r.url)
        .collect();
    urls.sort();
    urls
}

#[test]
fn a_mutations_key_and_its_invalidates_list_both_apply_on_success() {
    let h = Harness::new();
    h.serve_page(0, vec![todo(1, "milk")]);
    h.fakes
        .http
        .respond(format!("{API}/hello/a"), ok(&"hi".to_owned()));
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    let query = h.query();
    let _page = query.observe::<TodosQuery>((0,));
    let _greeting = query.observe::<GreetingQuery>(("a".to_owned(),));
    let _settings = query.observe::<SettingsQuery>(());
    h.t.run_pending();
    h.fakes.http.take_calls();
    h.fakes.http.respond(post_todos(), ok(&todo(2, "eggs")));

    let ctx = h.ctx();
    h.t.run_until(async move {
        ctx.mutate::<AddTodoMutation>(("eggs".to_owned(),))
            .invalidates(["greeting"])
            .await
    })
    .unwrap();
    h.t.run_pending();
    assert_eq!(
        urls(&h),
        [
            format!("{API}/hello/a"),
            format!("{API}/todos"),
            format!("{API}/todos?page=0")
        ],
        "the key `todos` and the explicit `greeting` refetch; `settings` does not"
    );
}

#[test]
fn a_key_with_a_parameter_invalidates_only_that_entity() {
    let h = Harness::new();
    let mine = Uuid([1; 16]);
    let theirs = Uuid([2; 16]);
    h.fakes.http.respond(
        Matcher::url_prefix(format!("{API}/todos/{mine}")).and(Matcher::method(HttpMethod::Get)),
        ok(&todo(1, "mine")),
    );
    h.fakes.http.respond(
        Matcher::url_prefix(format!("{API}/todos/{theirs}")).and(Matcher::method(HttpMethod::Get)),
        ok(&todo(2, "theirs")),
    );
    h.fakes.http.respond(
        Matcher::post(format!("{API}/todos/{mine}")),
        HttpResponse::new(204, Vec::new()),
    );
    let query = h.query();
    let _a = query.observe::<TodoByIdQuery>((mine, false));
    let _b = query.observe::<TodoByIdQuery>((theirs, false));
    h.t.run_pending();
    h.fakes.http.take_calls();

    let ctx = h.ctx();
    h.t.run_until(async move {
        ctx.mutate::<RenameTodoMutation>((mine, "renamed".to_owned()))
            .await
    })
    .unwrap();
    h.t.run_pending();
    assert_eq!(
        urls(&h),
        [format!("{API}/todos/{mine}"), format!("{API}/todos/{mine}")],
        "the POST and the refetch of `mine`; `theirs` was left alone"
    );
}

#[test]
fn a_failed_mutation_invalidates_nothing() {
    let h = Harness::new();
    let _handle = observed_page(&h);
    h.fakes.http.take_calls();
    h.fakes
        .http
        .respond(post_todos(), HttpResponse::new(500, b"no".to_vec()));
    let ctx = h.ctx();
    let result = h.t.run_until(async move {
        ctx.mutate::<AddTodoMutation>(("x".to_owned(),))
            .invalidates(["todos"])
            .await
    });
    assert!(result.is_err());
    h.t.run_pending();
    assert_eq!(urls(&h), [format!("{API}/todos")], "just the POST");
}

#[test]
fn invalidate_targets_can_be_typed() {
    let h = Harness::new();
    let _handle = observed_page(&h);
    h.fakes.http.take_calls();
    h.fakes
        .http
        .respond(Matcher::post(format!("{API}/todos")), ok(&todo(3, "c")));
    let ctx = h.ctx();
    h.t.run_until(async move {
        ctx.mutate::<KeyProbeMutation>(())
            .invalidates([Invalidate::exact::<TodosQuery>(&(0,))])
            .await
    })
    .unwrap();
    h.t.run_pending();
    assert_eq!(urls(&h), [format!("{API}/todos?page=0")]);
}

#[test]
fn a_mutation_with_no_key_and_no_invalidations_touches_no_query() {
    let h = Harness::new();
    let _handle = observed_page(&h);
    h.fakes.http.take_calls();
    let ctx = h.ctx();
    let result =
        h.t.run_until(async move { ctx.mutate::<KeyProbeMutation>(()).await });
    assert_eq!(result, Ok(false));
    h.t.run_pending();
    assert!(urls(&h).is_empty());
}

// ----- retry and the idempotency key ------------------------------------------------------------

#[test]
fn a_mutation_retries_with_the_standard_backoff_and_keeps_one_idempotency_key() {
    let h = Harness::new();
    h.fakes.http.respond_sequence(
        format!("{API}/flaky"),
        [Err(network_down()), Err(network_down())],
    );
    h.fakes
        .http
        .respond(format!("{API}/flaky"), ok(&todo(5, "flaky")));
    let (result, _) = spawn(
        &h,
        h.ctx().mutate::<FlakyAddMutation>(("flaky".to_owned(),)),
    );
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1);

    // The key is drawn first (16 bytes), then one jitter draw per retry.
    let rng = SeededRng::default();
    let _key = rng.fill(16);
    let delays: Vec<u64> = (0..2)
        .map(|attempt| {
            let bytes: [u8; 8] = rng.fill(8).0.try_into().unwrap();
            backoff_ms(attempt, u64::from_le_bytes(bytes))
        })
        .collect();
    for (retry, delay) in delays.iter().enumerate() {
        h.advance_ms(delay - 1);
        assert_eq!(h.http_calls(), retry + 1, "retry {retry} came early");
        h.advance_ms(1);
        assert_eq!(h.http_calls(), retry + 2);
    }
    assert_eq!(take(&result), Some(Ok(todo(5, "flaky"))));

    let keys: Vec<String> = h
        .fakes
        .http
        .calls()
        .iter()
        .map(|r| {
            r.header("Idempotency-Key")
                .expect("every attempt has the key")
                .to_owned()
        })
        .collect();
    assert_eq!(keys.len(), 3);
    assert!(
        keys.iter().all(|k| k == &keys[0]),
        "one key for all attempts: {keys:?}"
    );
}

#[test]
fn a_mutation_with_no_retries_fails_at_once() {
    let h = Harness::new();
    h.fakes.http.fail(post_todos(), network_down());
    let ctx = h.ctx();
    let result =
        h.t.run_until(async move { ctx.mutate::<AddTodoMutation>(("x".to_owned(),)).await });
    assert_eq!(result, Err(TodoError::Http(network_down())));
    assert_eq!(h.http_calls(), 1);
}

#[test]
fn the_idempotency_key_is_only_visible_to_idempotent_mutations_and_survives_a_retry() {
    assert_eq!(keel_query::idempotency_key(), None);
    let h = Harness::new();
    let ctx = h.ctx();
    let plain =
        h.t.run_until(async move { ctx.mutate::<KeyProbeMutation>(()).await });
    assert_eq!(plain, Ok(false), "a non-idempotent mutation has no key");

    // The idempotent probe sends its key to the server; the first attempt fails, the retry
    // (`retry = 1`) succeeds, and both carried the same key.
    h.fakes.http.respond_sequence(
        Matcher::url_prefix(format!("{API}/probe")),
        [Err(network_down())],
    );
    h.fakes.http.respond(
        Matcher::url_prefix(format!("{API}/probe")),
        HttpResponse::new(200, Vec::new()),
    );
    let (result, _) = spawn(&h, h.ctx().mutate::<KeyProbeIdemMutation>(()));
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1);
    let rng = SeededRng::default();
    let _key = rng.fill(16);
    let bytes: [u8; 8] = rng.fill(8).0.try_into().unwrap();
    h.advance_ms(backoff_ms(0, u64::from_le_bytes(bytes)));
    let key = take(&result)
        .expect("finished")
        .expect("succeeded on the retry");
    assert_eq!(key.len(), 36);
    let urls: Vec<String> = h.fakes.http.calls().into_iter().map(|r| r.url).collect();
    assert_eq!(
        urls,
        [
            format!("{API}/probe?key={key}"),
            format!("{API}/probe?key={key}")
        ]
    );
    assert_eq!(
        keel_query::idempotency_key(),
        None,
        "and it is not left set on the thread"
    );
}

#[test]
fn an_idempotent_key_is_a_random_version_4_uuid_from_the_rng_port() {
    let h = Harness::new();
    h.fakes.http.respond(
        Matcher::url_prefix(format!("{API}/probe")),
        HttpResponse::new(200, Vec::new()),
    );
    let ctx = h.ctx();
    let key =
        h.t.run_until(async move { ctx.mutate::<KeyProbeIdemMutation>(()).await })
            .unwrap();
    let expected = {
        let mut bytes: [u8; 16] = SeededRng::default().fill(16).0.try_into().unwrap();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid(bytes).to_string()
    };
    assert_eq!(
        key, expected,
        "the key comes from the seeded Rng, so it is reproducible"
    );
    assert_eq!(&key[14..15], "4");
}

#[test]
fn an_optimistic_closure_that_panics_halfway_gets_its_writes_undone() {
    let h = Harness::new();
    let handle = observed_page(&h);
    let (result, _) = spawn(
        &h,
        h.ctx().mutate::<KeyProbeMutation>(()).optimistic(|cache| {
            push_eggs(cache);
            panic!("the update blew up after its first write");
        }),
    );
    h.t.run_pending();
    assert!(
        take(&result).is_none(),
        "the panicked task produced no result"
    );
    assert_eq!(
        handle.data().get().unwrap().items,
        vec![todo(1, "milk")],
        "the half-applied update was rolled back"
    );
    assert_eq!(handle.status().get(), QueryStatus::Success);
}

// ----- concurrent optimistic mutations: a rollback undoes only its own changes ------------------
//
// Mutation A writes the cache optimistically and is still running when mutation B writes the
// cache optimistically too. A's failure must not take B's placeholder away (the playground
// finding: a failed toggle removed the item an add had just put on screen).

type Patch = Box<dyn FnOnce(&mut keel_query::CacheView<'_>) + Send>;

/// Retitles the first item of page 0, like a toggle changing an item that is already there.
fn retitle_first(title: &'static str) -> Patch {
    Box::new(move |cache| {
        assert!(cache.update::<TodosQuery>((0,), |page| page.items[0].title = title.to_owned()));
    })
}

/// Appends an item to page 0, like an add showing its placeholder.
fn append(n: u8, title: &'static str) -> Patch {
    Box::new(move |cache| {
        assert!(cache.update::<TodosQuery>((0,), |page| {
            page.items.push(todo(n, title));
            page.total += 1;
        }));
    })
}

fn titles(handle: &keel_query::QueryHandle<TodosQuery>) -> Vec<String> {
    handle
        .data()
        .get()
        .expect("the page has data")
        .items
        .into_iter()
        .map(|todo| todo.title)
        .collect()
}

type Reply = Result<HttpResponse, HttpError>;

fn refused() -> Reply {
    Ok(HttpResponse::new(500, b"no".to_vec()))
}

fn accepted(todo: &Todo) -> Reply {
    Ok(ok(todo))
}

/// A `slow_add` that takes `ms` before its request, with `patch` as its optimistic update.
fn slow(h: &Harness, title: &str, ms: u32, patch: Patch) -> Slot<Result<Todo, TodoError>> {
    spawn(
        h,
        h.ctx()
            .mutate::<SlowAddMutation>((title.to_owned(), ms))
            .optimistic(patch),
    )
    .0
}

/// From now on the server's page 0 is `items` (and nothing else is scripted yet).
fn server_page_is(h: &Harness, items: Vec<Todo>) {
    h.fakes.http.reset();
    h.serve_page(0, items);
}

/// The server answers the POSTs in the order the requests arrive.
fn answer_posts(h: &Harness, replies: impl IntoIterator<Item = Reply>) {
    h.fakes.http.respond_sequence(post_todos(), replies);
}

#[test]
fn a_failed_mutation_keeps_the_placeholder_of_a_later_one_on_the_same_entry() {
    let h = Harness::new();
    let handle = observed_page(&h);
    // The server's page, once B is accepted, has B's item and no trace of the failed toggle.
    server_page_is(&h, vec![todo(1, "milk"), todo(3, "walk")]);
    // A (a toggle of `milk`) is refused after 100 ms; B (an add) is accepted after 600 ms.
    answer_posts(&h, [refused(), accepted(&todo(3, "walk"))]);
    let a = slow(&h, "toggle", 100, retitle_first("milk (done)"));
    let b = slow(&h, "walk", 600, append(3, "walk"));
    h.t.run_pending();
    assert_eq!(titles(&handle), ["milk (done)", "walk"]);

    h.advance_ms(100);
    assert_eq!(take(&a), Some(Err(TodoError::Rejected("no".into()))));
    assert!(take(&b).is_none(), "B is still in flight");
    assert_eq!(
        titles(&handle),
        ["milk (done)", "walk"],
        "B's placeholder survives A's rollback; A's own change is inside B's value until B settles"
    );

    // B settles: the server's page takes over.
    h.advance_ms(500);
    assert_eq!(take(&b), Some(Ok(todo(3, "walk"))));
    assert_eq!(titles(&handle), ["milk", "walk"]);
    assert_eq!(handle.status().get(), QueryStatus::Success);
}

#[test]
fn when_both_mutations_fail_the_entry_goes_back_to_before_both() {
    let h = Harness::new();
    let handle = observed_page(&h);
    let before = (handle.data().get(), handle.updated_at().get());
    h.advance_ms(5_000);
    answer_posts(&h, [refused(), refused()]);
    let a = slow(&h, "toggle", 100, retitle_first("milk (done)"));
    let b = slow(&h, "walk", 600, append(3, "walk"));
    h.t.run_pending();

    h.advance_ms(100);
    assert!(take(&a).unwrap().is_err());
    assert_eq!(titles(&handle), ["milk (done)", "walk"]);
    h.advance_ms(500);
    assert!(take(&b).unwrap().is_err());
    assert_eq!(
        (handle.data().get(), handle.updated_at().get()),
        before,
        "A's failed change did not outlive both rollbacks"
    );
    assert_eq!(handle.status().get(), QueryStatus::Success);
}

#[test]
fn rollbacks_in_reverse_order_undo_one_layer_at_a_time() {
    let h = Harness::new();
    let handle = observed_page(&h);
    let before = (handle.data().get(), handle.updated_at().get());
    h.advance_ms(5_000);
    // B (started last) fails first, at 100 ms; A fails at 600 ms.
    answer_posts(&h, [refused(), refused()]);
    let a = slow(&h, "toggle", 600, retitle_first("milk (done)"));
    let b = slow(&h, "walk", 100, append(3, "walk"));
    h.t.run_pending();

    h.advance_ms(100);
    assert!(take(&b).unwrap().is_err());
    assert_eq!(
        titles(&handle),
        ["milk (done)"],
        "B's rollback leaves what A wrote"
    );
    h.advance_ms(500);
    assert!(take(&a).unwrap().is_err());
    assert_eq!((handle.data().get(), handle.updated_at().get()), before);
}

#[test]
fn a_failed_mutation_leaves_an_entry_another_mutation_wrote_and_restores_its_own() {
    let h = Harness::new();
    let page0 = observed_page(&h);
    let before = (page0.data().get(), page0.updated_at().get());
    h.advance_ms(5_000);
    answer_posts(&h, [refused(), accepted(&todo(9, "x"))]);
    // A writes page 0; B, started after it, writes page 7 (and creates it).
    let a = slow(&h, "toggle", 100, retitle_first("milk (done)"));
    let b = slow(
        &h,
        "x",
        600,
        Box::new(|cache| cache.set::<TodosQuery>((7,), page(vec![todo(9, "phantom")]))),
    );
    h.t.run_pending();
    assert_eq!(titles(&page0), ["milk (done)"]);

    h.advance_ms(100);
    assert!(take(&a).unwrap().is_err());
    assert_eq!((page0.data().get(), page0.updated_at().get()), before);
    assert_eq!(
        h.query().get::<TodosQuery>((7,)),
        Some(page(vec![todo(9, "phantom")])),
        "B's entry was not A's to undo"
    );
    h.advance_ms(500);
    assert!(take(&b).unwrap().is_ok());
    assert_eq!(h.query().cached_entries(), 2);
}

#[test]
fn a_failure_after_the_later_mutation_settled_does_not_undo_the_server_data_it_brought() {
    let h = Harness::new();
    let handle = observed_page(&h);
    server_page_is(&h, vec![todo(1, "milk"), todo(3, "walk")]);
    // B (an add) is accepted after 100 ms and refetches the page; A (a toggle) fails at 600 ms.
    answer_posts(&h, [accepted(&todo(3, "walk")), refused()]);
    let a = slow(&h, "toggle", 600, retitle_first("milk (done)"));
    let b = slow(&h, "walk", 100, append(3, "walk"));
    h.t.run_pending();

    h.advance_ms(100);
    assert_eq!(take(&b), Some(Ok(todo(3, "walk"))));
    assert_eq!(
        titles(&handle),
        ["milk", "walk"],
        "the refetch after B brought the server's page"
    );
    h.advance_ms(500);
    assert!(take(&a).unwrap().is_err());
    assert_eq!(
        titles(&handle),
        ["milk", "walk"],
        "A's snapshot (the page before both) is older than the server's page and stays unused"
    );
    assert_eq!(handle.status().get(), QueryStatus::Success);
}

#[test]
fn a_failure_does_not_overwrite_a_fetch_that_landed_after_its_write() {
    let h = Harness::new();
    let handle = observed_page(&h);
    // The server has a new item by the time somebody refetches the page.
    server_page_is(&h, vec![todo(1, "milk"), todo(4, "bread")]);
    answer_posts(&h, [refused(), refused()]);
    let a = slow(&h, "toggle", 100, retitle_first("milk (done)"));
    h.t.run_pending();
    handle.refetch();
    h.t.run_pending();
    assert_eq!(titles(&handle), ["milk", "bread"]);
    // B starts on top of that page.
    let b = slow(&h, "walk", 600, append(3, "walk"));
    h.t.run_pending();
    assert_eq!(titles(&handle), ["milk", "bread", "walk"]);

    h.advance_ms(100);
    assert!(take(&a).unwrap().is_err());
    assert_eq!(titles(&handle), ["milk", "bread", "walk"]);
    h.advance_ms(500);
    assert!(take(&b).unwrap().is_err());
    assert_eq!(
        titles(&handle),
        ["milk", "bread"],
        "B goes back to the fetched page, not to the page from before A"
    );
}

#[test]
fn a_failure_does_not_overwrite_a_value_set_after_its_write() {
    let h = Harness::new();
    let handle = observed_page(&h);
    answer_posts(&h, [refused()]);
    let a = slow(&h, "toggle", 100, retitle_first("milk (done)"));
    h.t.run_pending();
    h.query()
        .set::<TodosQuery>((0,), page(vec![todo(1, "milk"), todo(5, "set")]));
    h.advance_ms(100);
    assert!(take(&a).unwrap().is_err());
    assert_eq!(titles(&handle), ["milk", "set"]);
}

#[test]
fn an_entry_a_failed_mutation_created_stays_while_a_later_one_writes_it_and_goes_with_it() {
    let h = Harness::new();
    let _page0 = observed_page(&h);
    answer_posts(&h, [refused(), refused()]);
    let fresh: Patch = Box::new(|cache| cache.set::<TodosQuery>((7,), page(vec![todo(7, "a")])));
    let on_top: Patch = Box::new(|cache| {
        assert!(cache.update::<TodosQuery>((7,), |page| page.items.push(todo(8, "b"))));
    });
    let a = slow(&h, "a", 100, fresh);
    let b = slow(&h, "b", 600, on_top);
    h.t.run_pending();
    assert_eq!(h.query().cached_entries(), 2);

    h.advance_ms(100);
    assert!(take(&a).unwrap().is_err());
    assert_eq!(
        h.query().get::<TodosQuery>((7,)).map(|p| p.items.len()),
        Some(2),
        "B is still using the entry"
    );
    h.advance_ms(500);
    assert!(take(&b).unwrap().is_err());
    assert_eq!(h.query().get::<TodosQuery>((7,)), None);
    assert_eq!(
        h.query().cached_entries(),
        1,
        "neither left anything behind"
    );
}

#[test]
fn a_failure_after_a_later_mutation_succeeded_does_not_undo_that_mutations_writes() {
    let h = Harness::new();
    let handle = observed_page(&h);
    let mine = Uuid([1; 16]);
    h.fakes.http.respond(
        Matcher::post(format!("{API}/todos/{mine}")),
        HttpResponse::new(204, Vec::new()),
    );
    answer_posts(&h, [refused()]);
    // A fails at 600 ms. B is accepted at once and only invalidates `todo:<id>`, not the page, so
    // nothing refetches the page: what B wrote optimistically is all the page shows.
    let a = slow(&h, "toggle", 600, retitle_first("milk (done)"));
    let (b, _) = spawn(
        &h,
        h.ctx()
            .mutate::<RenameTodoMutation>((mine, "milk".to_owned()))
            .optimistic(append(3, "walk")),
    );
    h.t.run_pending();
    assert_eq!(take(&b), Some(Ok(())));
    assert_eq!(titles(&handle), ["milk (done)", "walk"]);

    h.advance_ms(600);
    assert!(take(&a).unwrap().is_err());
    assert_eq!(
        titles(&handle),
        ["milk (done)", "walk"],
        "B's settled write is not A's to undo"
    );
}

#[test]
fn the_playground_flow_offline_a_queued_add_keeps_its_placeholder_when_an_earlier_toggle_fails() {
    // Offline is turned on while a toggle is in flight; an add is made and parked in the queue
    // (idempotent, network error). The toggle then fails: the add's item must stay on screen
    // until the replay settles it.
    let h = Harness::new();
    h.settle();
    let handle = observed_page(&h);
    h.fakes.connectivity.go_offline();
    assert!(!h.query().is_online());
    // The add's request is the first to go out (it fails at once: no network); the toggle's goes
    // out after 100 ms and is refused.
    answer_posts(&h, [Err(network_down()), refused()]);
    let a = slow(&h, "toggle", 100, retitle_first("milk (done)"));
    let (b, _) = spawn(
        &h,
        h.ctx()
            .mutate::<AddTodoMutation>(("walk".to_owned(),))
            .optimistic(append(3, "walk")),
    );
    h.t.run_pending();
    assert_eq!(h.query().pending_mutations(), 1, "the add is parked");
    assert!(take(&b).is_none());

    h.advance_ms(100);
    assert!(take(&a).unwrap().is_err(), "the toggle failed");
    assert_eq!(
        titles(&handle),
        ["milk (done)", "walk"],
        "the parked add is still on screen"
    );

    // Back online: the add is replayed and accepted, and the page is refetched.
    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(3, "walk")));
    h.serve_page(0, vec![todo(1, "milk"), todo(3, "walk")]);
    h.fakes.connectivity.go_online(keel_ports::NetKind::Wifi);
    h.advance_ms(0);
    assert_eq!(take(&b), Some(Ok(todo(3, "walk"))));
    assert_eq!(h.query().pending_mutations(), 0);
    assert_eq!(titles(&handle), ["milk", "walk"]);
}
