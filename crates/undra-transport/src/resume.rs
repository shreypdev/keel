//! Session resume: what the server keeps for a client that dropped, so that it can come back to
//! the same objects (ADR-051).
//!
//! A client that wants this puts a random token in the URL of its WebSocket upgrade
//! (`?undra_session=<token>`), the same on every connection of one `UndraCore`; a reconnecting
//! client that holds constructed objects adds `&undra_resume=1`. The token is URL material, not
//! envelope material: nothing on the wire changes, and a client that sends none is served as it
//! always was (its objects are released when it disconnects).
//!
//! With [`ServerConfig::resume_grace`](crate::ServerConfig::resume_grace) above zero, tearing down
//! an attached client that sent a token **retains** the objects its constructors made instead of
//! releasing them. They are released when the grace passes, when a client with another token (or
//! none) attaches, or when the server stops; a connection that presents the token and asks to
//! resume **adopts** them. At most one session is retained: `undra dev` serves one client at a
//! time, and a dev core that outlives many app launches must not keep every launch's stores.

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use undra_runtime::Runtime;
use undra_runtime::log::INFO;

const TARGET: &str = "undra::transport";

/// The query parameter that carries the client's session token.
pub(crate) const SESSION_PARAM: &str = "undra_session";

/// The query parameter a reconnecting client sets (to `1`) when it holds objects it expects to
/// find again.
pub(crate) const RESUME_PARAM: &str = "undra_resume";

/// The longest token accepted.
const MAX_TOKEN_LEN: usize = 64;

/// What a connection said about its session in the query of its upgrade request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRequest {
    /// The client's token: 1 to 64 characters of `A-Z a-z 0-9 . _ -`.
    pub(crate) token: String,
    /// Whether the client asked to find its objects again.
    pub(crate) resume: bool,
}

impl SessionRequest {
    /// The token cut short for a log line.
    pub(crate) fn short(&self) -> &str {
        short_token(&self.token)
    }
}

/// The first eight characters of a token (tokens are ASCII).
pub(crate) fn short_token(token: &str) -> &str {
    &token[..token.len().min(8)]
}

/// Reads the session parameters out of the query of a request URI. `None` when there is no
/// valid token: the connection is then an ordinary one.
pub(crate) fn parse_query(query: Option<&str>) -> Option<SessionRequest> {
    let mut token = None;
    let mut resume = false;
    for pair in query?.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            SESSION_PARAM => token = valid_token(value).then(|| value.to_owned()),
            RESUME_PARAM => resume = value == "1",
            _ => {}
        }
    }
    token.map(|token| SessionRequest { token, resume })
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOKEN_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// What a dropped client left behind.
#[derive(Debug)]
pub(crate) struct Retained {
    pub(crate) token: String,
    pub(crate) handles: Vec<u64>,
    expires: Instant,
    since: Instant,
}

impl Retained {
    /// How long the client has been gone.
    pub(crate) fn away(&self) -> Duration {
        self.since.elapsed()
    }
}

#[derive(Default)]
struct State {
    retained: Option<Retained>,
    stopping: bool,
}

/// The retained session, with the clock that ends it.
pub(crate) struct Resume {
    grace: Duration,
    state: Mutex<State>,
    wake: Condvar,
}

impl Resume {
    /// A registry that keeps a session for `grace`; zero switches resuming off.
    pub(crate) fn new(grace: Duration) -> Arc<Resume> {
        Arc::new(Resume {
            grace,
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        })
    }

    /// Whether sessions are kept at all.
    pub(crate) fn enabled(&self) -> bool {
        !self.grace.is_zero()
    }

    /// How long a session is kept.
    pub(crate) fn grace(&self) -> Duration {
        self.grace
    }

    /// Keeps `handles` for the client with `token`. Returns the handles of a session that was
    /// retained before and is now replaced (the caller releases them).
    pub(crate) fn retain(&self, token: &str, handles: Vec<u64>) -> Vec<u64> {
        let mut state = self.state.lock();
        let now = Instant::now();
        let replaced = state.retained.replace(Retained {
            token: token.to_owned(),
            handles,
            expires: now + self.grace,
            since: now,
        });
        self.wake.notify_all();
        replaced.map(|r| r.handles).unwrap_or_default()
    }

    /// Hands over the retained session when it is `token`'s and has not expired.
    pub(crate) fn take(&self, token: &str) -> Option<Retained> {
        let mut state = self.state.lock();
        let live = state
            .retained
            .as_ref()
            .is_some_and(|r| r.token == token && Instant::now() < r.expires);
        if !live {
            return None;
        }
        self.wake.notify_all();
        state.retained.take()
    }

