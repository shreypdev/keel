//! What a platform runtime sees: the query handle as an object (constructor, five signals,
//! `refetch` and `invalidate` by their pinned method ids) and mutations as functions, driven
//! through the runtime's raw call path exactly as a Swift, Kotlin or TypeScript runtime does,
//! with the ids and shapes checked against the `undra-bindgen` golden bindings.

mod common;

use std::cell::Cell;

use common::*;
use undra::meta::{QueryKind, Schema, collect_schema, ids};
use undra::runtime::testing::{ReplyRecord, TestRuntime};
use undra::signals::ALL_SIGNALS;
use undra::wire::payload::{CallTarget, ChangeEntry, ChangeOp, ChangeSet, ReplyStatus, Snapshot};
use undra_query::{
    CtxQuery, INVALIDATE_METHOD_ID, MutationDef, QueryDef, QueryStatus, REFETCH_METHOD_ID,
};
use undra_wire::{Decode, Encode, Handle, Reader, Timestamp, Uuid};

/// The signal ids `undra-bindgen` numbers a query handle's signals with.
const DATA: u32 = 0;
const STATUS: u32 = 1;
const ERROR: u32 = 2;
const FETCHING: u32 = 3;
const UPDATED_AT: u32 = 4;

/// A platform: a runtime, its fakes, and the call ids a host allocates.
struct Platform {
    h: Harness,
    next_call: Cell<u32>,
}

impl Platform {
    fn new() -> Platform {
        Platform {
            h: Harness::new(),
            next_call: Cell::new(1),
        }
    }

    fn t(&self) -> &TestRuntime {
        &self.h.t
    }

    fn call_id(&self) -> u32 {
        let id = self.next_call.get();
        self.next_call.set(id + 1);
        id
    }

    /// `new TodosQueryHandle(page)`.
    fn construct<Q: QueryDef>(&self, params: &impl Encode) -> Handle {
        let reply = self.t().call_sync(
            CallTarget::Constructor {
                type_id: Q::ID,
                method_id: Q::ID,
            },
            self.call_id(),
            &params.encode_to_vec(),
        );
        assert_eq!(reply.status, ReplyStatus::Ok, "constructor: {reply:?}");
        Handle::decode_exact(&reply.body).unwrap()
    }

    fn method(&self, handle: Handle, method_id: u32) -> ReplyRecord {
        self.t().call_sync(
            CallTarget::Method { handle, method_id },
            self.call_id(),
            &[],
        )
    }

    /// Observes every signal of `handle` and returns the initial change-set.
    fn observe(&self, handle: Handle) -> ChangeSet {
        self.t().take_change_sets();
        self.t().runtime().observe(handle.0, ALL_SIGNALS, true);
        one(self.change_sets())
    }

    fn change_sets(&self) -> Vec<ChangeSet> {
        self.t().host().take_decoded_change_sets()
    }
}

fn one<T: std::fmt::Debug>(mut items: Vec<T>) -> T {
    assert_eq!(items.len(), 1, "expected exactly one, got {items:?}");
    items.remove(0)
}

fn signal_ids(cs: &ChangeSet) -> Vec<u32> {
    cs.entries.iter().map(|e| e.signal_id).collect()
}

fn entry(cs: &ChangeSet, signal_id: u32) -> &ChangeEntry {
    cs.entries
        .iter()
        .find(|e| e.signal_id == signal_id)
        .unwrap_or_else(|| panic!("no entry for signal {signal_id} in {cs:?}"))
}

fn reason(reply: &ReplyRecord) -> String {
    Reader::new(&reply.body).read_str().unwrap().to_owned()
}

// ----- ids and shapes, checked against the golden bindings -----------------------------------

#[test]
fn the_ids_are_the_ones_the_generated_bindings_were_built_with() {
    // `crates/undra-bindgen/tests/golden/queries/swift/.../Ids.swift`, verbatim.
    assert_eq!(TodosQuery::QUERY_ID, 0x5420_9c7c);
    assert_eq!(TodoByIdQuery::QUERY_ID, 0x2bb5_eff3);
    assert_eq!(AddTodoMutation::MUTATION_ID, 0x4abb_0ec8);
    assert_eq!(ClearTodosMutation::MUTATION_ID, 0xd0e9_5d23);
    assert_eq!(REFETCH_METHOD_ID, 0x21d1_b9e2);
    assert_eq!(INVALIDATE_METHOD_ID, 0x44ce_c2fa);
    assert_eq!(REFETCH_METHOD_ID, ids::fnv1a32("query.refetch"));
    assert_eq!(INVALIDATE_METHOD_ID, ids::fnv1a32("query.invalidate"));
}

