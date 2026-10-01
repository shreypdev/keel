//! The offline queue (SPEC 9): idempotent mutations that fail for lack of a network are parked in
//! a persisted queue and replayed first in first out when connectivity returns; everything else
//! fails at once.

mod common;

use common::*;
use undra::wire::Writer;
use undra_ports::fakes::{Fakes, Matcher, SeededRng};
use undra_ports::{HttpError, HttpRequest, HttpResponse, NetKind, Rng};
use undra_query::{CtxQuery, QUEUE_KEY, QueryStatus, backoff_ms};
use undra_wire::{Decode, Encode, Reader, Uuid};

fn network_down() -> HttpError {
    HttpError::Network("down".into())
}

fn post_todos() -> Matcher {
    Matcher::post(format!("{API}/todos"))
}

/// One queued mutation as stored: `(mutation id, encoded input, idempotency key)`.
type Stored = (u32, Vec<u8>, Uuid);

/// Reads the persisted queue back: `(schema hash, items)`.
fn stored_queue(h: &Harness) -> Option<(u64, Vec<Stored>)> {
    let bytes = h.fakes.kv.value(QUEUE_KEY)?;
    let mut r = Reader::new(&bytes);
    let hash = r.read_u64().unwrap();
    let count = r.read_u32().unwrap();
    let items = (0..count)
        .map(|_| {
            (
                r.read_u32().unwrap(),
                r.read_bytes().unwrap().to_vec(),
                Uuid::decode(&mut r).unwrap(),
            )
        })
        .collect();
    r.finish().unwrap();
    Some((hash, items))
}

/// A runtime that has heard from the platform that it is offline.
fn offline_harness() -> Harness {
    let h = Harness::new();
    h.settle();
    h.fakes.connectivity.go_offline();
    assert!(!h.query().is_online());
    h
}

fn add(
    h: &Harness,
    title: &str,
) -> (
    Slot<Result<Todo, TodoError>>,
    undra::runtime::executor::TaskId,
) {
    spawn(h, h.ctx().mutate::<AddTodoMutation>((title.to_owned(),)))
}

fn bodies(h: &Harness) -> Vec<String> {
    h.fakes
        .http
        .calls()
        .iter()
        .filter(|r| r.method == undra_ports::HttpMethod::Post)
        .map(|r| String::from_utf8_lossy(&r.body.clone().unwrap_or_default().0).into_owned())
        .collect()
}

/// The server: every `POST /todos` creates a todo titled with the request body, id 1, 2, 3, ...
fn echo_server(h: &Harness) {
    let next = std::sync::atomic::AtomicU8::new(1);
    h.fakes
        .http
        .respond_with(post_todos(), move |request: &HttpRequest| {
            let n = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let title =
                String::from_utf8_lossy(&request.body.clone().unwrap_or_default().0).into_owned();
            Ok(ok(&todo(n, &title)))
        });
}

// ----- queueing ---------------------------------------------------------------------------------

#[test]
fn an_idempotent_mutation_failing_offline_waits_in_the_persisted_queue_and_completes_online() {
    let h = offline_harness();
    h.serve_page(0, vec![todo(1, "milk")]);
    let page0 = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    h.fakes.http.fail(post_todos(), network_down());

    let ctx = h.ctx();
    let (result, _) = spawn(
        &h,
        ctx.mutate::<AddTodoMutation>(("eggs".to_owned(),))
            .optimistic(|cache| {
                cache.update::<TodosQuery>((0,), |page| page.items.push(todo(2, "eggs")));
            }),
    );
    h.t.run_pending();

    // Parked, not failed: the caller keeps waiting and the optimistic todo stays on screen.
    assert!(take(&result).is_none());
    assert_eq!(h.query().pending_mutations(), 1);
    assert_eq!(page0.data().get().unwrap().items.len(), 2);
    let (hash, items) = stored_queue(&h).expect("the queue is persisted");
    assert_eq!(hash, h.t.runtime().schema_hash());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].0, AddTodoMutation::MUTATION_ID);
    assert_eq!(items[0].1, ("eggs".to_owned(),).encode_to_vec());
    let first_key = h
        .fakes
        .http
        .last_call()
        .unwrap()
        .header("Idempotency-Key")
        .unwrap()
        .to_owned();
    assert_eq!(
        items[0].2.to_string(),
        first_key,
        "the queue keeps the key of the first attempt"
    );

    // The network is back: the queue replays, the caller's `await` finishes with the result of
    // the replay, and what the mutation touched refetches.
    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(2, "eggs")));
    h.serve_page(0, vec![todo(1, "milk"), todo(2, "eggs")]);
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();

    assert_eq!(take(&result), Some(Ok(todo(2, "eggs"))));
    assert_eq!(h.query().pending_mutations(), 0);
    assert!(
        stored_queue(&h).is_none(),
        "the persisted queue is empty again"
    );
    let replay = h
        .fakes
        .http
        .calls()
        .into_iter()
        .find(|r| r.method == undra_ports::HttpMethod::Post)
        .unwrap();
    assert_eq!(replay.header("Idempotency-Key"), Some(first_key.as_str()));
    assert_eq!(page0.data().get().unwrap().items.len(), 2);
    assert_eq!(page0.status().get(), QueryStatus::Success);
}