    /// Removes the retained session whatever its token: a client attached that is not resuming
    /// it. The caller releases the handles.
    pub(crate) fn supersede(&self) -> Option<Retained> {
        let mut state = self.state.lock();
        self.wake.notify_all();
        state.retained.take()
    }

    /// Whether a session is being kept (tests).
    #[cfg(test)]
    pub(crate) fn holds(&self) -> bool {
        self.state.lock().retained.is_some()
    }

    /// Starts the thread that releases a session when its grace passes. It ends with
    /// [`Resume::stop`].
    pub(crate) fn spawn_reaper(self: &Arc<Self>, rt: Arc<Runtime>) -> std::io::Result<JoinHandle<()>> {
        let this = self.clone();
        thread::Builder::new()
            .name("undra-transport-resume".to_owned())
            .spawn(move || this.reap(&rt))
    }

    fn reap(&self, rt: &Runtime) {
        let mut state = self.state.lock();
        loop {
            if state.stopping {
                return;
            }
            let Some(expires) = state.retained.as_ref().map(|r| r.expires) else {
                self.wake.wait(&mut state);
                continue;
            };
            if Instant::now() < expires {
                self.wake.wait_until(&mut state, expires);
                continue;
            }
            let Some(gone) = state.retained.take() else { continue };
            drop(state);
            release_all(rt, &gone.handles);
            rt.log(
                INFO,
                TARGET,
                &format!(
                    "session {} expired after {} s without its client: released {} object(s)",
                    short_token(&gone.token),
                    self.grace.as_secs(),
                    gone.handles.len()
                ),
            );
            state = self.state.lock();
        }
    }

    /// Stops the reaper and returns what was still retained (the caller releases it).
    pub(crate) fn stop(&self) -> Option<Retained> {
        let mut state = self.state.lock();
        state.stopping = true;
        self.wake.notify_all();
        state.retained.take()
    }
}

/// Releases `handles` in the runtime.
pub(crate) fn release_all(rt: &Runtime, handles: &[u64]) {
    for handle in handles {
        rt.release(*handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_comes_from_the_query() {
        let both = parse_query(Some("undra_session=abc-123_x.y&undra_resume=1")).unwrap();
        assert_eq!(both.token, "abc-123_x.y");
        assert!(both.resume);
        let plain = parse_query(Some("undra_session=abc")).unwrap();
        assert!(!plain.resume);
        let other = parse_query(Some("a=b&undra_resume=1&undra_session=zz&c")).unwrap();
        assert_eq!((other.token.as_str(), other.resume), ("zz", true));
    }

    #[test]
    fn a_missing_or_malformed_token_is_no_session() {
        for query in [
            None,
            Some(""),
            Some("undra_resume=1"),
            Some("undra_session="),
            Some("undra_session=a%20b"),
            Some("undra_session=a/b"),
        ] {
            assert_eq!(parse_query(query), None, "{query:?}");
        }
        let long = format!("undra_session={}", "a".repeat(MAX_TOKEN_LEN + 1));
        assert_eq!(parse_query(Some(&long)), None);
        let longest = format!("undra_session={}", "a".repeat(MAX_TOKEN_LEN));
        assert!(parse_query(Some(&longest)).is_some());
    }

    #[test]
    fn a_session_is_taken_once_and_only_by_its_token() {
        let resume = Resume::new(Duration::from_secs(60));
        assert!(resume.retain("t1", vec![1, 2]).is_empty());
        assert!(resume.take("other").is_none());
        assert!(resume.holds(), "a wrong token does not consume the session");
        let got = resume.take("t1").expect("the owner takes it");
        assert_eq!(got.handles, [1, 2]);
        assert!(resume.take("t1").is_none(), "taken once");
    }

    #[test]
    fn retaining_again_hands_back_what_it_replaces() {
        let resume = Resume::new(Duration::from_secs(60));
        resume.retain("a", vec![1]);
        assert_eq!(resume.retain("b", vec![2]), [1]);
        assert_eq!(resume.supersede().map(|r| r.token), Some("b".to_owned()));
        assert!(!resume.holds());
    }

    #[test]
    fn an_expired_session_is_not_resumable() {
        let resume = Resume::new(Duration::from_millis(1));
        resume.retain("t", vec![9]);
        thread::sleep(Duration::from_millis(10));
        assert!(resume.take("t").is_none());
    }
}