#[test]
fn the_registered_queries_are_the_ones_the_golden_schema_describes() {
    let golden: Schema = serde_json::from_str(include_str!(
        "../../undra-bindgen/tests/golden/queries/schema.json"
    ))
    .expect("the golden schema parses");
    let mine = collect_schema("t");
    // `todo_count` and `clear_todos` are hand-written in the golden (no error type) and cannot
    // be produced by the macros, which require `Result<T, E>`; the other three are compared
    // whole: name, id, kind, key template, parameters, return type, staleness, persistence.
    for name in ["todos", "todo_by_id", "add_todo"] {
        let wanted = golden.queries.iter().find(|q| q.name == name).unwrap();
        let found = mine.queries.iter().find(|q| q.name == name).unwrap();
        assert_eq!(
            found, wanted,
            "`{name}` differs from what the bindings were generated for"
        );
    }
    let clear = mine
        .queries
        .iter()
        .find(|q| q.name == "clear_todos")
        .unwrap();
    assert_eq!(
        (clear.kind, clear.key.as_str()),
        (QueryKind::Mutation, "todos")
    );
}

// ----- the handle as an object ---------------------------------------------------------------

#[test]
fn constructing_and_observing_emits_the_initial_change_set_with_signals_0_to_4() {
    let p = Platform::new();
    p.h.serve_page(0, vec![todo(1, "milk")]);
    let handle = p.construct::<TodosQuery>(&0_u32);
    assert!(!handle.is_null());
    assert_eq!(p.t().runtime().objects().live(), 1);

    let cs = p.observe(handle);
    assert_eq!(signal_ids(&cs), [DATA, STATUS, ERROR, FETCHING, UPDATED_AT]);
    for e in &cs.entries {
        assert_eq!((e.handle, e.op), (handle, ChangeOp::Full));
    }
    // The fetch has started but not run: nothing to show yet.
    assert_eq!(entry(&cs, DATA).value, None::<Page>.encode_to_vec());
    assert_eq!(
        entry(&cs, STATUS).value,
        QueryStatus::Fetching.encode_to_vec()
    );
    assert_eq!(
        entry(&cs, STATUS).value,
        [1, 0],
        "a unit enum is its u16 index"
    );
    assert_eq!(entry(&cs, ERROR).value, None::<TodoError>.encode_to_vec());
    assert_eq!(entry(&cs, FETCHING).value, [1]);
    assert_eq!(
        entry(&cs, UPDATED_AT).value,
        None::<Timestamp>.encode_to_vec()
    );
}

#[test]
fn a_constructor_takes_the_parameters_in_declaration_order() {
    let p = Platform::new();
    let id = Uuid([0x42; 16]);
    p.h.fakes.http.respond(
        undra_ports::fakes::Matcher::url_prefix(format!("{API}/todos/{id}")),
        ok(&todo(0x42, "milk")),
    );
    // `todo_by_id(id: Uuid, fresh: bool)`: the id, then the flag, no framing.
    let mut args = Vec::new();
    args.extend_from_slice(&id.0);
    args.push(1);
    let reply = p.t().call_sync(
        CallTarget::Constructor {
            type_id: TodoByIdQuery::QUERY_ID,
            method_id: TodoByIdQuery::QUERY_ID,
        },
        1,
        &args,
    );
    assert_eq!(reply.status, ReplyStatus::Ok);
    let handle = Handle::decode_exact(&reply.body).unwrap();
    p.observe(handle);
    p.t().run_pending();
    assert_eq!(
        p.h.fakes.http.last_call().unwrap().url,
        format!("{API}/todos/{id}?fresh=1")
    );
    let cs = one(p.change_sets());
    assert_eq!(
        entry(&cs, DATA).value,
        Some(todo(0x42, "milk")).encode_to_vec()
    );
}