#[test]
fn queued_mutations_replay_in_the_order_they_were_made() {
    let h = offline_harness();
    h.fakes.http.fail(post_todos(), network_down());
    let mut results = Vec::new();
    for title in ["a", "b", "c"] {
        let (result, _) = add(&h, title);
        h.t.run_pending();
        results.push(result);
    }
    assert_eq!(h.query().pending_mutations(), 3);
    let (_, stored) = stored_queue(&h).unwrap();
    let inputs: Vec<Vec<u8>> = stored.iter().map(|s| s.1.clone()).collect();
    assert_eq!(
        inputs,
        ["a", "b", "c"].map(|t| (t.to_owned(),).encode_to_vec()),
        "the persisted order is the arrival order"
    );

    h.fakes.http.reset();
    echo_server(&h);
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();

    assert_eq!(bodies(&h), ["a", "b", "c"], "first in, first out");
    let done: Vec<_> = results.iter().map(|r| take(r).unwrap().unwrap()).collect();
    assert_eq!(done, [todo(1, "a"), todo(2, "b"), todo(3, "c")]);
    assert_eq!(h.query().pending_mutations(), 0);
}

#[test]
fn a_non_idempotent_mutation_fails_at_once_when_offline_and_is_rolled_back() {
    let h = offline_harness();
    h.serve_page(0, vec![todo(1, "milk")]);
    let page0 = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    h.fakes.http.fail(
        Matcher::method(undra_ports::HttpMethod::Delete),
        network_down(),
    );
    let ctx = h.ctx();
    let result = h.t.run_until(async move {
        ctx.mutate::<ClearTodosMutation>(())
            .optimistic(|cache| {
                cache.set::<TodosQuery>((0,), page(Vec::new()));
            })
            .await
    });
    assert_eq!(result, Err(TodoError::Http(network_down())));
    assert_eq!(h.query().pending_mutations(), 0);
    assert!(stored_queue(&h).is_none());
    assert_eq!(
        page0.data().get().unwrap().items.len(),
        1,
        "the optimistic clear was undone"
    );
}

#[test]
fn a_network_error_while_the_platform_says_online_is_an_ordinary_failure() {
    let h = Harness::new();
    h.settle();
    assert!(h.query().is_online());
    h.fakes.http.fail(post_todos(), network_down());
    let (result, _) = add(&h, "x");
    h.t.run_pending();
    assert_eq!(take(&result), Some(Err(TodoError::Http(network_down()))));
    assert_eq!(h.query().pending_mutations(), 0);
}

#[test]
fn only_network_errors_are_queued() {
    let h = offline_harness();
    h.fakes.http.respond(
        post_todos(),
        HttpResponse::new(422, b"title too short".to_vec()),
    );
    let (result, _) = add(&h, "x");
    h.t.run_pending();
    assert_eq!(
        take(&result),
        Some(Err(TodoError::Rejected("title too short".into()))),
        "a validation error is the server's answer, offline or not"
    );
    assert_eq!(h.query().pending_mutations(), 0);
}

#[test]
fn an_error_type_that_is_http_error_itself_is_recognised_too() {
    let h = offline_harness();
    h.fakes.http.fail(format!("{API}/ping"), network_down());
    let (result, _) = spawn(&h, h.ctx().mutate::<PingMutation>(()));
    h.t.run_pending();
    assert!(take(&result).is_none());
    assert_eq!(h.query().pending_mutations(), 1);

    h.fakes.http.reset();
    h.fakes
        .http
        .respond(format!("{API}/ping"), HttpResponse::new(200, Vec::new()));
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();
    assert_eq!(take(&result), Some(Ok(())));
}

