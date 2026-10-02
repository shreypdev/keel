//! The devtools of a dev server (ADR-054): a page served from the dev server's own listener, and
//! the connection it talks to the core through.
//!
//! Everything here exists only when [`ServerConfig::devtools`](crate::ServerConfig::devtools) is
//! set, which only the dev runner `undra dev` generates does. A production core has no `Server` at
//! all, and a server without that setting answers every `/devtools` request with `404`.
//!
//! * [`proto`]: the messages of the connection (documented, and public so a test client can speak
//!   them).
//! * `hub`: observes every store while a page is attached, routes change-sets, records port
//!   calls, keeps the ring of snapshots, and carries out time travel.
//! * `ring`: the bounded history of snapshots.
//! * `http`: `GET /devtools[/..]`, a fixed table of embedded assets.
//! * `serve`: the WebSocket upgrade at `/devtools/ws` and the connection's loop.
//!
//! # The token
//!
//! A page can read the state of the core and restore it, so the endpoint is not open to whoever
//! can reach the port: every request needs `?token=<DevtoolsConfig::token>` and a missing or
//! wrong token is answered with `404`, exactly like a path that does not exist, so that the
//! endpoint does not announce itself. `undra dev` makes a token per run and prints it in the URL.

pub(crate) mod http;
pub(crate) mod hub;
pub mod proto;
pub(crate) mod ring;
pub(crate) mod serve;

/// One file of the page, compiled into the program that serves it.
#[derive(Clone, Copy)]
pub struct Asset {
    /// The path below `/devtools/`, e.g. `app.js`; `index.html` is also served at `/devtools`.
    pub path: &'static str,
    /// The `Content-Type`.
    pub content_type: &'static str,
    /// The bytes. In `index.html` the text `__UNDRA_DEVTOOLS_TOKEN__` is replaced by the token when
    /// it is served, so the page can build its own URLs.
    pub bytes: &'static [u8],
}

impl std::fmt::Debug for Asset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Asset")
            .field("path", &self.path)
            .field("content_type", &self.content_type)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// What [`ServerConfig::devtools`](crate::ServerConfig::devtools) turns on.
#[derive(Clone)]
pub struct DevtoolsConfig {
    /// The secret every request must carry (`?token=`): at least 16 characters of `A-Z a-z 0-9`.
    /// A shorter or odd one disables devtools (the server logs why).
    pub token: String,
    /// The page. May be empty (the connection then works for a test client and `GET /devtools`
    /// answers `404`).
    pub assets: &'static [Asset],
    /// How many steps the ring keeps. Default 200.
    pub max_steps: usize,
    /// How many bytes of snapshots the ring keeps in all. Default 32 MiB.
    pub max_bytes: usize,
    /// The largest snapshot the ring keeps; a state over it is listed but cannot be restored.
    /// Default 4 MiB.
    pub max_step_bytes: usize,
    /// How many pages may be attached at once. Default 4.
    pub max_clients: usize,
}

impl std::fmt::Debug for DevtoolsConfig {
    // The token is a secret: it is never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevtoolsConfig")
            .field("token", &"(hidden)")
            .field("assets", &self.assets)
            .field("max_steps", &self.max_steps)
            .field("max_bytes", &self.max_bytes)
            .field("max_step_bytes", &self.max_step_bytes)
            .field("max_clients", &self.max_clients)
            .finish()
    }
}

impl DevtoolsConfig {
    /// A configuration with the defaults.
    pub fn new(token: impl Into<String>, assets: &'static [Asset]) -> DevtoolsConfig {
        DevtoolsConfig {
            token: token.into(),
            assets,
            max_steps: 200,
            max_bytes: 32 << 20,
            max_step_bytes: 4 << 20,
            max_clients: 4,
        }
    }

    /// Whether the token is usable.
    pub(crate) fn token_is_valid(&self) -> bool {
        self.token.len() >= 16 && self.token.bytes().all(|b| b.is_ascii_alphanumeric())
    }
}

/// Compares two tokens without stopping at the first difference.
pub(crate) fn token_matches(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    let mut diff = a.len() ^ b.len();
    for (i, x) in a.iter().enumerate() {
        diff |= usize::from(x ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

/// The value of `name` in a URL query (`a=1&token=abc`), not decoded.
pub(crate) fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(k, v)| (k == name).then_some(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_whole() {
        assert!(token_matches("abcdef0123456789", "abcdef0123456789"));
        assert!(!token_matches("abcdef0123456789", "abcdef0123456788"));
        assert!(!token_matches("abcdef0123456789", "abcdef012345678"));
        assert!(!token_matches("abcdef0123456789", ""));
        assert!(!token_matches("abcdef0123456789", "abcdef0123456789x"));
    }

    #[test]
    fn a_token_must_be_long_and_plain() {
        assert!(DevtoolsConfig::new("0123456789abcdef", &[]).token_is_valid());
        assert!(!DevtoolsConfig::new("short", &[]).token_is_valid());
        assert!(!DevtoolsConfig::new("0123456789abcde/", &[]).token_is_valid());
        assert!(!DevtoolsConfig::new("", &[]).token_is_valid());
    }

    #[test]
    fn the_query_is_split_on_ampersands() {
        assert_eq!(query_param("a=1&token=xyz&b=2", "token"), Some("xyz"));
        assert_eq!(query_param("token=", "token"), Some(""));
        assert_eq!(query_param("tok=1", "token"), None);
        assert_eq!(query_param("", "token"), None);
    }
}
