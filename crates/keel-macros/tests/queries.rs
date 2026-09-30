//! Behaviour tests for `#[keel::query]` and `#[keel::mutation]`: the generated `QueryDef` and
//! `MutationDef` implementations are run through the traits of `keel::query`.
#![forbid(unsafe_code)]

use keel::meta::{QueryKind, TypeRef, collect_schema, ids};
use keel::query::{MutationDef, QueryDef};
use keel::runtime::Ctx;
use keel::wire::Encode;
use keel_macros as k;

mod support;
use support::Runtime;
use support::testing::block_on;

/// Reads a constant flag at run time (clippy rejects asserting on constants).
fn flag(value: bool) -> bool {
    value
}

#[k::error]
#[derive(Clone, PartialEq)]
pub enum HttpError {
    #[error("timeout")]
    Timeout,
}

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub id: u32,
    pub title: String,
}

/// Lists todos.
#[k::query(key = "todos:{page}:{q}", stale = "30s", persist, retry = 5)]
pub async fn todos(ctx: &Ctx, page: u32, q: String) -> Result<Vec<Todo>, HttpError> {
    let _ = ctx.clone();
    if page == 99 {
        Err(HttpError::Timeout)
    } else {
        Ok(vec![Todo { id: page, title: q }])
    }
}

#[k::query(key = "count", stale = "2h", idempotent)]
async fn count(ctx: Ctx) -> Result<u32, HttpError> {
    let _ = ctx;
    Ok(3)
}

#[k::mutation(idempotent, retry = 2, key = "todos")]
pub async fn add_todo(ctx: &Ctx, title: String, done: bool) -> Result<Todo, HttpError> {
    let _ = ctx;
    Ok(Todo {
        id: u32::from(done),
        title,
    })
}

#[k::mutation]
pub async fn wipe(ctx: &Ctx) -> Result<(), HttpError> {
    let _ = ctx;
    Err(HttpError::Timeout)
}

#[test]
fn query_constants_follow_the_arguments() {
    assert_eq!(TodosQuery::QUERY_ID, ids::query_id("todos"));
    assert_eq!(TodosQuery::KEY, "todos:{page}:{q}");
    assert_eq!(TodosQuery::STALE_MS, Some(30_000));
    assert!(flag(TodosQuery::PERSIST));
    assert_eq!(TodosQuery::RETRY, 5);
    assert!(!flag(TodosQuery::IDEMPOTENT));

    assert_eq!(CountQuery::STALE_MS, Some(7_200_000));
    assert!(!flag(CountQuery::PERSIST));
    assert_eq!(CountQuery::RETRY, 3, "queries retry three times by default");
    assert!(flag(CountQuery::IDEMPOTENT));
}

#[test]
fn the_query_def_trait_mirrors_the_constants() {
    fn check<Q: QueryDef>() -> (u32, &'static str, Option<u64>, bool, u32) {
        (Q::ID, Q::KEY, Q::STALE_MS, Q::PERSIST, Q::RETRY)
    }
    assert_eq!(
        check::<TodosQuery>(),
        (
            ids::query_id("todos"),
            "todos:{page}:{q}",
            Some(30_000),
            true,
            5
        )
    );
    assert_eq!(check::<CountQuery>().0, ids::query_id("count"));
}

#[test]
fn fetch_runs_the_function_with_the_tuple_of_parameters() {
    let ctx = Runtime::new().ctx();
    let ok = block_on(TodosQuery::fetch(ctx.clone(), (2, "milk".to_owned()))).unwrap();
    assert_eq!(
        ok,
        vec![Todo {
            id: 2,
            title: "milk".into()
        }]
    );
    let err = block_on(TodosQuery::fetch(ctx.clone(), (99, String::new()))).unwrap_err();
    assert_eq!(err, HttpError::Timeout);
    // A single by-value `Ctx` parameter and no other parameters.
    assert_eq!(block_on(CountQuery::fetch(ctx, ())).unwrap(), 3);
}

#[test]
fn associated_types_are_the_tuple_the_value_and_the_error() {
    fn params<Q: QueryDef<Params = (u32, String), Output = Vec<Todo>, Error = HttpError>>() {}
    params::<TodosQuery>();
    fn unit<Q: QueryDef<Params = (), Output = u32, Error = HttpError>>() {}
    unit::<CountQuery>();
    // `Params` is `Encode`: the query cache keys entries by its encoded bytes.
    assert_eq!(
        (7_u32, "a".to_owned()).encode_to_vec(),
        [7, 0, 0, 0, 1, 0, 0, 0, b'a']
    );
}

#[test]
fn mutation_constants_and_defaults() {
    assert_eq!(AddTodoMutation::MUTATION_ID, ids::mutation_id("add_todo"));
    assert_eq!(AddTodoMutation::KEY, "todos");
    assert_eq!(AddTodoMutation::RETRY, 2);
    assert!(flag(AddTodoMutation::IDEMPOTENT));
    assert_eq!(
        WipeMutation::RETRY,
        0,
        "mutations do not retry unless asked"
    );
    assert!(!flag(WipeMutation::IDEMPOTENT));
    assert_eq!(WipeMutation::KEY, "");
    assert_eq!(WipeMutation::STALE_MS, None);
    assert!(!flag(WipeMutation::PERSIST));
}

#[test]
fn execute_runs_the_mutation() {
    let ctx = Runtime::new().ctx();
    let todo = block_on(AddTodoMutation::execute(
        ctx.clone(),
        ("write".to_owned(), true),
    ))
    .unwrap();
    assert_eq!(
        todo,
        Todo {
            id: 1,
            title: "write".into()
        }
    );
    assert_eq!(
        block_on(WipeMutation::execute(ctx, ())).unwrap_err(),
        HttpError::Timeout
    );
    fn ids_of<M: MutationDef>() -> (u32, u32, bool) {
        (M::ID, M::RETRY, M::IDEMPOTENT)
    }
    assert_eq!(
        ids_of::<AddTodoMutation>(),
        (ids::mutation_id("add_todo"), 2, true)
    );
}

#[test]
fn query_meta_describes_params_returns_and_flags() {
    let schema = collect_schema("queries-test");
    let query = |name: &str| schema.queries.iter().find(|q| q.name == name).unwrap();

    let todos = query("todos");
    assert_eq!(todos.kind, QueryKind::Query);
    assert_eq!(todos.query_id, ids::query_id("todos"));
    assert_eq!(todos.key, "todos:{page}:{q}");
    assert_eq!(todos.stale_ms, Some(30_000));
    assert!(todos.persist && !todos.idempotent);
    assert_eq!(
        todos
            .params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["page", "q"]
    );
    assert_eq!(todos.params[0].ty, TypeRef::U32);
    assert_eq!(
        todos.returns,
        TypeRef::result(
            TypeRef::vec(TypeRef::named("Todo")),
            TypeRef::named("HttpError")
        )
    );

    let add = query("add_todo");
    assert_eq!(add.kind, QueryKind::Mutation);
    assert_eq!(add.query_id, ids::mutation_id("add_todo"));
    assert!(add.idempotent && !add.persist);
    assert_eq!(add.stale_ms, None);
    assert_eq!(add.key, "todos");
    assert_eq!(
        query("wipe").returns,
        TypeRef::result(TypeRef::Unit, TypeRef::named("HttpError"))
    );
    assert!(
        query("count").params.is_empty(),
        "Ctx is not a schema parameter"
    );
}

#[test]
fn the_registered_schema_with_queries_validates() {
    collect_schema("queries-test")
        .validate()
        .unwrap_or_else(|errors| panic!("{errors:#?}"));
}