#[test]
fn a_finished_fetch_is_one_change_set_of_only_what_changed() {
    let p = Platform::new();
    p.h.serve_page(0, vec![todo(1, "milk")]);
    let handle = p.construct::<TodosQuery>(&0_u32);
    p.observe(handle);

    p.t().run_pending();
    let cs = one(p.change_sets());
    // `error` did not change, so it is not in the change-set.
    assert_eq!(signal_ids(&cs), [DATA, STATUS, FETCHING, UPDATED_AT]);
    assert_eq!(
        entry(&cs, DATA).value,
        Some(page(vec![todo(1, "milk")])).encode_to_vec()
    );
    assert_eq!(
        entry(&cs, STATUS).value,
        QueryStatus::Success.encode_to_vec()
    );
    assert_eq!(entry(&cs, FETCHING).value, [0]);
    assert_eq!(
        entry(&cs, UPDATED_AT).value,
        Some(Timestamp(undra_ports::Clock::now_ms(&*p.h.fakes.clock))).encode_to_vec()
    );
}

#[test]
fn a_failed_fetch_ships_the_typed_error() {
    let p = Platform::new();
    p.h.fakes.http.respond(
        format!("{API}/settings"),
        undra_ports::HttpResponse::new(500, b"broken".to_vec()),
    );
    let handle = p.construct::<SettingsQuery>(&());
    p.observe(handle);
    p.t().run_pending();
    let cs = one(p.change_sets());
    assert_eq!(signal_ids(&cs), [STATUS, ERROR, FETCHING]);
    assert_eq!(entry(&cs, STATUS).value, QueryStatus::Error.encode_to_vec());
    assert_eq!(
        entry(&cs, ERROR).value,
        Some(TodoError::Rejected("broken".into())).encode_to_vec()
    );
}

#[test]
fn only_observed_signals_are_delivered() {
    let p = Platform::new();
    p.h.serve_page(0, vec![todo(1, "milk")]);
    let handle = p.construct::<TodosQuery>(&0_u32);
    p.t().take_change_sets();
    p.t().runtime().observe(handle.0, DATA, true);
    let initial = one(p.change_sets());
    assert_eq!(signal_ids(&initial), [DATA]);

    p.t().run_pending();
    let cs = one(p.change_sets());
    assert_eq!(
        signal_ids(&cs),
        [DATA],
        "status and the rest were never observed"
    );
}

#[test]
fn refetch_and_invalidate_are_reachable_by_their_pinned_method_ids() {
    let p = Platform::new();
    p.h.serve_page(0, vec![todo(1, "milk")]);
    let handle = p.construct::<TodosQuery>(&0_u32);
    p.observe(handle);
    p.t().run_pending();
    p.change_sets();
    assert_eq!(p.h.http_calls(), 1);

    // `refetch()` (0x21d1b9e2): a fetch starts although the data is fresh. A second later, so
    // the moment the data was confirmed changes too.
    p.h.advance_ms(1_000);
    let reply = p.method(handle, 0x21d1_b9e2);
    assert_eq!(
        (reply.status, reply.body.is_empty()),
        (ReplyStatus::Ok, true)
    );
    let cs = one(p.change_sets());
    assert_eq!(signal_ids(&cs), [FETCHING]);
    assert_eq!(entry(&cs, FETCHING).value, [1]);
    p.t().run_pending();
    assert_eq!(p.h.http_calls(), 2);
    let cs = one(p.change_sets());
    // Same data, so it is not sent again; the time it was confirmed is.
    assert_eq!(signal_ids(&cs), [FETCHING, UPDATED_AT]);

    // `invalidate()` (0x44cec2fa): marks it stale, and it refetches because it is observed.
    let reply = p.method(handle, 0x44ce_c2fa);
    assert_eq!(reply.status, ReplyStatus::Ok);
    let cs = one(p.change_sets());
    assert_eq!(signal_ids(&cs), [FETCHING]);
    p.t().run_pending();
    assert_eq!(p.h.http_calls(), 3);
}

#[test]
fn an_unknown_method_or_a_stale_handle_is_a_bad_request_not_a_panic() {
    let p = Platform::new();
    p.h.serve_page(0, vec![todo(1, "milk")]);
    let handle = p.construct::<TodosQuery>(&0_u32);
    let unknown = p.method(handle, ids::fnv1a32("query.nope"));
    assert_eq!(unknown.status, ReplyStatus::BadRequest);

    p.t().runtime().release(handle.0);
    let stale = p.method(handle, REFETCH_METHOD_ID);
    assert_eq!(stale.status, ReplyStatus::BadRequest);
    assert!(reason(&stale).contains("stale"), "{}", reason(&stale));
}