// ----- replay outcomes --------------------------------------------------------------------------

#[test]
fn a_replay_the_server_rejects_leaves_the_queue_and_rolls_the_caller_back() {
    let h = offline_harness();
    h.serve_page(0, vec![todo(1, "milk")]);
    let page0 = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    h.fakes.http.fail(post_todos(), network_down());
    let ctx = h.ctx();
    let (bad, _) = spawn(
        &h,
        ctx.mutate::<AddTodoMutation>(("x".to_owned(),))
            .optimistic(|cache| {
                cache.update::<TodosQuery>((0,), |page| page.items.push(todo(9, "x")));
            }),
    );
    h.t.run_pending();
    let (good, _) = add(&h, "fine");
    h.t.run_pending();
    assert_eq!(h.query().pending_mutations(), 2);
    assert_eq!(page0.data().get().unwrap().items.len(), 2);

    h.fakes.http.reset();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let seen = calls.clone();
    h.fakes
        .http
        .respond_with(post_todos(), move |_: &HttpRequest| {
            if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(HttpResponse::new(422, b"nope".to_vec()))
            } else {
                Ok(ok(&todo(2, "fine")))
            }
        });
    h.serve_page(0, vec![todo(1, "milk")]);
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();

    assert_eq!(take(&bad), Some(Err(TodoError::Rejected("nope".into()))));
    assert_eq!(
        take(&good),
        Some(Ok(todo(2, "fine"))),
        "the next one replays despite the rejection"
    );
    assert_eq!(h.query().pending_mutations(), 0);
    assert!(stored_queue(&h).is_none());
    assert_eq!(
        page0.data().get().unwrap().items,
        vec![todo(1, "milk")],
        "rolled back"
    );
}

#[test]
fn a_replay_that_fails_for_lack_of_network_while_online_backs_off_and_tries_again() {
    let h = offline_harness();
    h.fakes.http.fail(post_todos(), network_down());
    let (result, _) = add(&h, "eggs");
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1);

    // Back online according to the platform, but the server is still unreachable.
    h.fakes.connectivity.go_online(NetKind::Cellular);
    h.t.run_pending();
    assert_eq!(h.http_calls(), 2, "the replay tried at once");
    assert!(take(&result).is_none());
    assert_eq!(h.query().pending_mutations(), 1);

    // It retries with the standard backoff: the key was drawn first (16 bytes), then one
    // jitter per wait.
    let rng = SeededRng::default();
    let _key = rng.fill(16);
    let jitter = |rng: &SeededRng| u64::from_le_bytes(rng.fill(8).0.try_into().unwrap());
    let first = backoff_ms(0, jitter(&rng));
    h.advance_ms(first - 1);
    assert_eq!(h.http_calls(), 2, "no retry before the backoff is over");
    h.advance_ms(1);
    assert_eq!(h.http_calls(), 3);

    // The server recovers; the next attempt (after a longer wait) goes through.
    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(1, "eggs")));
    let second = backoff_ms(1, jitter(&rng));
    assert!(
        second >= 1_600,
        "the second wait is about two seconds: {second}"
    );
    h.advance_ms(second);
    assert_eq!(take(&result), Some(Ok(todo(1, "eggs"))));
    assert_eq!(h.query().pending_mutations(), 0);
}

#[test]
fn going_offline_again_pauses_the_replay_until_the_next_online_event() {
    let h = offline_harness();
    h.fakes.http.fail(post_todos(), network_down());
    let (result, _) = add(&h, "eggs");
    h.t.run_pending();

    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();
    assert_eq!(h.http_calls(), 2);
    // The replay is waiting out its backoff when the network drops again.
    h.fakes.connectivity.go_offline();
    h.advance_ms(60_000);
    assert_eq!(h.http_calls(), 2, "an offline client does not retry");
    assert_eq!(h.query().pending_mutations(), 1);

    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(1, "eggs")));
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();
    assert_eq!(take(&result), Some(Ok(todo(1, "eggs"))));
}

