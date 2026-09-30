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
