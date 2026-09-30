//! Cache keys and invalidation targets.

use std::sync::Arc;

use keel_wire::Encode;

use crate::defs::QueryDef;
use crate::shared::Entry;

/// What identifies a cache entry: the query and its encoded parameters (SPEC 9's `QueryKey`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct QueryKey {
    /// `fnv1a32("query.<fn>")`.
    pub(crate) query_id: u32,
    /// The parameters, encoded canonically.
    pub(crate) params: Arc<[u8]>,
}

impl QueryKey {
    pub(crate) fn new(query_id: u32, params: Arc<[u8]>) -> QueryKey {
        QueryKey { query_id, params }
    }
}

/// Which cache entries an invalidation applies to.
///
/// Invalidated entries are marked stale; the ones something observes refetch at once.
///
/// A *prefix* is matched against the entry's rendered key: the query's key template with the
/// parameters spliced in (`todos:{page}` for page 3 is `todos:3`). So `"todos"` invalidates
/// every page, `"todos:3"` page 3 only. Parameter values are rendered as plain text (numbers
/// and strings as they are, a `Uuid` hyphenated, an enum as its variant name, a list as
/// `[a,b]`).
///
/// A `&str` or `String` converts to a prefix, so `.invalidates(["todos"])` reads naturally.
///
/// ```
/// use keel_query::Invalidate;
///
/// assert_eq!(Invalidate::from("todos"), Invalidate::prefix("todos"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalidate {
    /// Every entry whose rendered key starts with this text. The empty prefix matches all.
    Prefix(String),
    /// Every entry of one query, whatever its parameters.
    Query(u32),
    /// The entry of one query with exactly these encoded parameters.
    Exact {
        /// The query's id.
        query_id: u32,
        /// The parameters, encoded.
        params: Vec<u8>,
    },
}

impl Invalidate {
    /// Entries whose rendered key starts with `prefix`.
    pub fn prefix(prefix: impl Into<String>) -> Invalidate {
        Invalidate::Prefix(prefix.into())
    }

    /// Every entry of query `Q`.
    pub fn query<Q: QueryDef>() -> Invalidate {
        Invalidate::Query(Q::ID)
    }

    /// The entry of query `Q` with these parameters.
    pub fn exact<Q: QueryDef>(params: &Q::Params) -> Invalidate {
        Invalidate::Exact {
            query_id: Q::ID,
            params: params.encode_to_vec(),
        }
    }

    pub(crate) fn matches(&self, key: &QueryKey, entry: &Entry) -> bool {
        match self {
            Invalidate::Prefix(prefix) => entry.rendered.starts_with(prefix.as_str()),
            Invalidate::Query(id) => key.query_id == *id,
            Invalidate::Exact { query_id, params } => {
                key.query_id == *query_id && *key.params == params[..]
            }
        }
    }
}

impl From<&str> for Invalidate {
    fn from(prefix: &str) -> Invalidate {
        Invalidate::Prefix(prefix.to_owned())
    }
}

impl From<String> for Invalidate {
    fn from(prefix: String) -> Invalidate {
        Invalidate::Prefix(prefix)
    }
}

impl From<&String> for Invalidate {
    fn from(prefix: &String) -> Invalidate {
        Invalidate::Prefix(prefix.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::{BoxFuture, QueryDef};
    use crate::erased::query_vtable;
    use keel_runtime::Ctx;

    struct Q;

    impl QueryDef for Q {
        const ID: u32 = 21;
        const KEY: &'static str = "q:{n}";
        const STALE_MS: Option<u64> = None;
        const PERSIST: bool = false;
        const RETRY: u32 = 0;
        type Params = (u8,);
        type Output = u8;
        type Error = u8;
        fn fetch(_: Ctx, _: (u8,)) -> BoxFuture<Result<u8, u8>> {
            Box::pin(async { Ok(1) })
        }
    }

    fn keyed(n: u8) -> (QueryKey, Entry) {
        let key = QueryKey::new(Q::ID, Arc::from((n,).encode_to_vec()));
        let entry = Entry::new(query_vtable::<Q>(), format!("q:{n}"));
        (key, entry)
    }

    #[test]
    fn prefixes_match_the_rendered_key_from_the_start() {
        let (key, entry) = keyed(3);
        assert!(Invalidate::prefix("q").matches(&key, &entry));
        assert!(Invalidate::prefix("q:").matches(&key, &entry));
        assert!(Invalidate::prefix("q:3").matches(&key, &entry));
        assert!(Invalidate::prefix("").matches(&key, &entry));
        assert!(!Invalidate::prefix("q:4").matches(&key, &entry));
        assert!(
            !Invalidate::prefix("3").matches(&key, &entry),
            "a prefix, not a substring"
        );
        assert!(!Invalidate::prefix("q:33").matches(&key, &entry));
    }

    #[test]
    fn typed_targets_match_by_id_and_by_exact_parameters() {
        let (key, entry) = keyed(3);
        assert!(Invalidate::query::<Q>().matches(&key, &entry));
        assert!(Invalidate::exact::<Q>(&(3,)).matches(&key, &entry));
        assert!(!Invalidate::exact::<Q>(&(4,)).matches(&key, &entry));
        assert!(!Invalidate::Query(22).matches(&key, &entry));
        assert!(
            !Invalidate::Exact {
                query_id: 22,
                params: (3_u8,).encode_to_vec()
            }
            .matches(&key, &entry)
        );
    }

    #[test]
    fn strings_convert_to_prefixes() {
        assert_eq!(
            Invalidate::from("todos"),
            Invalidate::Prefix("todos".into())
        );
        assert_eq!(
            Invalidate::from("todos".to_owned()),
            Invalidate::prefix("todos")
        );
        let owned = "todos".to_owned();
        assert_eq!(Invalidate::from(&owned), Invalidate::prefix("todos"));
    }

    #[test]
    fn keys_are_equal_when_id_and_params_are() {
        let a = QueryKey::new(1, Arc::from(vec![1_u8, 2]));
        let b = QueryKey::new(1, Arc::from(vec![1_u8, 2]));
        assert_eq!(a, b);
        assert_ne!(a, QueryKey::new(2, Arc::from(vec![1_u8, 2])));
        assert_ne!(a, QueryKey::new(1, Arc::from(vec![1_u8])));
        let mut set = std::collections::HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }
}