#[test]
fn a_bad_constructor_call_is_a_bad_request() {
    let p = Platform::new();
    // Not enough bytes for a `u32` page.
    let reply = p.t().call_sync(
        CallTarget::Constructor {
            type_id: TodosQuery::QUERY_ID,
            method_id: TodosQuery::QUERY_ID,
        },
        1,
        &[1, 2],
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(
        reason(&reply).contains("bad arguments"),
        "{}",
        reason(&reply)
    );
    // Trailing bytes are refused too.
    let reply = p.t().call_sync(
        CallTarget::Constructor {
            type_id: TodosQuery::QUERY_ID,
            method_id: TodosQuery::QUERY_ID,
        },
        2,
        &[0, 0, 0, 0, 9],
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    // A type no query has.
    let reply = p.t().call_sync(
        CallTarget::Constructor {
            type_id: ids::query_id("no_such_query"),
            method_id: ids::query_id("no_such_query"),
        },
        3,
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    // Nothing leaked into the object table, and nothing is being fetched.
    assert_eq!(p.t().runtime().objects().live(), 0);
    p.t().run_pending();
    assert_eq!(p.h.http_calls(), 0);
}

#[test]
fn releasing_the_handle_removes_the_observer_and_cancels_the_fetch() {
    let p = Platform::new();
    p.h.fakes
        .http
        .respond(format!("{API}/slow?ms=500"), ok(&7_u32));
    let handle = p.construct::<SlowQuery>(&500_u32);
    p.observe(handle);
    p.t().run_pending();
    p.h.advance_ms(100);
    assert_eq!(p.h.http_calls(), 1);

    p.t().runtime().release(handle.0);
    assert_eq!(p.t().runtime().objects().live(), 0);
    p.change_sets();
    p.h.advance_ms(1_000);
    assert!(
        p.change_sets().is_empty(),
        "a released handle delivers nothing"
    );
    assert_eq!(
        p.h.query().get::<SlowQuery>((500,)),
        None,
        "the fetch was cancelled"
    );
}

#[test]
fn two_platform_handles_of_one_query_share_a_fetch_and_the_last_release_cancels_it() {
    let p = Platform::new();
    p.h.fakes
        .http
        .respond(format!("{API}/slow?ms=500"), ok(&7_u32));
    let a = p.construct::<SlowQuery>(&500_u32);
    let b = p.construct::<SlowQuery>(&500_u32);
    assert_ne!(a, b);
    p.t().run_pending();
    assert_eq!(p.h.http_calls(), 1);
    p.t().runtime().release(a.0);
    p.observe(b);
    p.h.advance_ms(500);
    let cs = one(p.change_sets());
    assert_eq!(entry(&cs, DATA).value, Some(7_u32).encode_to_vec());
}

#[test]
fn a_snapshot_keeps_what_a_handle_is_made_of_and_a_restore_keeps_the_handle_working() {
    let p = Platform::new();
    p.h.serve_page(0, vec![todo(1, "milk")]);
    let handle = p.construct::<TodosQuery>(&0_u32);
    p.observe(handle);
    p.t().run_pending();

    // A handle is not a snapshotted store: its one record is a recreation record (ADR-059).
    let snapshot = p.t().runtime().snapshot();
    let decoded = Snapshot::decode(&mut Reader::new(&snapshot)).unwrap();
    assert_eq!(decoded.stores.len(), 1);
    assert_eq!(decoded.stores[0].handle, handle);
    assert_eq!(decoded.stores[0].type_id, TodosQuery::ID);
    assert!(decoded.stores[0].recreation().is_some());
    p.t().runtime().restore(&snapshot).unwrap();
    // Unlike an object that is not in a snapshot, the handle is not stale: the platform runs
    // nothing to re-create it, and the cache outlived the restore.
    assert_eq!(p.method(handle, REFETCH_METHOD_ID).status, ReplyStatus::Ok);
    let again = p.construct::<TodosQuery>(&0_u32);
    let cs = p.observe(again);
    assert_eq!(
        entry(&cs, DATA).value,
        Some(page(vec![todo(1, "milk")])).encode_to_vec(),
        "the cache outlived the restore"
    );
}

// ----- mutations as functions ----------------------------------------------------------------

/// Calls the mutation `M` as a free function and runs the executor; returns the one reply.
fn call_mutation<M: MutationDef>(p: &Platform, input: &impl Encode) -> ReplyRecord {
    let id = p.call_id();
    assert_eq!(
        p.t().call(
            CallTarget::Function { method_id: M::ID },
            id,
            &input.encode_to_vec()
        ),
        0
    );
    p.t().run_pending();
    let reply = one(p.t().take_replies());
    assert_eq!(reply.call_id, id);
    reply
}

#[test]
fn a_mutation_called_as_a_function_replies_with_its_result() {
    let p = Platform::new();
    p.h.fakes.http.respond(
        undra_ports::fakes::Matcher::post(format!("{API}/todos")),
        ok(&todo(9, "bread")),
    );
    let reply = call_mutation::<AddTodoMutation>(&p, &"bread".to_owned());
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(Todo::decode_exact(&reply.body).unwrap(), todo(9, "bread"));

    // The idempotent mutation carried a key, a version 4 UUID, in its request.
    let request = p.h.fakes.http.last_call().unwrap();
    let key = request
        .header("Idempotency-Key")
        .expect("an idempotency key")
        .to_owned();
    assert_eq!(key.len(), 36);
    assert_eq!(&key[14..15], "4", "{key}");
}

#[test]
fn a_mutation_error_is_a_typed_error_reply() {
    let p = Platform::new();
    p.h.fakes.http.respond(
        undra_ports::fakes::Matcher::post(format!("{API}/todos")),
        undra_ports::HttpResponse::new(422, b"title too short".to_vec()),
    );
    let reply = call_mutation::<AddTodoMutation>(&p, &"x".to_owned());
    assert_eq!(reply.status, ReplyStatus::Error);
    assert_eq!(
        TodoError::decode_exact(&reply.body).unwrap(),
        TodoError::Rejected("title too short".into())
    );
}

#[test]
fn a_mutation_with_no_parameters_takes_no_arguments() {
    let p = Platform::new();
    p.h.fakes.http.respond(
        undra_ports::fakes::Matcher::method(undra_ports::HttpMethod::Delete),
        undra_ports::HttpResponse::new(204, Vec::new()),
    );
    let reply = call_mutation::<ClearTodosMutation>(&p, &());
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert!(
        reply.body.is_empty(),
        "a mutation returning () replies with an empty body"
    );
}

#[test]
fn a_mutation_needs_the_async_path_and_validates_its_arguments() {
    let p = Platform::new();
    let sync = p.t().call_sync(
        CallTarget::Function {
            method_id: AddTodoMutation::MUTATION_ID,
        },
        1,
        &"bread".to_owned().encode_to_vec(),
    );
    assert_eq!(sync.status, ReplyStatus::BadRequest);
    assert!(reason(&sync).contains("asynchronous"), "{}", reason(&sync));

    let bad = p.t().call(
        CallTarget::Function {
            method_id: AddTodoMutation::MUTATION_ID,
        },
        2,
        &[1],
    );
    assert_eq!(bad, 0);
    let reply = one(p.t().take_replies());
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert_eq!(p.h.http_calls(), 0, "nothing ran");
}

#[test]
fn a_cancelled_mutation_call_stops_and_replies_cancelled() {
    let p = Platform::new();
    p.h.fakes.http.respond(
        undra_ports::fakes::Matcher::post(format!("{API}/todos")),
        ok(&todo(9, "bread")),
    );
    let id = p.call_id();
    p.t().call(
        CallTarget::Function {
            method_id: AddTodoMutation::MUTATION_ID,
        },
        id,
        &"bread".to_owned().encode_to_vec(),
    );
    p.t().runtime().cancel(id);
    p.t().run_pending();
    let reply = one(p.t().take_replies());
    assert_eq!((reply.call_id, reply.status), (id, ReplyStatus::Cancelled));
}

#[test]
fn a_successful_mutation_refetches_the_observed_queries_its_key_names() {
    let p = Platform::new();
    p.h.serve_page(0, vec![todo(1, "milk")]);
    p.h.serve_page(1, vec![todo(2, "eggs")]);
    p.h.fakes
        .http
        .respond(format!("{API}/hello/a"), ok(&"hi".to_owned()));
    let observed = p.construct::<TodosQuery>(&0_u32);
    let other_page = p.construct::<TodosQuery>(&1_u32);
    p.observe(observed);
    p.observe(other_page);
    p.t().run_pending();
    p.change_sets();
    p.h.fakes.http.take_calls();
    p.h.fakes.http.respond(
        undra_ports::fakes::Matcher::post(format!("{API}/todos")),
        ok(&todo(9, "bread")),
    );

    // `add_todo` has `key = "todos"`: every entry whose key starts with it is stale.
    let reply = call_mutation::<AddTodoMutation>(&p, &"bread".to_owned());
    assert_eq!(reply.status, ReplyStatus::Ok);
    let mut urls: Vec<String> =
        p.h.fakes
            .http
            .take_calls()
            .into_iter()
            .map(|r| r.url)
            .collect();
    urls.sort();
    assert_eq!(
        urls,
        [
            format!("{API}/todos"),
            format!("{API}/todos?page=0"),
            format!("{API}/todos?page=1")
        ]
    );
    assert!(
        p.change_sets()
            .iter()
            .any(|cs| cs.entries.iter().any(|e| e.signal_id == FETCHING))
    );
}

// ----- optimistic updates and rollback, as the platform sees them -------------------------------

#[test]
fn an_optimistic_update_is_one_change_set_and_its_rollback_is_one_more_that_restores_exactly() {
    let p = Platform::new();
    p.h.serve_page(0, vec![todo(1, "milk")]);
    let handle = p.construct::<TodosQuery>(&0_u32);
    p.observe(handle);
    p.t().run_pending();
    p.change_sets();
    let original = Some(page(vec![todo(1, "milk")])).encode_to_vec();

    // The server will reject the mutation.
    p.h.fakes.http.respond(
        undra_ports::fakes::Matcher::post(format!("{API}/todos")),
        undra_ports::HttpResponse::new(422, b"no".to_vec()),
    );
    let ctx = p.h.ctx();
    let result = p.t().run_until(async move {
        ctx.mutate::<AddTodoMutation>(("eggs".to_owned(),))
            .optimistic(|cache| {
                assert!(cache.update::<TodosQuery>((0,), |page| {
                    page.items.push(todo(2, "eggs"));
                    page.total += 1;
                }));
            })
            .await
    });
    assert_eq!(result, Err(TodoError::Rejected("no".into())));

    let change_sets = p.change_sets();
    assert_eq!(
        change_sets.len(),
        2,
        "one transaction for the optimistic write, one for the rollback: {change_sets:?}"
    );
    let optimistic = &change_sets[0];
    assert_eq!(
        signal_ids(optimistic),
        [DATA],
        "only the data changed (same clock)"
    );
    assert_eq!(
        entry(optimistic, DATA).value,
        Some(Page {
            items: vec![todo(1, "milk"), todo(2, "eggs")],
            total: 2
        })
        .encode_to_vec()
    );
    let rollback = &change_sets[1];
    assert_eq!(signal_ids(rollback), [DATA]);
    assert_eq!(
        entry(rollback, DATA).value,
        original,
        "restored to exactly what it was"
    );
}

#[test]
fn a_rollback_that_leaves_an_entry_another_mutation_wrote_is_still_one_transaction() {
    let p = Platform::new();
    for n in 0..3_u32 {
        p.h.serve_page(n, vec![todo(n as u8 + 1, "milk")]);
    }
    let handles: Vec<Handle> = (0..3_u32).map(|n| p.construct::<TodosQuery>(&n)).collect();
    for handle in &handles {
        p.observe(*handle);
    }
    p.t().run_pending();
    p.change_sets();
    p.h.fakes.http.respond_sequence(
        undra_ports::fakes::Matcher::post(format!("{API}/todos")),
        [
            Ok(undra_ports::HttpResponse::new(422, b"no".to_vec())),
            Ok(ok(&todo(9, "b"))),
        ],
    );

    // A writes pages 0, 1 and 2 and is refused after 100 ms; B writes page 2 and is accepted
    // after 600 ms.
    let (a, _) = spawn(
        &p.h,
        p.h.ctx()
            .mutate::<SlowAddMutation>(("a".to_owned(), 100))
            .optimistic(|cache| {
                for n in 0..3_u32 {
                    cache.update::<TodosQuery>((n,), |page| page.items.push(todo(7, "a")));
                }
            }),
    );
    let (b, _) = spawn(
        &p.h,
        p.h.ctx()
            .mutate::<SlowAddMutation>(("b".to_owned(), 600))
            .optimistic(|cache| {
                cache.update::<TodosQuery>((2,), |page| page.items.push(todo(8, "b")));
            }),
    );
    p.t().run_pending();
    assert_eq!(
        p.change_sets().len(),
        4,
        "A's write is one transaction (three stores), B's one more (one store)"
    );

    p.h.advance_ms(100);
    assert_eq!(take(&a), Some(Err(TodoError::Rejected("no".into()))));
    assert!(take(&b).is_none());
    let rollback = p.change_sets();
    let mut restored: Vec<Handle> = rollback
        .iter()
        .flat_map(|cs| cs.entries.iter().map(|e| e.handle))
        .collect();
    restored.sort_by_key(|handle| handle.0);
    restored.dedup();
    assert_eq!(
        restored,
        handles[..2],
        "pages 0 and 1 are restored; page 2 belongs to B now and is not touched"
    );
    assert_eq!(rollback.len(), 2, "one change-set per restored store");
    assert_eq!(
        rollback[0].txn_id, rollback[1].txn_id,
        "the restores are one transaction"
    );
}

// ----- the raw bytes, straight from SPEC 3.3 and 3.4 -----------------------------------------------

#[test]
fn a_platform_that_builds_its_own_payloads_gets_the_same_answers() {
    let p = Platform::new();
    p.h.serve_page(0, vec![todo(1, "milk")]);

    // Constructor: `target u8 = 2, type_id u32, method_id u32, call_id u32, args`.
    let mut call = vec![2_u8];
    call.extend_from_slice(&TodosQuery::QUERY_ID.to_le_bytes());
    call.extend_from_slice(&TodosQuery::QUERY_ID.to_le_bytes());
    call.extend_from_slice(&7_u32.to_le_bytes());
    call.extend_from_slice(&0_u32.to_le_bytes()); // page 0
    let reply = p.t().runtime().call_sync(&call);
    // Reply: `call_id u32, status u8 (0 = ok), body` with the handle as a u64.
    assert_eq!(&reply[..4], &7_u32.to_le_bytes());
    assert_eq!(reply[4], 0);
    let handle = Handle(u64::from_le_bytes(reply[5..13].try_into().unwrap()));
    assert_eq!(reply.len(), 13);

    // Object method: `target u8 = 1, handle u64, method_id u32, call_id u32`, no args.
    let mut call = vec![1_u8];
    call.extend_from_slice(&handle.0.to_le_bytes());
    call.extend_from_slice(&0x21d1_b9e2_u32.to_le_bytes()); // refetch
    call.extend_from_slice(&8_u32.to_le_bytes());
    let reply = p.t().runtime().call_sync(&call);
    assert_eq!(reply, [8, 0, 0, 0, 0], "call_id 8, status ok, empty body");

    // The same call on a handle nobody issued: status 5 (bad request) with a reason.
    let mut call = vec![1_u8];
    call.extend_from_slice(&0xdead_beef_u64.to_le_bytes());
    call.extend_from_slice(&0x21d1_b9e2_u32.to_le_bytes());
    call.extend_from_slice(&9_u32.to_le_bytes());
    let reply = p.t().runtime().call_sync(&call);
    assert_eq!((&reply[..4], reply[4]), (&9_u32.to_le_bytes()[..], 5));

    // Free function (a mutation), async: `target u8 = 0, handle u64 = 0, method_id u32,
    // call_id u32, args`; `Runtime::call` answers 0 (accepted) and the reply follows.
    p.h.fakes.http.respond(
        undra_ports::fakes::Matcher::post(format!("{API}/todos")),
        ok(&todo(9, "x")),
    );
    let mut call = vec![0_u8];
    call.extend_from_slice(&0_u64.to_le_bytes());
    call.extend_from_slice(&AddTodoMutation::MUTATION_ID.to_le_bytes());
    call.extend_from_slice(&10_u32.to_le_bytes());
    call.extend_from_slice(&1_u32.to_le_bytes()); // the title: a string of one byte ...
    call.extend_from_slice(b"x");
    assert_eq!(p.t().runtime().call(&call), 0);
    p.t().run_pending();
    let reply = one(p.t().take_replies());
    assert_eq!((reply.call_id, reply.status), (10, ReplyStatus::Ok));
    assert_eq!(Todo::decode_exact(&reply.body).unwrap(), todo(9, "x"));
}
