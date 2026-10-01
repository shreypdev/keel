//! Dev notices: one line the dev server says to a client that attaches soon after it started
//! (ADR-053), so that a status bar can say "Reloaded, state kept" for a few seconds.
//!
//! A notice is an ordinary `Log` record (SPEC 5.10) with the target [`NOTICE_TARGET`]; nothing
//! on the wire changes, and a client that ignores it loses nothing. Only a server whose
//! [`ServerConfig::attach_notices`](crate::ServerConfig::attach_notices) is set sends one, which
//! is `undra dev`'s runner after a reload: an in-process core and a production build never do.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// The `target` of the `Log` record that carries a dev notice; its message is the sentence to show.
///
/// Reserved for the server: a record the core itself logs under this target is printed by the
/// [`LogSink`](crate::LogSink) but never sent to a client, so a core cannot pass itself off as the
/// dev server.
pub const NOTICE_TARGET: &str = "undra::dev";

/// What a freshly started server tells the clients that attach to it (ADR-053).
///
/// Every client that attaches within [`window`](AttachNotices::window) of the server's start is
/// told once (per session token; a client without one is told per connection): a client that
/// **resumed** its session gets [`resumed`](AttachNotices::resumed), any other client gets
/// [`fresh`](AttachNotices::fresh). A client the server refuses (a session it cannot resume, a
/// schema it does not speak) is told nothing: it is not attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachNotices {
    /// The sentence for a client that resumed its session, e.g. `Reloaded, state kept`.
    pub resumed: Option<String>,
    /// The sentence for a client that did not, e.g. `Reloaded, state reset: schema changed`.
    pub fresh: Option<String>,
    /// How long after the server starts a client can still be told. Default 30 s: a reload is
    /// followed by the clients' reconnect backoff (at most five seconds a try), and a notice
    /// that arrives minutes later would describe something the developer has long moved past.
    pub window: Duration,
}

impl Default for AttachNotices {
    fn default() -> Self {
        AttachNotices {
            resumed: None,
            fresh: None,
            window: Duration::from_secs(30),
        }
    }
}

/// Which notice a client gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    /// The client resumed its session.
    Resumed,
    /// The client is new.
    Fresh,
}

/// The notices of one server, and who has been told.
pub(crate) struct Notices {
    config: AttachNotices,
    until: Instant,
    told: Mutex<HashSet<(Kind, String)>>,
}

impl Notices {
    /// The notices of a server that starts now.
    pub(crate) fn new(config: AttachNotices) -> Notices {
        let until = Instant::now() + config.window;
        Notices {
            config,
            until,
            told: Mutex::new(HashSet::new()),
        }
    }

    /// The sentence for a client of `kind` with session `token`, if there is one, the window is
    /// open and the client has not been told it yet.
    pub(crate) fn for_client(&self, kind: Kind, token: Option<&str>) -> Option<String> {
        if Instant::now() >= self.until {
            return None;
        }
        let text = match kind {
            Kind::Resumed => self.config.resumed.as_ref(),
            Kind::Fresh => self.config.fresh.as_ref(),
        }?;
        if let Some(token) = token {
            if !self.told.lock().insert((kind, token.to_owned())) {
                return None;
            }
        }
        Some(text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notices(window: Duration) -> Notices {
        Notices::new(AttachNotices {
            resumed: Some("kept".to_owned()),
            fresh: Some("reset".to_owned()),
            window,
        })
    }

    #[test]
    fn each_kind_gets_its_own_sentence() {
        let n = notices(Duration::from_secs(60));
        assert_eq!(n.for_client(Kind::Resumed, Some("a")).as_deref(), Some("kept"));
        assert_eq!(n.for_client(Kind::Fresh, Some("b")).as_deref(), Some("reset"));
    }

    #[test]
    fn every_client_is_told_once() {
        let n = notices(Duration::from_secs(60));
        assert!(n.for_client(Kind::Resumed, Some("a")).is_some());
        assert!(n.for_client(Kind::Resumed, Some("b")).is_some(), "a second client is told too");
        assert!(n.for_client(Kind::Resumed, Some("a")).is_none(), "but not the same client twice");
        assert!(n.for_client(Kind::Resumed, None).is_some(), "no token: told per connection");
        assert!(n.for_client(Kind::Resumed, None).is_some());
    }

    #[test]
    fn nothing_is_said_after_the_window() {
        let n = notices(Duration::ZERO);
        assert!(n.for_client(Kind::Resumed, Some("a")).is_none());
    }

    #[test]
    fn a_missing_sentence_is_not_sent() {
        let n = Notices::new(AttachNotices {
            fresh: Some("reset".to_owned()),
            ..AttachNotices::default()
        });
        assert!(n.for_client(Kind::Resumed, Some("a")).is_none());
        assert!(n.for_client(Kind::Fresh, Some("a")).is_some());
    }
}
