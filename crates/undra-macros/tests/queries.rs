//! Behaviour tests for `#[undra::query]` and `#[undra::mutation]`: the generated `QueryDef` and
//! `MutationDef` implementations are run through the traits of `undra::query`.
#![forbid(unsafe_code)]

use undra::meta::{QueryKind, TypeRef, collect_schema, ids};
use undra::query::{InfiniteQueryDef, MutationDef, QueryDef};
use undra::runtime::Ctx;
use undra::wire::Encode;
use undra_macros as k;

mod support;
use support::Runtime;
use support::testing::block_on;

/// Reads a constant flag at run time (clippy rejects asserting on constants).
fn flag(value: bool) -> bool {
    value
}

#[k::error]
#[derive(Clone, PartialEq)]
pub enum NetError {
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
pub async fn todos(ctx: &Ctx, page: u32, q: String) -> Result<Vec<Todo>, NetError> {
    let _ = ctx.clone();
    if page == 99 {
        Err(NetError::Timeout)
    } else {
        Ok(vec![Todo { id: page, title: q }])
    }
}

#[k::query(key = "count", stale = "2h", idempotent)]
async fn count(ctx: Ctx) -> Result<u32, NetError> {
    let _ = ctx;
    Ok(3)
}

/// A polled feed of todos, paged by an opaque cursor.
#[k::query(
    key = "feed/{tag}",
    infinite,
    item_key = "id",
    stale = "1m",
    interval = "1m",
    poll_in_background,
    refetch_pages = 2,
    persist,
    persist_pages = 3
)]
pub async fn feed(
    ctx: &Ctx,
    tag: String,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Todo, String>, NetError> {
    let _ = ctx;
    let from: u32 = cursor.as_deref().map_or(0, |c| c.parse().unwrap());
    let next = (from < 4).then(|| (from + 2).to_string());
    Ok(undra::query::Page::new(
        (from..from + 2)
            .map(|id| Todo {
                id,
                title: tag.clone(),
            })
            .collect(),
        next,
    ))
}

#[k::mutation(idempotent, retry = 2, key = "todos")]
pub async fn add_todo(ctx: &Ctx, title: String, done: bool) -> Result<Todo, NetError> {
    let _ = ctx;
    Ok(Todo {
        id: u32::from(done),
        title,
    })
}

#[k::mutation]
pub async fn wipe(ctx: &Ctx) -> Result<(), NetError> {
    let _ = ctx;
    Err(NetError::Timeout)
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
    assert_eq!(err, NetError::Timeout);
    // A single by-value `Ctx` parameter and no other parameters.
    assert_eq!(block_on(CountQuery::fetch(ctx, ())).unwrap(), 3);
}

#[test]
fn associated_types_are_the_tuple_the_value_and_the_error() {
    fn params<Q: QueryDef<Params = (u32, String), Output = Vec<Todo>, Error = NetError>>() {}
    params::<TodosQuery>();
    fn unit<Q: QueryDef<Params = (), Output = u32, Error = NetError>>() {}
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
        NetError::Timeout
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
            TypeRef::named("NetError")
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
        TypeRef::result(TypeRef::Unit, TypeRef::named("NetError"))
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

#[test]
fn polling_and_paging_constants_follow_the_arguments() {
    assert_eq!(FeedQuery::INTERVAL_MS, Some(60_000));
    assert!(flag(FeedQuery::POLL_IN_BACKGROUND));
    assert_eq!(FeedQuery::ITEM_KEY, "id");
    assert_eq!(FeedQuery::REFETCH_PAGES, Some(2));
    assert_eq!(FeedQuery::PERSIST_PAGES, 3);
    assert_eq!(<FeedQuery as QueryDef>::INTERVAL_MS, Some(60_000));
    assert!(flag(<FeedQuery as QueryDef>::POLL_IN_BACKGROUND));
    // An ordinary query does not poll, and `InfiniteQueryDef` is not implemented for it.
    assert_eq!(TodosQuery::INTERVAL_MS, None);
    assert!(!flag(TodosQuery::POLL_IN_BACKGROUND));
    assert_eq!(<CountQuery as QueryDef>::INTERVAL_MS, None);
    assert!(<CountQuery as QueryDef>::PAGED.is_none());
    assert!(<FeedQuery as QueryDef>::PAGED.is_some());
}

#[test]
fn an_infinite_query_is_a_query_def_of_its_list_and_an_infinite_def_of_its_pages() {
    fn list<Q: QueryDef<Params = (String,), Output = Vec<Todo>, Error = NetError>>() {}
    list::<FeedQuery>();
    fn pages<Q: InfiniteQueryDef<Item = Todo, Cursor = String>>() {}
    pages::<FeedQuery>();

    let ctx = Runtime::new().ctx();
    // One page at a cursor: `None` is the first page, the cursor goes where it was declared.
    let first = block_on(FeedQuery::fetch_page(ctx.clone(), ("a".to_owned(),), None)).unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next.as_deref(), Some("2"));
    let last = block_on(FeedQuery::fetch_page(
        ctx.clone(),
        ("a".to_owned(),),
        Some("4".to_owned()),
    ))
    .unwrap();
    assert_eq!(last.items[0].id, 4);
    assert_eq!(last.next, None);
    // `fetch` is the first page's rows.
    assert_eq!(
        block_on(FeedQuery::fetch(ctx, ("a".to_owned(),))).unwrap(),
        first.items
    );
}

#[test]
fn the_item_key_is_the_hash_of_the_encoded_key_field() {
    let todo = Todo {
        id: 7,
        title: "x".into(),
    };
    assert_eq!(
        FeedQuery::item_key(&todo),
        ids::fnv1a64(&7_u32.encode_to_vec())
    );
    let other = Todo {
        id: 8,
        title: "x".into(),
    };
    assert_ne!(FeedQuery::item_key(&todo), FeedQuery::item_key(&other));
    assert_eq!(
        FeedQuery::item_key(&todo),
        FeedQuery::item_key(&Todo {
            id: 7,
            title: "another title".into()
        }),
        "only the key field counts"
    );
}

#[test]
fn query_meta_of_a_polled_and_infinite_query() {
    let schema = collect_schema("queries-test");
    let query = |name: &str| schema.queries.iter().find(|q| q.name == name).unwrap();
    let feed = query("feed");
    assert_eq!(feed.interval_ms, Some(60_000));
    assert!(feed.poll_in_background);
    assert_eq!(
        feed.params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["tag"],
        "the cursor is not a schema parameter"
    );
    assert_eq!(
        feed.returns,
        TypeRef::result(
            TypeRef::vec(TypeRef::named("Todo")),
            TypeRef::named("NetError")
        )
    );
    let infinite = feed.infinite.as_ref().unwrap();
    assert_eq!(infinite.cursor, TypeRef::String);
    assert_eq!(infinite.item_key, "id");
    let todos = query("todos");
    assert_eq!((todos.interval_ms, todos.poll_in_background), (None, false));
    assert_eq!(todos.infinite, None);
}