#[test]
fn cancelling_the_caller_does_not_unqueue_the_mutation() {
    let h = offline_harness();
    h.fakes.http.fail(post_todos(), network_down());
    let (result, task) = add(&h, "eggs");
    h.t.run_pending();
    assert_eq!(h.query().pending_mutations(), 1);

    h.ctx().cancel_task(task);
    assert!(take(&result).is_none());
    assert_eq!(h.query().pending_mutations(), 1, "once queued, it will run");

    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(1, "eggs")));
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();
    assert_eq!(bodies(&h), ["eggs"]);
    assert_eq!(h.query().pending_mutations(), 0);
}

// ----- surviving a restart -------------------------------------------------------------------------

/// Queues one `add_todo("eggs")` on a runtime that is then dropped; returns the fakes (with the
/// persisted queue in their `Kv`) and the idempotency key it was queued under.
fn queue_then_restart() -> (Fakes, Uuid) {
    let first = offline_harness();
    first.fakes.http.fail(post_todos(), network_down());
    let (_result, _) = add(&first, "eggs");
    first.t.run_pending();
    let (_, items) = stored_queue(&first).unwrap();
    assert_eq!(items.len(), 1);
    let key = items[0].2;
    let fakes = first.fakes.clone();
    drop(first);
    fakes.http.reset();
    (fakes, key)
}

#[test]
fn a_queued_mutation_survives_a_restart_and_replays_when_the_new_runtime_hydrates() {
    let (fakes, key) = queue_then_restart();
    fakes.http.respond(post_todos(), ok(&todo(1, "eggs")));

    // A new process: same storage, a runtime that assumes it is online until told otherwise.
    let second = Harness::with_fakes(fakes);
    second.settle();
    let posts: Vec<HttpRequest> = second
        .fakes
        .http
        .calls()
        .into_iter()
        .filter(|r| r.method == undra_ports::HttpMethod::Post)
        .collect();
    assert_eq!(posts.len(), 1, "the queued mutation ran at start-up");
    assert_eq!(
        posts[0].header("Idempotency-Key"),
        Some(key.to_string().as_str())
    );
    assert_eq!(second.query().pending_mutations(), 0);
    assert!(stored_queue(&second).is_none());
}

#[test]
fn a_restored_queue_waits_for_the_network_if_the_platform_says_offline() {
    let (fakes, _key) = queue_then_restart();
    let second = Harness::with_fakes(fakes);
    // The platform reports its initial state right after start-up.
    second.fakes.connectivity.go_offline();
    second.settle();
    assert_eq!(second.query().pending_mutations(), 1);
    assert_eq!(second.http_calls(), 0);

    second
        .fakes
        .http
        .respond(post_todos(), ok(&todo(1, "eggs")));
    second.fakes.connectivity.go_online(NetKind::Wifi);
    second.t.run_pending();
    assert_eq!(second.query().pending_mutations(), 0);
    assert_eq!(bodies(&second), ["eggs"]);
}

#[test]
fn a_queue_written_by_another_build_is_dropped_not_replayed() {
    let fakes = Fakes::new();
    let mut w = Writer::new();
    w.write_u64(0xdead_beef); // not this build's schema hash
    w.write_len(1);
    w.write_u32(AddTodoMutation::MUTATION_ID);
    w.write_bytes(&("x".to_owned(),).encode_to_vec());
    Uuid([7; 16]).encode(&mut w);
    fakes.kv.insert(QUEUE_KEY, w.into_vec());
    fakes.http.respond(post_todos(), ok(&todo(1, "x")));

    let h = Harness::with_fakes(fakes);
    h.settle();
    assert_eq!(h.query().pending_mutations(), 0);
    assert_eq!(
        h.http_calls(),
        0,
        "arguments encoded by another schema are never replayed"
    );
    assert!(stored_queue(&h).is_none(), "and the stale queue is deleted");
}

#[test]
fn a_queued_mutation_this_build_does_not_define_is_dropped() {
    let fakes = Fakes::new();
    let schema_hash = {
        let probe = Harness::new();
        probe.t.runtime().schema_hash()
    };
    let mut w = Writer::new();
    w.write_u64(schema_hash);
    w.write_len(1);
    w.write_u32(0x1234_5678);
    w.write_bytes(&[]);
    Uuid([7; 16]).encode(&mut w);
    fakes.kv.insert(QUEUE_KEY, w.into_vec());

    let h = Harness::with_fakes(fakes);
    h.settle();
    assert_eq!(h.query().pending_mutations(), 0);
    assert!(stored_queue(&h).is_none());
    assert_eq!(h.http_calls(), 0);
}
